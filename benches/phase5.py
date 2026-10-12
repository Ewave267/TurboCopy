#!/usr/bin/env python3
"""Linux Phase 5: verified dense copies, bounded space, and explicit cache conditions."""
import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import html
import json
import math
import os
from pathlib import Path
import platform
import random
import re
import resource
import shutil
import signal
import statistics
import subprocess
import tempfile
import time
import zlib

from phase4 import durable_tree, manifest

GIB = 1 << 30
MIB = 1 << 20
DEFAULTS = [("one-10gib", 1, 10 * GIB), ("ten-1gib", 10, GIB),
            ("hundred-100mib", 100, 100 * MIB), ("ten-thousand-1mib", 10000, MIB)]
LIBC = ctypes.CDLL(None, use_errno=True)
LIBC.mmap.restype = ctypes.c_void_p
LIBC.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int,
                      ctypes.c_int, ctypes.c_int, ctypes.c_long]
LIBC.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]
LIBC.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]


class StopBenchmark(RuntimeError):
    pass


def capture(command):
    p = subprocess.run(command, capture_output=True, text=True)
    return {"command": command, "returncode": p.returncode,
            "stdout": p.stdout.strip(), "stderr": p.stderr.strip()}


def memory_available():
    values = dict(line.split(":", 1) for line in Path("/proc/meminfo").read_text().splitlines())
    available = int(values["MemAvailable"].split()[0]) * 1024
    # Respect a container's memory budget as well as host RAM.
    for line in Path("/proc/self/cgroup").read_text().splitlines():
        if line.startswith("0::"):
            base = Path("/sys/fs/cgroup") / line[3:].lstrip("/")
            while base != Path("/sys/fs"):
                limit = base / "memory.max"
                if limit.exists() and limit.read_text().strip() != "max":
                    available = min(available, max(0, int(limit.read_text()) -
                                    int((base / "memory.current").read_text())))
                base = base.parent
    return available


def pressure():
    result = {}
    for kind in ("io", "memory"):
        path = Path("/proc/pressure") / kind
        if path.exists():
            result[kind] = {line.split()[0]: dict(part.split("=") for part in line.split()[1:])
                            for line in path.read_text().splitlines()}
    return result


def physical_devices(major_minor):
    node = Path("/sys/dev/block") / major_minor
    if not node.exists():
        return []
    node = node.resolve()
    slaves = list((node / "slaves").iterdir()) if (node / "slaves").exists() else []
    if slaves:
        return sorted({name for slave in slaves
                       for name in physical_devices((slave / "dev").read_text().strip())})
    if (node / "partition").exists():
        node = node.parent
    if "/virtual/" in str(node):
        return []
    return [node.name]


def filesystem(path):
    p = capture(["findmnt", "-J", "-T", str(path), "-o", "TARGET,SOURCE,FSTYPE,OPTIONS,MAJ:MIN"])
    if p["returncode"]:
        raise StopBenchmark(f"cannot identify filesystem: {p}")
    fs = json.loads(p["stdout"])["filesystems"][-1]
    usage = shutil.disk_usage(path)
    fs.update(path=str(path), st_dev=path.stat().st_dev, total_bytes=usage.total,
              free_bytes=usage.free, physical_devices=physical_devices(fs["maj:min"]))
    if fs["fstype"] == "btrfs":
        fs["btrfs_usage"] = capture(["btrfs", "filesystem", "usage", "-b", str(path)])
        fs["btrfs_devices"] = capture(["btrfs", "filesystem", "show", "--raw", fs["target"]])
        paths = re.findall(r" path (.+?)(?: MISSING)?$", fs["btrfs_devices"]["stdout"], re.M)
        names = []
        resolved = 0
        for device_path in paths:
            before = len(names)
            device = Path(device_path)
            if device.exists():
                number = device.stat().st_rdev
                names.extend(physical_devices(f"{os.major(number)}:{os.minor(number)}"))
            else:
                # Btrfs st_dev is virtual; sandboxed /dev can hide a mapper visible in sysfs.
                for node in Path("/sys/class/block").iterdir():
                    mapper = node / "dm/name"
                    if mapper.exists() and mapper.read_text().strip() == device.name:
                        names.extend(physical_devices((node / "dev").read_text().strip()))
            resolved += len(names) > before
        total = re.search(r"Total devices (\d+)", fs["btrfs_devices"]["stdout"])
        complete = total and resolved == len(paths) == int(total.group(1))
        fs["physical_devices"] = sorted(set(names)) if complete else []
    return fs


def check_space(parents, requirements, reserve_gib, reserve_fraction):
    grouped = {}
    for path, needed in zip(parents, requirements):
        device = path.stat().st_dev
        grouped.setdefault(device, [path, 0])[1] += needed
    for path, needed in grouped.values():
        usage = shutil.disk_usage(path)
        reserve = max(int(reserve_gib * GIB), int(usage.total * reserve_fraction))
        if usage.free - needed < reserve:
            raise StopBenchmark(f"insufficient space at {path}: free={usage.free}, "
                                f"additional={needed}, required reserve={reserve}")
        stats = os.statvfs(path)
        if stats.f_files and stats.f_favail < 25000:
            raise StopBenchmark(f"insufficient free inodes at {path}")
    if memory_available() < 512 * MIB:
        raise StopBenchmark("less than 512 MiB available memory")
    return pressure()


def resident_fraction(files):
    pages = resident = 0
    page_size = os.sysconf("SC_PAGE_SIZE")
    for path in files:
        size = path.stat().st_size
        if not size:
            continue
        count = math.ceil(size / page_size)
        vector = (ctypes.c_ubyte * count)()
        with path.open("rb") as handle:
            address = LIBC.mmap(None, size, 1, 1, handle.fileno(), 0)
            if address == ctypes.c_void_p(-1).value:
                raise OSError(ctypes.get_errno(), "mmap for mincore failed")
            try:
                if LIBC.mincore(address, size, vector):
                    raise OSError(ctypes.get_errno(), "mincore failed")
                resident += sum(byte & 1 for byte in vector)
                pages += count
            finally:
                LIBC.munmap(address, size)
    return resident / pages if pages else 0.0


def prepare_cache(files, mode):
    started = time.perf_counter()
    for path in files:
        with path.open("rb") as handle:
            if mode == "warm":
                while handle.read(4 * MIB):
                    pass
            else:
                os.posix_fadvise(handle.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
    fraction = resident_fraction(files)
    controlled = fraction >= 0.99 if mode == "warm" else fraction <= 0.01
    return {"mode": mode, "source_resident_fraction": fraction,
            "status": "controlled" if controlled else "uncontrolled",
            "preparation_seconds": time.perf_counter() - started}


def checked_manifest(root):
    result = manifest(root)
    for relative, values in result.items():
        st = (root / relative).stat()
        values[3].update({name: os.getxattr(root / relative, name)
                          for name in os.listxattr(root / relative)
                          if name.startswith("system.posix_acl")})
        result[relative] = (*values, st.st_uid, st.st_gid, st.st_size if st.st_size and
                            (root / relative).is_file() else 0)
    return result


def make_source(source, count, size, guard):
    source.mkdir(mode=0o755)
    creation = metadata = data = 0.0
    sample_ratios = []
    for index in range(count):
        if index % 64 == 0:
            guard()
        path = source / f"file-{index:05d}"
        started = time.perf_counter()
        with path.open("xb") as handle:
            creation += time.perf_counter() - started
            started = time.perf_counter()
            remaining = size
            while remaining:
                block = os.urandom(min(4 * MIB, remaining))
                if len(sample_ratios) < 8:
                    sample_ratios.append(len(zlib.compress(block)) / len(block))
                handle.write(block)
                remaining -= len(block)
                guard()
            data += time.perf_counter() - started
        started = time.perf_counter()
        path.chmod(0o640)
        os.utime(path, ns=(1600000000000000000, 1600000000000000000))
        metadata += time.perf_counter() - started
    os.utime(source, ns=(1600000000000000000, 1600000000000000000))
    started = time.perf_counter()
    durable_tree(source)
    sync = time.perf_counter() - started
    for path in source.iterdir():
        if path.stat().st_blocks * 512 < size:
            raise StopBenchmark(f"source is sparse or compressed: {path}")
        if any(name.startswith(("user.", "system.posix_acl")) for name in os.listxattr(path)):
            raise StopBenchmark("inherited extended metadata violates the plain archive dataset")
    if any(ratio < 0.99 for ratio in sample_ratios):
        raise StopBenchmark("generated data is compressible")
    return {"file_creation_seconds": creation, "data_generation_write_seconds": data,
            "metadata_seconds": metadata, "sync_seconds": sync,
            "compression_sample_ratios": sample_ratios,
            "allocated_bytes": sum(p.stat().st_blocks * 512 for p in source.iterdir())}


def block_counters(devices):
    result = {}
    for device in devices:
        fields = (Path("/sys/class/block") / device / "stat").read_text().split()
        result[device] = {"read_bytes": int(fields[2]) * 512,
                          "write_bytes": int(fields[6]) * 512}
    return result


def run_tool(command, log_root, guard):
    stdout = log_root / "stdout"
    stderr = log_root / "stderr"
    measurement = log_root / "measurement.json"
    ready_read, ready_write = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(ready_read)
        os.setsid()
        signal.signal(signal.SIGINT, signal.SIG_DFL)
        signal.signal(signal.SIGTERM, signal.SIG_DFL)
        os.write(ready_write, b"1")
        os.close(ready_write)
        try:
            with stdout.open("w") as out, stderr.open("w") as err:
                started = time.perf_counter()
                completed = subprocess.run(command, stdout=out, stderr=err)
                elapsed = time.perf_counter() - started
            usage = resource.getrusage(resource.RUSAGE_CHILDREN)
            io = dict((key, int(value)) for key, value in
                      (line.split(":") for line in Path("/proc/self/io").read_text().splitlines()))
            cpu = usage.ru_utime + usage.ru_stime
            measurement.write_text(json.dumps({"returncode": completed.returncode,
                "copy_seconds": elapsed, "user_cpu_seconds": usage.ru_utime,
                "system_cpu_seconds": usage.ru_stime, "cpu_percent_one_core": 100 * cpu / elapsed,
                "max_rss_kib": usage.ru_maxrss, "process_io": io}))
            os._exit(0)
        except BaseException as error:
            try:
                measurement.write_text(json.dumps({"error": str(error)}))
            finally:
                os._exit(1)
    os.close(ready_write)
    try:
        os.read(ready_read, 1)
        os.close(ready_read)
        while not os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT):
            guard()
            time.sleep(0.005)
        _, status, _ = os.wait4(pid, 0)
    except BaseException:
        # Only the process group created above belongs to this benchmark.
        try:
            os.killpg(pid, signal.SIGTERM)
            deadline = time.monotonic() + 5
            while not os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT):
                if time.monotonic() >= deadline:
                    os.killpg(pid, signal.SIGKILL)
                    break
                time.sleep(0.05)
            os.wait4(pid, 0)
        except ProcessLookupError:
            pass
        raise
    metrics = json.loads(measurement.read_text())
    if os.waitstatus_to_exitcode(status) or metrics.pop("returncode", 1):
        raise StopBenchmark(f"tool failed: {command}: {metrics.get('error', '')} {stderr.read_text()[:2000]}")
    return metrics, stdout.read_text()


def summarize(result):
    groups = {}
    for sample in result["samples"]:
        if sample["status"] != "verified" or sample["cache"]["status"] != "controlled":
            continue
        key = (sample["pair"], sample["workload"], sample["scenario"], sample["cache"]["mode"], sample["tool"])
        groups.setdefault(key, []).append(sample)
    result["summaries"] = []
    for key, samples in groups.items():
        row = dict(zip(("pair", "workload", "scenario", "cache", "tool"), key))
        row.update(n=len(samples), complete=len(samples) >= result["runs"])
        for field in ("seconds", "copy_seconds", "sync_seconds", "cpu_percent_one_core",
                      "throughput_mib_s", "files_per_second"):
            vals = [s[field] for s in samples if s[field] is not None]
            row[field] = statistics.median(vals) if vals else None
        rates = [s["throughput_mib_s"] for s in samples if s["throughput_mib_s"] is not None]
        row["min_mib_s"] = min(rates) if rates else None
        row["max_mib_s"] = max(rates) if rates else None
        result["summaries"].append(row)


def write_report(result, output):
    summarize(result)
    output.with_suffix(".json").write_text(json.dumps(result, indent=2) + "\n")
    lines = ["# TurboCopy Phase 5: large datasets and parallel throughput", "",
             f"Status: **{result['status']}**. {result.get('stop_reason', '')}", "",
             f"Recorded {result['started_utc']}; source revision `{result['revision']}`.", "",
             f"Verified {sum(s['status'] == 'verified' and s['scenario'] == 'dense' for s in result['samples'])} "
             f"dense copies and {sum(s['status'] == 'verified' and s['scenario'] == 'metadata-only' for s in result['samples'])} "
             "empty-file companion copies. The copy engine was unchanged.", "",
             "## Storage and safety", "",
             "Inventory, commands, binary hash, individual samples and cache checks are in "
             f"[{output.with_suffix('.json').name}]({output.with_suffix('.json').name}). Only owned temporary trees are removed. "
             "No storage configuration or global caches are changed. One source and one destination "
             "are retained at a time. Free-space reserves, free inodes, available RAM and PSI are "
             "checked before and during execution; failures stop the benchmark and retain results.", "",
             "| Parent | Filesystem / mount | Physical storage | Initial free GiB |",
             "| --- | --- | --- | ---: |"]
    device_models = {}
    inventory = result["block_inventory"]
    nodes = json.loads(inventory["stdout"]).get("blockdevices", []) if inventory["returncode"] == 0 else []
    while nodes:
        node = nodes.pop()
        device_models[node["name"]] = node.get("model")
        nodes.extend(node.get("children", []))
    for fs in result["filesystems"]:
        devices = ", ".join(f"{name} ({device_models.get(name) or 'model unavailable'})"
                            for name in fs["physical_devices"])
        lines.append(f"| `{fs['path']}` | {fs['fstype']} / `{fs['target']}` | "
                     f"{devices or 'unresolved'} | {fs['free_bytes']/GIB:.2f} |")
    reserves = result["reserves"]
    lines += ["", f"Free space must exceed the dataset working set plus the larger of "
              f"{reserves['gib']:g} GiB and {100*reserves['fraction']:g}% of filesystem capacity. "
              "The guard stops below 512 MiB available RAM, or after 10 seconds with "
              "I/O full PSI avg10 ≥50% or memory full PSI avg10 ≥10%. "
              "Existing compression options remain in place; dense random "
              "data avoids a compression advantage. Mount options and Btrfs allocation estimates "
              "are retained in JSON."]
    lines += ["", result["cross_device_note"], "", "## Method", "",
              "Defaults: 1 × 10 GiB, 10 × 1 GiB, 100 × 100 MiB (9.765625 GiB), "
              "10,000 × 1 MiB (9.765625 GiB). Fresh OS random bytes are written throughout each "
              "file, with no repeated blocks or sparse seeks; allocation and sampled zlib ratios "
              "are checked. Sources use identical modes, ownership and nanosecond mtimes, with no "
              "intentional ACLs, hard links or user xattrs. Every destination is new. SHA-256, "
              "file set, size, mode, uid/gid and mtime are verified outside the measured interval. "
              "Atime/ctime and security labels are outside the shared preservation contract; "
              "rsync -a and existing default TurboCopy do not preserve all of those fields.", "",
              "GNU `cp -a --reflink=never --sparse=never`, `rsync -a`, `tcopy --auto`, and "
              "`tcopy --workers N` for N = 1, 2, 4, 8, 16 are compared. TurboCopy also emits JSON. "
              "Order is shuffled with a recorded seed for each of at least three iterations. "
              "No reflink, sparse, resume, incremental or explicit chunk options are enabled.", "",
              "The common durability endpoint is tool completion plus fsync of every destination "
              "file and directory, including the containing directory. Total time and throughput "
              "include this final barrier, whose time is also recorded separately. TurboCopy's "
              "existing per-file sync and atomic publication remain included in its tool time; "
              "intermediate failure guarantees are stronger and cannot be equalized without "
              "changing the engine. Final metadata and durability requirements are identical. "
              "The common final tree fsync is serial, so tools that already flushed in parallel "
              "can spend less time in that barrier; copy-only time is reported separately.", "",
              "Storage runs request POSIX_FADV_DONTNEED on clean source files and require ≤1% "
              "source-page residency via mincore immediately before copying. Warm runs read the "
              "entire source and require ≥99% residency. Warm runs that cannot fit within half "
              "the currently available memory are skipped. Uncontrolled samples never enter "
              "the comparison. Metadata, controller/device caches, CPU frequency, SSD temperature, "
              "compression mount options and other host activity remain uncontrolled. Storage "
              "runs measure verified source page-cache eviction, not a fully cold device.", "",
              "wait4 captures tool/descendant CPU time; CPU percent uses one core = 100%. "
              "A dedicated supervisor reads `/proc/self/io` after waiting for the tool, including "
              "waited descendants and a small amount of supervisor I/O. These are "
              "process-attributed storage bytes and logical syscall counters, not exact physical "
              "traffic; delayed writeback and metadata accounting can differ. Physical device "
              "sector deltas cover tool execution plus final sync and include other host I/O. "
              "Verification and source preparation are excluded. Empty-file companion copies "
              "measure creation/metadata/sync overhead separately; their cache is explicitly "
              "uncontrolled and they are not subtracted from dense-copy times. TurboCopy's "
              "metadata/transfer timers are sums across concurrent workers, not wall time.", "",
              "## Measured throughput", "",
              "Only verified, controlled groups with all requested iterations are graphed. "
              "Bars are median durable MiB/s; whiskers are observed min–max across iterations. "
              "Workload graphs share a scale within each pair/cache condition. All raw I/O/CPU metrics are in JSON.", ""]
    dense = [r for r in result["summaries"] if r["scenario"] == "dense" and r["complete"]]
    workload_order = {w[0]: i for i, w in enumerate(result["workloads"])}
    tool_order = {name: i for i, name in enumerate(result["tools"])}
    dense.sort(key=lambda r: (r["pair"], workload_order[r["workload"]], r["cache"], tool_order[r["tool"]]))
    plots = [(pair, cache, index, w[0]) for pair in range(len(result["pairs"]))
             for cache in ("storage", "warm") for index, w in enumerate(result["workloads"])]
    for pair, cache, workload_index, workload in plots:
        rows = [r for r in dense if r["pair"] == pair and r["cache"] == cache and r["workload"] == workload]
        if not rows:
            continue
        graph_path = output.parent / f"{output.stem}-{pair}-{cache}-{workload_index}.svg"
        height = 65 + 25 * len(rows)
        maximum = max(r["max_mib_s"] for r in dense if r["pair"] == pair and r["cache"] == cache)
        title = html.escape(f"{workload}, pair {pair}, {cache}: durable MiB/s (median and observed range)")
        svg = [f'<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="{height}" '
               f'viewBox="0 0 1000 {height}" role="img"><title>{title}</title>'
               '<rect width="100%" height="100%" fill="white"/>'
               '<g font-family="sans-serif" font-size="12" fill="#172033">',
               f'<text x="15" y="25">{title}</text>']
        for index, row in enumerate(rows):
            y = 50 + index * 25
            rate = row["throughput_mib_s"]
            low = 350 + 500 * row["min_mib_s"] / maximum
            high = 350 + 500 * row["max_mib_s"] / maximum
            color = "#2563eb" if row["tool"].startswith("tcopy") else "#64748b"
            svg += [f'<text x="15" y="{y+13}">{html.escape(row["tool"])}</text>',
                    f'<rect x="350" y="{y}" width="{500*rate/maximum:.1f}" height="17" fill="{color}"/>',
                    f'<path d="M {low:.1f} {y+8} H {high:.1f} M {low:.1f} {y+4} V {y+12} '
                    f'M {high:.1f} {y+4} V {y+12}" stroke="#172033" fill="none"/>',
                    f'<text x="880" y="{y+13}">{rate:.1f}</text>']
        graph_path.write_text("\n".join(svg + ["</g></svg>"]))
        lines += [f"![{workload}, pair {pair}, {cache} throughput]({graph_path.name})", ""]
    lines += ["| Pair | Workload | Cache | Tool | n | Durable MiB/s | Copy s | Total s | Final sync s | CPU % |",
              "| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for row in dense:
        lines.append(f"| {row['pair']} | {row['workload']} | {row['cache']} | {row['tool']} | "
                     f"{row['n']} | {row['throughput_mib_s']:.1f} | {row['copy_seconds']:.3f} | {row['seconds']:.3f} | "
                     f"{row['sync_seconds']:.3f} | {row['cpu_percent_one_core']:.1f} |")
    lines += ["", "## Parallelism analysis", ""]
    for pair in range(len(result["pairs"])):
        for workload, _, _ in result["workloads"]:
            for cache in ("storage", "warm"):
                rows = [r for r in dense if r["pair"] == pair and r["workload"] == workload and r["cache"] == cache]
                single = next((r for r in rows if r["tool"] == "tcopy workers 1"), None)
                parallel = [r for r in rows if r["tool"].startswith("tcopy workers ") and r is not single]
                if not single or not parallel:
                    continue
                best = max(parallel, key=lambda r: r["throughput_mib_s"])
                gain = best["throughput_mib_s"] / single["throughput_mib_s"]
                separated = best["min_mib_s"] > single["max_mib_s"]
                lines.append(f"- Pair {pair}, {workload}, {cache}: best parallel setting is "
                             f"**{best['tool']}**, {gain:.2f}× workers 1. "
                             f"{'Its entire observed range exceeds workers 1.' if separated else 'Observed ranges overlap or parallelism is slower; no clear gain beyond noise.'}")
    if not dense:
        lines += ["No complete controlled measurement groups; no throughput or parallelism conclusion is supported."]
    automatic = [s["tcopy"] for s in result["samples"] if s["status"] == "verified"
                 and s["scenario"] == "dense" and s["tool"] == "tcopy auto"]
    limits = ", ".join(f"{initial} → {final}" for initial, final in sorted({
        (s["workers"], s["final_workers"]) for s in automatic})) or "unmeasured"
    lines += ["", "One dense file exposes no file-level parallelism in manual worker mode without "
              "chunks. More files can occupy more workers; additional workers can also increase "
              "same-device contention and per-file fsync costs. Treat these as explanations to "
              "check against recorded worker/chunk telemetry, not performance claims for untested storage.", "",
              f"Observed automatic-mode initial → final worker limits: **{limits}** "
              f"across {len(automatic)} verified dense copies. These reflect the existing pressure policy. "
              "Prior verification and preparation I/O can influence its recent pressure window. "
              "Manual workers retain their limits. The 10,000-file runs spend most of their "
              "summed worker time in metadata/sync work, so overlapping those waits produces "
              "the largest measured parallel gain. Observed ranges are not confidence intervals.", "",
              "## Read/write accounting", "",
              "Medians in GiB. Device totals cover all backing devices and include other host activity; "
              "process totals cover the tool and supervisor, ending before the final external fsync.", "",
              "| Pair/cache | Workload | Tool | Process read GiB | Process write GiB | Device read GiB | Device write GiB |",
              "| --- | --- | --- | ---: | ---: | ---: | ---: |"]
    for row in dense:
        samples = [s for s in result["samples"] if s["status"] == "verified" and s["scenario"] == "dense"
                   and s["cache"]["status"] == "controlled" and s["pair"] == row["pair"]
                   and s["workload"] == row["workload"] and s["tool"] == row["tool"]
                   and s["cache"]["mode"] == row["cache"]]
        proc = [statistics.median(s["process_io"][field] for s in samples) / GIB
                for field in ("read_bytes", "write_bytes")]
        device = [statistics.median(sum(values[field] for values in s["device_io_including_other_activity"].values())
                                    for s in samples) / GIB for field in ("read_bytes", "write_bytes")]
        lines.append(f"| {row['pair']}/{row['cache']} | {row['workload']} | {row['tool']} | "
                     + " | ".join(f"{value:.3f}" for value in proc + device) + " |")
    lines += ["",
              "## Creation and metadata overhead", "",
              "| Pair | Workload | Tool | Median empty-tree s | Files/s | Final sync s |",
              "| --- | --- | --- | ---: | ---: | ---: |"]
    metadata_groups = {}
    for s in result["samples"]:
        if s["scenario"] == "metadata-only" and s["status"] == "verified":
            metadata_groups.setdefault((s["pair"], s["workload"], s["tool"]), []).append(s)
    for (pair, workload, tool), samples in metadata_groups.items():
        if len(samples) >= result["runs"]:
            lines.append(f"| {pair} | {workload} | {tool} | {statistics.median(s['seconds'] for s in samples):.3f} | "
                         f"{statistics.median(s['files_per_second'] for s in samples):.1f} | "
                         f"{statistics.median(s['sync_seconds'] for s in samples):.3f} |")
    lines += ["", "Source generation, file creation, metadata setup and sync times are recorded "
              "separately in `datasets`; generation time includes randomness and pressure checks. "
              "Creation measures file open only; these setup timers do not include every setup operation.", "",
              "## Unsupported and uncontrolled conditions", ""]
    lines += [f"- {item}" for item in result["omitted"]]
    unsupported = sum(s["status"] == "unsupported-cache" for s in result["samples"])
    if unsupported:
        lines.append(f"- {unsupported} samples skipped because cache residency could not be controlled; see JSON.")
    if any("No such file" in fs.get("btrfs_usage", {}).get("stderr", "") for fs in result["filesystems"]):
        lines.append("- Detailed Btrfs per-device allocation is unavailable: mapped /dev nodes are hidden "
                     "in this execution namespace. The userspace MISSING text reflects that visibility limit; "
                     "the live mount, capacity estimates and backing NVMe remain visible through the kernel.")
    lines += ["", "## Reproduce", "", "```bash", "cargo build --release",
              "python3 benches/phase5.py --directory . --runs 3 --output docs/benchmark-phase5",
              "# Repeat --directory for independently mounted writable physical devices.",
              "# Override defaults with repeatable --dataset NAME:COUNT:BYTES.",
              "# --inventory-only detects storage and checks capacity without making datasets.",
              "python3 -m unittest discover -s benches -p 'test_phase5.py'", "```", "",
              "The JSON records the exact invocation, reserve thresholds and seed. Custom small "
              "datasets are validation runs and do not substitute for the default large workloads.", "",
              "Sources: [GNU cp](https://www.gnu.org/software/coreutils/manual/html_node/cp-invocation.html), "
              "[posix_fadvise](https://man7.org/linux/man-pages/man2/posix_fadvise.2.html), "
              "[mincore](https://man7.org/linux/man-pages/man2/mincore.2.html), "
              "[proc_pid_io](https://man7.org/linux/man-pages/man5/proc_pid_io.5.html).", ""]
    output.with_suffix(".md").write_text("\n".join(lines))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, action="append", required=True)
    parser.add_argument("--binary", type=Path, default=Path("target/release/tcopy"))
    parser.add_argument("--dataset", action="append", help="NAME:COUNT:BYTES; replaces defaults")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--seed", type=int, default=2026)
    parser.add_argument("--reserve-gib", type=float, default=20)
    parser.add_argument("--reserve-fraction", type=float, default=0.10)
    parser.add_argument("--output", type=Path, default=Path("docs/benchmark-phase5"))
    parser.add_argument("--inventory-only", action="store_true")
    args = parser.parse_args()
    def interrupted(signum, frame):
        raise KeyboardInterrupt(f"signal {signum}")
    signal.signal(signal.SIGTERM, interrupted)
    if args.runs < 3 or not math.isfinite(args.reserve_gib) or args.reserve_gib < 1 or not 0 <= args.reserve_fraction < 1:
        parser.error("at least three runs, reserve-gib >= 1, and reserve-fraction in [0, 1) required")
    workloads = DEFAULTS
    if args.dataset:
        try:
            workloads = [(name, int(count), int(size)) for name, count, size in
                         (value.split(":") for value in args.dataset)]
        except ValueError:
            parser.error("datasets must be NAME:COUNT:BYTES")
        if any(not name or count < 1 or size < 1 for name, count, size in workloads) or len({w[0] for w in workloads}) != len(workloads):
            parser.error("unique dataset names and positive counts/bytes required")
    parents = list(dict.fromkeys(p.resolve(strict=True) for p in args.directory))
    binary = args.binary.resolve(strict=True)
    commands = {"GNU cp -a": ["cp", "-a", "--reflink=never", "--sparse=never"],
                "rsync -a": ["rsync", "-a"], "tcopy auto": [str(binary), "--auto", "--json"]}
    commands.update({f"tcopy workers {n}": [str(binary), "--workers", str(n), "--json"] for n in (1, 2, 4, 8, 16)})
    filesystems = [filesystem(p) for p in parents]
    result = {"schema_version": 1, "status": "running", "started_utc": datetime.now(timezone.utc).isoformat(),
              "invocation": list(os.sys.argv), "revision": capture(["git", "rev-parse", "HEAD"])["stdout"],
              "platform": platform.platform(), "logical_cpus": os.cpu_count(),
              "memory_available_bytes": memory_available(), "filesystems": filesystems,
              "mounts": capture(["findmnt", "-J", "-o", "TARGET,SOURCE,FSTYPE,OPTIONS,MAJ:MIN"]),
              "block_inventory": capture(["lsblk", "-J", "-b", "-o", "NAME,TYPE,SIZE,FSTYPE,MOUNTPOINTS,MODEL,ROTA,PKNAME"]),
              "workloads": workloads, "runs": args.runs, "seed": args.seed,
              "reserves": {"gib": args.reserve_gib, "fraction": args.reserve_fraction,
                           "min_memory_mib": 512, "io_full_avg10_percent": 50,
                           "memory_full_avg10_percent": 10, "pressure_sustained_seconds": 10},
              "tools": commands, "tool_versions": {}, "pairs": [], "datasets": [], "samples": [], "omitted": []}
    with binary.open("rb") as handle:
        result["binary_sha256"] = hashlib.file_digest(handle, "sha256").hexdigest()
    for name, command in commands.items():
        version = capture([command[0], "--version"])
        if version["returncode"]:
            raise StopBenchmark(f"required tool unavailable: {name}")
        result["tool_versions"][name] = version["stdout"].splitlines()[0]
    independent = []
    for i, source in enumerate(filesystems):
        for j, dest in enumerate(filesystems):
            if i == j:
                result["pairs"].append({"source": i, "destination": j, "kind": "same-device"})
            elif source["physical_devices"] and dest["physical_devices"] and not set(source["physical_devices"]) & set(dest["physical_devices"]):
                independent.append({"source": i, "destination": j, "kind": "cross-device"})
    result["pairs"].extend(independent)
    result["cross_device_note"] = ("Independent physical-device directions are included." if independent else
                                   "Cross-device storage benchmark unsupported: no independent writable mounted physical-device pair was selected. tmpfs is not physical storage.")
    rng = random.Random(args.seed)
    try:
        if any(fs["fstype"] not in ("btrfs", "ext4", "xfs", "f2fs") or not fs["physical_devices"] for fs in filesystems):
            raise StopBenchmark("storage benchmarks require a resolved physical-device filesystem (btrfs/ext4/xfs/f2fs)")
        maximum = max(count * (size + 4096) for _, count, size in workloads)
        for parent in parents:
            stats = os.statvfs(parent)
            if stats.f_files and stats.f_favail < 2 * max(w[1] for w in workloads) + 25000:
                raise StopBenchmark(f"insufficient free inodes for configured datasets at {parent}")
        for pair in result["pairs"]:
            selected = [parents[pair["source"]], parents[pair["destination"]]]
            check_space(selected, [maximum, maximum], args.reserve_gib, args.reserve_fraction)
        if args.inventory_only:
            result["status"] = "inventory-only"
        else:
            for pair_index, pair in enumerate(result["pairs"]):
                selected = [parents[pair["source"]], parents[pair["destination"]]]
                devices = sorted(set(filesystems[pair["source"]]["physical_devices"] + filesystems[pair["destination"]]["physical_devices"]))
                for name, count, size in workloads:
                    check_space(selected, [count * (size + 4096)] * 2, args.reserve_gib, args.reserve_fraction)
                    last_check = 0.0
                    pressure_since = None

                    def guard():
                        nonlocal last_check, pressure_since
                        now = time.monotonic()
                        if now - last_check < 1:
                            return
                        last_check = now
                        psi = check_space(selected, [0, 0], args.reserve_gib, args.reserve_fraction)
                        stressed = any(float(psi.get(kind, {}).get("full", {}).get("avg10", 0)) >= limit
                                       for kind, limit in (("io", 50), ("memory", 10)))
                        pressure_since = (pressure_since or now) if stressed else None
                        if pressure_since and now - pressure_since >= 10:
                            raise StopBenchmark(f"sustained excessive storage/memory pressure: {psi}")

                    with tempfile.TemporaryDirectory(prefix="tcopy-phase5-src-", dir=selected[0]) as src_owned, \
                         tempfile.TemporaryDirectory(prefix="tcopy-phase5-dst-", dir=selected[1]) as dst_owned:
                        source = Path(src_owned) / "source"
                        root = Path(dst_owned)
                        for scenario, file_size in (("dense", size), ("metadata-only", 0)):
                            if source.exists():
                                shutil.rmtree(source)
                            dataset = make_source(source, count, file_size, guard)
                            dataset.update(pair=pair_index, workload=name, scenario=scenario, files=count,
                                           bytes=count * file_size)
                            result["datasets"].append(dataset)
                            expected = checked_manifest(source)
                            files = sorted(source.iterdir())
                            modes = ["storage", "warm"] if scenario == "dense" else ["metadata"]
                            if scenario == "dense" and count * size > memory_available() // 2:
                                modes.remove("warm")
                                result["omitted"].append(f"pair {pair_index}, {name}: warm cache unsupported; source exceeds half available RAM")
                            for mode in modes:
                                for run in range(args.runs):
                                    order = list(commands)
                                    rng.shuffle(order)
                                    for position, tool in enumerate(order):
                                        guard()
                                        cache = prepare_cache(files, mode) if mode != "metadata" else {
                                            "mode": "metadata", "status": "uncontrolled", "reason": "empty files; dentry/inode caches uncontrolled"}
                                        sample = {"pair": pair_index, "workload": name, "scenario": scenario,
                                                  "run": run + 1, "order": position, "tool": tool, "cache": cache,
                                                  "status": "pending"}
                                        result["samples"].append(sample)
                                        if cache["status"] == "uncontrolled" and mode != "metadata":
                                            sample["status"] = "unsupported-cache"
                                            continue
                                        destination = root / "destination"
                                        destination.mkdir()
                                        command = [*commands[tool], str(source), str(destination)]
                                        sample["command"] = command
                                        before = block_counters(devices)
                                        metrics, stdout = run_tool(command, root, guard)
                                        sync_started = time.perf_counter()
                                        sync_cpu = time.process_time()
                                        durable_tree(destination)
                                        metrics["sync_cpu_seconds"] = time.process_time() - sync_cpu
                                        metrics["sync_seconds"] = time.perf_counter() - sync_started
                                        after = block_counters(devices)
                                        sample.update(metrics)
                                        sample["device_io_including_other_activity"] = {device: {
                                            field: after[device][field] - before[device][field]
                                            for field in before[device]} for device in devices}
                                        sample["seconds"] = sample["copy_seconds"] + sample["sync_seconds"]
                                        sample["throughput_mib_s"] = count * file_size / MIB / sample["seconds"] if file_size else None
                                        sample["files_per_second"] = count / sample["seconds"]
                                        if tool.startswith("tcopy"):
                                            telemetry = json.loads(stdout)
                                            if telemetry["failures"] or telemetry["copied"] != count or telemetry["transferred_bytes"] != count * file_size:
                                                raise StopBenchmark(f"invalid TurboCopy counters: {telemetry}")
                                            sample["tcopy"] = telemetry
                                        started = time.perf_counter()
                                        if checked_manifest(destination / "source") != expected:
                                            raise StopBenchmark(f"content/metadata verification failed: {tool}")
                                        sample["verification_seconds"] = time.perf_counter() - started
                                        if any(p.stat().st_blocks * 512 < file_size for p in (destination / "source").iterdir()):
                                            raise StopBenchmark(f"destination sparse/compressed: {tool}")
                                        sample["status"] = "verified"
                                        shutil.rmtree(destination)
                                        rate = (f"{sample['throughput_mib_s']:.1f} MiB/s" if file_size else
                                                f"{sample['files_per_second']:.1f} files/s")
                                        print(f"pair={pair_index} {name} {scenario}/{mode} run={run+1} "
                                              f"{tool}: {sample['seconds']:.3f}s {rate} verified", flush=True)
                                        write_report(result, args.output)
            result["status"] = "completed"
    except (Exception, KeyboardInterrupt) as error:
        result["status"] = "stopped"
        result["stop_reason"] = str(error) or "interrupted"
        if result["samples"] and result["samples"][-1]["status"] == "pending":
            result["samples"][-1]["status"] = "aborted"
        print(f"STOPPED: {result['stop_reason']}", flush=True)
    finally:
        result["finished_utc"] = datetime.now(timezone.utc).isoformat()
        write_report(result, args.output)
    return 0 if result["status"] in ("completed", "inventory-only") else 1


if __name__ == "__main__":
    raise SystemExit(main())
