#!/usr/bin/env python3
"""Paired worker-policy benchmark using Phase 5 generation, cache, safety and accounting."""
import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import random
import shutil
import signal
import statistics
import tempfile
import time

import phase5

MIB = phase5.MIB
WORKLOADS = {
    "one-large": [(1, 512 * MIB)],
    "several-large": [(8, 64 * MIB)],
    "many-small": [(4096, 64 * 1024)],
    "many-1mib": [(4096, MIB)],
    "mixed": [(2048, 4096), (128, MIB), (4, 64 * MIB)],
    "short": [(1, 4096)],
}


def make_source(source, groups, guard):
    source.mkdir(mode=0o755)
    for index, (count, size) in enumerate(groups):
        part = source / f"group-{index}"
        phase5.make_source(part, count, size, guard)
        for path in part.iterdir():
            path.rename(source / f"{index}-{path.name}")
        part.rmdir()
    os.utime(source, ns=(1600000000000000000, 1600000000000000000))
    phase5.durable_tree(source)


def is_worker(name):
    return name.startswith('tcopy-') and name[6:].isdigit()


def summarize(samples):
    rows = []
    for workload, tool in sorted({(s['workload'], s['tool']) for s in samples}):
        group = [s for s in samples if s['workload'] == workload and s['tool'] == tool]
        row = dict(workload=workload, tool=tool, n=len(group))
        for field in ('seconds', 'mib_s', 'files_s', 'cpu_percent', 'max_rss_kib',
                      'sampled_peak_rss_kib', 'sampled_peak_fds'):
            values = [s[field] for s in group if s[field] is not None]
            row[field] = dict(median=statistics.median(values), min=min(values), max=max(values)) if values else None
        rows.append(row)
    return rows


def save(result, output):
    result['summaries'] = summarize(result['samples'])
    temporary = output.with_suffix('.tmp')
    temporary.write_text(json.dumps(result, indent=2) + '\n')
    temporary.replace(output.with_suffix('.json'))
    lines = ['# Paired worker-policy benchmark', '', f"Status: {result['status']}", '',
             'TurboCopy timings include its native fsync barriers; cp/rsync timings include a final',
             'external tree fsync. Source cache preparation and residency are recorded per sample.',
             'Every sample requires exact content/metadata verification. Device/controller and metadata',
             'caches are uncontrolled. CPU includes',
             'the final barrier. RSS from child rusage can include pre-exec Python memory;',
             'sampled RSS/FD peaks observe the actual executable at 100 ms intervals and may miss peaks.', '',
             '| Workload | Tool | n | MiB/s median (range) | Files/s | Seconds | CPU % | Sampled RSS MiB | Sampled FDs |',
             '| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: |']
    for row in result['summaries']:
        rate = row['mib_s']
        rss = row['sampled_peak_rss_kib']
        fds = row['sampled_peak_fds']
        rss_text = f"{rss['median']/1024:.1f}" if rss else 'n/a'
        fds_text = f"{fds['median']:g}" if fds else 'n/a'
        lines.append(f"| {row['workload']} | {row['tool']} | {row['n']} | "
                     f"{rate['median']:.1f} ({rate['min']:.1f}–{rate['max']:.1f}) | "
                     f"{row['files_s']['median']:.1f} | {row['seconds']['median']:.4f} | "
                     f"{row['cpu_percent']['median']:.1f} | "
                     f"{rss_text} | {fds_text} |")
    if result.get('error'):
        lines += ['', result['error']]
    output.with_suffix('.md').write_text('\n'.join(lines) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, default=Path('.'))
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path)
    parser.add_argument('--auto-only', action='store_true', help='focused before/after confirmation without rerunning baseline tools')
    parser.add_argument('--cache', choices=['storage', 'warm'], default='storage')
    parser.add_argument('--reserve-gib', type=float, default=20)
    parser.add_argument('--runs', type=int, default=3)
    parser.add_argument('--seed', type=int, default=20261017)
    parser.add_argument('--workload', action='append', choices=WORKLOADS)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.runs < 3 or not math.isfinite(args.reserve_gib) or args.reserve_gib < 1:
        parser.error('at least three repetitions and a reserve of at least 1 GiB required')
    directory = args.directory.resolve(strict=True)
    before = args.before.resolve(strict=True)
    binaries = {str(before)}
    commands = {'auto before': [str(before), '--json'],
                **{f'manual {n}': [str(before), '--workers', str(n), '--json'] for n in (4, 8, 16)},
                'cp': ['cp', '-a', '--reflink=never', '--sparse=never'], 'rsync': ['rsync', '-a']}
    if args.after:
        after = args.after.resolve(strict=True)
        binaries.add(str(after))
        commands['auto after'] = [str(after), '--json']
    if args.auto_only:
        commands = {name: command for name, command in commands.items() if name.startswith('auto')}
    result = dict(status='running', invocation=os.sys.argv, started_utc=datetime.now(timezone.utc).isoformat(),
                  revision=phase5.capture(['git', 'rev-parse', 'HEAD'])['stdout'],
                  filesystem=phase5.filesystem(directory), memory_available_bytes=phase5.memory_available(),
                  binaries={p: hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in sorted(binaries)},
                  script_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  runs=args.runs, seed=args.seed, cache=args.cache, reserve_gib=args.reserve_gib, samples=[])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    def interrupted(signum, frame):
        raise KeyboardInterrupt(f'signal {signum}')
    signal.signal(signal.SIGTERM, interrupted)
    rng = random.Random(args.seed)
    try:
        for workload in args.workload or WORKLOADS:
            groups = WORKLOADS[workload]
            count = sum(n for n, size in groups)
            size = sum(n * size for n, size in groups)
            phase5.check_space([directory, directory], [size + count * 4096] * 2, args.reserve_gib, .10)
            if args.cache == 'warm' and size > phase5.memory_available() // 2:
                raise phase5.StopBenchmark('warm source exceeds half available memory')
            with tempfile.TemporaryDirectory(prefix='tcopy-workers-', dir=directory) as owned:
                root = Path(owned)
                source = root / 'source'
                make_source(source, groups, lambda: phase5.check_space([directory], [0], args.reserve_gib, .10))
                expected = phase5.checked_manifest(source)
                files = sorted(source.iterdir())
                for run in range(args.runs):
                    order = list(commands)
                    rng.shuffle(order)
                    for tool in order:
                        destination = root / 'destination'
                        destination.mkdir()
                        fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
                        try:
                            os.fsync(fd)
                        finally:
                            os.close(fd)
                        cache = phase5.prepare_cache(files, args.cache)
                        if cache['status'] != 'controlled':
                            raise phase5.StopBenchmark(f'uncontrolled source cache: {cache}')
                        waits = Counter()
                        peak_rss = peak_fds = None
                        last_profile = last_pressure = 0
                        stressed_since = None

                        def guard():
                            nonlocal last_profile, last_pressure, peak_rss, peak_fds, stressed_since
                            now = time.monotonic()
                            if now - last_pressure >= 1:
                                psi = phase5.check_space([directory], [0], args.reserve_gib, .10)
                                stressed = any(float(psi.get(kind, {}).get('full', {}).get('avg10', 0)) >= limit
                                               for kind, limit in [('io', 50), ('memory', 10)])
                                stressed_since = (stressed_since or now) if stressed else None
                                if stressed_since and now - stressed_since >= 10:
                                    raise phase5.StopBenchmark('sustained excessive I/O or memory pressure')
                                last_pressure = now
                            if now - last_profile < .1:
                                return
                            last_profile = now
                            for proc in Path('/proc').iterdir():
                                if not proc.name.isdecimal():
                                    continue
                                try:
                                    if os.readlink(proc / 'exe') not in binaries:
                                        continue
                                    fields = dict(line.split(':', 1) for line in (proc / 'status').read_text().splitlines())
                                    rss = int(fields.get('VmRSS', '0').split()[0])
                                    peak_rss = max(peak_rss or 0, rss)
                                    peak_fds = max(peak_fds or 0, len(list((proc / 'fd').iterdir())))
                                    for task in (proc / 'task').iterdir():
                                        if is_worker((task / 'comm').read_text().strip()):
                                            waits[(task / 'wchan').read_text().strip()] += 1
                                except (FileNotFoundError, ProcessLookupError, PermissionError):
                                    pass

                        metrics, stdout = phase5.run_tool([*commands[tool], str(source), str(destination)], root, guard)
                        sync_started = time.perf_counter()
                        sync_cpu = time.process_time()
                        telemetry = json.loads(stdout) if tool.startswith(('auto', 'manual')) else None
                        if telemetry:
                            if telemetry['failures'] or telemetry['copied'] != count or telemetry['transferred_bytes'] != size:
                                raise phase5.StopBenchmark(f'incorrect transfer counters: {telemetry}')
                        else:
                            phase5.durable_tree(destination)
                        sync_seconds = time.perf_counter() - sync_started
                        cpu = metrics['user_cpu_seconds'] + metrics['system_cpu_seconds'] + time.process_time() - sync_cpu
                        seconds = metrics['copy_seconds'] + sync_seconds
                        if phase5.checked_manifest(destination / 'source') != expected:
                            raise phase5.StopBenchmark(f'content or metadata mismatch: {tool}')
                        sample = dict(workload=workload, run=run+1, tool=tool, cache=cache, **metrics,
                                      sync_seconds=sync_seconds, seconds=seconds, mib_s=size/MIB/seconds,
                                      files_s=count/seconds, cpu_percent=100*cpu/seconds,
                                      sampled_peak_rss_kib=peak_rss, sampled_peak_fds=peak_fds,
                                      worker_wait_samples=dict(waits), tcopy=telemetry)
                        result['samples'].append(sample)
                        shutil.rmtree(destination)
                        save(result, args.output)
                        print(f"{workload} run={run+1} {tool}: {seconds:.4f}s {size/MIB/seconds:.1f} MiB/s verified", flush=True)
        result['status'] = 'completed'
    except (Exception, KeyboardInterrupt) as error:
        result['status'] = 'stopped'
        result['error'] = str(error) or 'interrupted'
    finally:
        result['finished_utc'] = datetime.now(timezone.utc).isoformat()
        save(result, args.output)
    return 0 if result['status'] == 'completed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
