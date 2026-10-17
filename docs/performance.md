# Benchmark methodology and results

The latest [Task 3 worker-selection results](automatic-workers.md) include a
verified parent-commit baseline and repeated before/after comparisons. On the
measured shared Btrfs/NVMe device, 4,096 × 1 MiB improved from 209.3 to
567.1 MiB/s median; this is not a universal speed claim. Older measurements below
use different dataset sizes and sometimes an additional final sync, so do not
pool their rates with the Task 3 matrix.

The original Phase 1–5 measurements predate the removal of PSI-based admission
throttling and automatic GPFS detection. The Phase 5 follow-up and
[current rebenchmark](#phase-5-current-automatic-mode-rebenchmark) below measure
the revised policy. Current CLI defaults select storage automatically
and retain the selected admission limit unless explicit tuning is requested.
Historical automatic-mode slowdowns remain recorded for comparison. See [GPFS guide](gpfs.md) for cluster verification.

Run a release binary with Python 3.11+:

```bash
cargo build --release
python3 benches/compare.py --binary target/release/tcopy --directory . \
  --runs 3 --output /tmp/tcopy-benchmark.json
```

The harness owns a temporary directory under the selected filesystem and deletes
only that dataset afterward. Its default dataset has 1,000 random 4 KiB files,
16 files of 1 MiB, one 64 MiB file, and one empty directory: 1,017 files totaling
83.90625 MiB. Larger files repeat a seeded random block; note this when testing
compressed/deduplicated storage. Override `--small-files`, `--medium-files` and `--large-mib` to
test other mixes. Select source/destination storage pairs in future benchmark
work; this harness measures one filesystem per run.

Every tool receives the same source basename and destination layout. Commands:

- `cp -rp --reflink=never --sparse=never SOURCE DESTINATION`
- `rsync -rltp SOURCE DESTINATION`
- `tcopy SOURCE DESTINATION`
- `tcopy --sync SOURCE DESTINATION`
- `tcopy --workers=4 --chunk-size=8M SOURCE DESTINATION`
- `tcopy --workers=4 --chunk-size=8M --sync SOURCE DESTINATION`
- `tcopy --auto SOURCE DESTINATION`
- `tcopy --auto --sync SOURCE DESTINATION`
- `tcopy --tune SOURCE DESTINATION`
- `tcopy --tune --sync SOURCE DESTINATION`

Phase 2 TurboCopy commands also use `--json` to retain engine/concurrency
telemetry; the harness validates these summaries.

All paths preserve permissions and mtimes. Ownership is identical for this
single-user dataset; ACLs, extended attributes and hard links are not included.
Reflinks and sparse expansion shortcuts are disabled for `cp`. Adaptive
configurations retain calibration, hardware and workload telemetry in each sample.

Read the source before each sample to measure **warm-cache** runs. The harness
never drops global caches or issues destructive device writes. Randomize tool
order with a fixed seed, collect three samples per scenario/tool, and report
medians. Separate first-copy from unchanged-incremental runs. Default `tcopy`
and `cp` recopy; `tcopy --sync` skips by size, while `rsync` uses its ordinary
size/time quick check. Changed incremental data, same-size modifications and
cold-cache runs require separate experiments.

The clock covers tool execution plus a final file/directory fsync of the entire
destination for every tool, including skipped files. This supplies comparable
end-of-run durability but also charges incremental copies for flushing the
whole tree. TurboCopy additionally fsyncs every file before publication and each
parent after rename; its per-file atomicity and intermediate durability are
stronger than these baseline commands. This difference is deliberate and must
remain visible in any comparison. No instantaneous or copied-byte throughput is
inferred from an unchanged-incremental run.

Outside the timed interval, verify SHA-256 contents, permission bits and
nanosecond mtimes for every file and directory. Output retains individual
samples, commands, tool/compiler versions, filesystem and host information.

## Phase 1 measurement

Results are recorded in [benchmark-phase1.json](benchmark-phase1.json).
On 2026-10-11, Linux x86-64 with eight logical CPUs, Rust 1.95.0, and the
project's Btrfs filesystem, the final release binary produced these medians:

| Tool | First copy (seconds) | Unchanged incremental (seconds) |
| --- | ---: | ---: |
| `tcopy` | 1.886 | 1.926 |
| `tcopy --sync` | 1.875 | 0.968 |
| `cp` | 1.324 | 1.119 |
| `rsync` | 1.447 | 0.969 |

All 24 benchmark samples passed the content and metadata checks. TurboCopy
was slower on the initial copy in this workload. Size-based sync skipped all
1,017 files on unchanged incremental runs; default copying transferred all
87,982,080 logical bytes again. Timings include the common durability flush
described above and are not isolated transfer throughput measurements.

The measurement is a small local regression baseline, not evidence of optimal
throughput or a general speed advantage. HDD, separate NVMe devices, NFS,
large-scale metadata workloads and cold-cache performance remain unmeasured.

Per-file syncing and post-order directory barriers are the main intentional
tradeoffs. They make failures and metadata ordering straightforward, at a cost
to throughput and parallelism. Phase 2 retains these safeguards while measuring
chunk and syscall alternatives.

## Phase 2 measurement

[benchmark-phase2.json](benchmark-phase2.json) records three samples per tool
and scenario on the same local Btrfs filesystem, eight logical CPUs and Rust
1.95.0. Benchmarks ran sequentially without concurrent test/build workloads.
All 36 samples passed SHA-256, mode, nanosecond-mtime and summary checks.

| Tool | First copy (seconds) | Unchanged incremental (seconds) |
| --- | ---: | ---: |
| `tcopy` | 1.618 | 1.743 |
| `tcopy --sync` | 1.581 | 0.809 |
| `tcopy chunks` | 1.614 | 1.597 |
| `tcopy chunks --sync` | 1.602 | 0.796 |
| `cp` | 1.094 | 0.966 |
| `rsync` | 1.191 | 0.876 |

The mixed-file workload recorded **zero chunked files**, including the explicit
chunk configurations: all I/O permits were occupied when the large file started.
Their timings measure the buffered fallback, not parallel range copying. This
is why the separate single-large-file measurement below matters. Default-copy
performance remains below `cp`/`rsync` for this dataset; differences from Phase 1
also reflect run-to-run cache/system conditions and do not establish a speedup.
Directory barriers and durability calls remain deliberate throughput costs.

Storage detection reports Btrfs and shared filesystem/storage correctly here.
Physical backing-device access is restricted in this sandbox, so its SSD/NVMe/HDD
classification is unknown. On Btrfs, a mount's superblock major/minor may differ
from the subvolume's `st_dev`; the detector verifies the mount-point stat device
before accepting that case. Multi-device Btrfs membership is not fully discovered.

## Single-large-file measurement

[benchmark-large-phase2.json](benchmark-large-phase2.json) isolates a 256 MiB
file and an empty directory, with no small/medium files. All 36 samples passed
content/metadata/summary checks. Every explicit-chunk transfer recorded one
chunked file and four peak active slots; sync incremental runs transferred zero.
The same warm-cache and common durability flush apply.

| Tool | First copy (seconds) | Unchanged incremental (seconds) |
| --- | ---: | ---: |
| `tcopy` | 0.225 | 0.241 |
| `tcopy --sync` | 0.222 | 0.005 |
| `tcopy chunks` | 0.227 | 0.249 |
| `tcopy chunks --sync` | 0.222 | 0.005 |
| `cp` | 0.208 | 0.246 |
| `rsync` | 0.329 | 0.045 |

Chunking did not establish an improvement on this storage pair: the ordinary
buffered and chunked first-copy medians were essentially equal. Chunking remains
an explicit capability for other workloads/devices, rather than the default.
A separate 1 GiB + 17-byte smoke copy confirmed live progress while a file was
still transferring, four active slots and exact final byte counts.

```bash
python3 benches/compare.py --directory . --runs 3 --small-files 0 \
  --medium-files 0 --large-mib 256 --output /tmp/tcopy-large-benchmark.json
```

## Copy-syscall experiment

[benchmark-io-phase2.json](benchmark-io-phase2.json) records 12 verified samples
on a single 128 MiB file plus a 17-byte partial tail, three per method. Source
contents repeat a seeded random 1 MiB block. This is a warm-cache, same-filesystem
experiment with a 128 KiB transfer buffer/request. Timing includes destination
creation, transfer, mode/mtime preservation, file fsync and parent-directory fsync;
SHA-256/metadata checks and cleanup happen afterward. Method order is randomized.
Unsupported syscalls are reported with errno and excluded from the median; real
I/O/space failures stop the experiment. These are Python loops, not Rust-engine
or cold-device measurements. Positioned I/O in this script is single-threaded.

| Method | Median seconds |
| --- | ---: |
| `read_write` | 0.106014 |
| `pread_pwrite` | 0.108636 |
| `sendfile` | 0.111832 |
| `copy_file_range` | 0.009132 |

`copy_file_range` can use filesystem offload/reflinks on Btrfs; its shorter time
is not evidence of faster physical copying. Production TurboCopy continues to
use buffered or explicitly requested positioned I/O, with atomic publication.
The ordinary transfer methods are close enough here that three warm-cache
samples do not justify choosing another production syscall.

```bash
python3 benches/io_methods.py --directory . --runs 3 \
  --output /tmp/tcopy-io-benchmark.json
```

HDD, independent NVMe pairs, network filesystems, cold-cache behavior, real device
exhaustion and Rust 1.75/AArch64 execution remain unmeasured. The test suite covers
partial/interrupted buffered I/O, positioned EOF/ENOSPC, kernel write-limit
rollback, cancellation and changing-source/parent-swap races in both copy modes.

## Phase 3 measurement

[benchmark-phase3.json](benchmark-phase3.json) records 60 verified samples: three
per tool/scenario on the same local Btrfs filesystem, eight logical CPUs, four
physical cores and Rust 1.95.0. The warm-cache, content/metadata and common final
fsync methodology above remains unchanged. No benchmark ran alongside a build
or test workload.

| Tool | First copy (seconds) | Unchanged incremental (seconds) |
| --- | ---: | ---: |
| `tcopy` | 1.767 | 1.716 |
| `tcopy --sync` | 1.682 | 0.839 |
| `tcopy chunks` | 1.639 | 1.696 |
| `tcopy chunks --sync` | 1.645 | 0.802 |
| `tcopy --auto` | 2.263 | 2.433 |
| `tcopy --auto --sync` | 2.230 | 0.820 |
| `tcopy --tune` | 2.647 | 2.703 |
| `tcopy --tune --sync` | 2.578 | 0.817 |
| `cp` | 1.145 | 0.932 |
| `rsync` | 1.213 | 0.904 |

Backing devices remain unknown in this sandbox, so automatic selection used a
two-slot ceiling and buffered copying. Default copying used four slots. The
conservative automatic and tuning modes were slower here. No speedup or optimal
configuration is established. Worker selection intentionally does not infer
SSD/NVMe classification from an inaccessible device or from these warm-cache
results. All samples classified the workload as 1,000 small, 16 medium and one
large file; no automatic file was chunked.

Most initial tune samples lasted less than two seconds inside the engine. They
measured one window at one slot, tried two, then retained the observed one-slot
setting with `short-workload`; two recorded adjustments include the final
restoration. Unchanged-sync runs transferred zero bytes and made no tuning
measurements. Some samples encountered I/O PSI above the policy threshold and
retained/reduced admission to one slot. Startup pressure selection is not counted
as a runtime adjustment. Other-process/storage pressure and filesystem cleanup
are not controlled; a late rsync sample took nearly four seconds, illustrating
why individual samples and medians are both retained.

[benchmark-large-phase3.json](benchmark-large-phase3.json) records another 60
verified samples on a single 256 MiB file.

| Tool | First copy (seconds) | Unchanged incremental (seconds) |
| --- | ---: | ---: |
| `tcopy` | 0.221 | 0.223 |
| `tcopy --sync` | 0.225 | 0.007 |
| `tcopy chunks` | 0.231 | 0.228 |
| `tcopy chunks --sync` | 0.224 | 0.006 |
| `tcopy auto` | 0.221 | 0.221 |
| `tcopy auto --sync` | 0.233 | 0.006 |
| `tcopy tune` | 0.227 | 0.247 |
| `tcopy tune --sync` | 0.225 | 0.006 |
| `cp` | 0.214 | 0.238 |
| `rsync` | 0.320 | 0.046 |

Automatic copies used one active buffered slot; topology was unknown, so they
correctly refused automatic chunks. Explicit chunk transfers used four peak
slots and one chunked file. Tune copies completed before the first measurement
window and reported `insufficient-work` with zero samples. Differences among
buffered/chunk/auto/tune configurations do not establish a large-file advantage.

[calibration-phase3.json](calibration-phase3.json) records a separate longer smoke
copy of 3,000 seeded random 4 KiB files and an unchanged sync pass. The first
copy took 4.601 seconds inside the engine, measured the one- and two-slot settings,
completed calibration at two slots and recorded one admission adjustment. Its
unchanged-sync pass skipped all 3,000 files, transferred zero bytes and made zero
measurements. Both verified SHA-256, permissions and nanosecond mtimes, with the
same final durability flush. This is runtime evidence of calibration completing,
not a comparison or proof that its chosen setting is optimal.

```bash
python3 benches/compare.py --directory . --output docs/benchmark-phase3.json
python3 benches/compare.py --directory . --runs 3 --small-files 0 \
  --medium-files 0 --large-mib 256 --output docs/benchmark-large-phase3.json
```

The 42-test suite includes deterministic selection/calibration/pressure histories,
CPU/RAM/cgroup parsing, live permit shrink/grow checks, byte-exact adaptive copies
and tuned-mode cancellation, source-change, parent-swap and write-failure
regressions. Real independent NVMe/HDD pairs, NFS, cold caches, cgroup v1, hidden
cgroup ancestors, real device exhaustion and Rust 1.75/AArch64 execution remain
unmeasured. Persistent tuning caches remain deferred.

## Phase 4 measurement

[benchmark-phase4.json](benchmark-phase4.json) records a warm-cache matrix on
the available Btrfs and tmpfs mounts: four source/destination pairs, three runs
per tool/scenario, 198 verified copies and nine explicitly unsupported GNU `cp`
reflink attempts. Each source has 100 seeded 4 KiB files, a 4 MiB dense file,
an approximately 32 MiB sparse file, an empty directory and a user xattr. The
harness verifies SHA-256 content, mode, nanosecond mtime, xattrs where requested,
sparse allocation, JSON counters and a common final file/directory fsync. It
randomizes tool order and records first-copy and unchanged-incremental samples.
The table shows selected first-copy medians in seconds; full samples and all
incremental medians remain in the JSON artifact.

| Source → destination | `tcopy` | `tcopy --sparse` | `tcopy --reflink --sparse` | GNU `cp -a --sparse=always` | `rsync -aXS` |
| --- | ---: | ---: | ---: | ---: | ---: |
| Btrfs → Btrfs | 0.157 | 0.136 | 0.129 | 0.085 | 0.175 |
| Btrfs → tmpfs | 0.017 | 0.006 | 0.006 | 0.007 | 0.084 |
| tmpfs → Btrfs | 0.148 | 0.138 | 0.137 | 0.091 | 0.183 |
| tmpfs → tmpfs | 0.011 | 0.005 | 0.005 | 0.007 | 0.082 |

The reflink flag falls back to sparse transfer when cloning is unsupported;
GNU `cp --reflink=always` succeeded only on Btrfs → Btrfs (0.071 seconds median).
For this source, `tcopy --sparse` and reflink allocated 8,192 bytes for the
32 MiB logical sparse file, versus 33,558,528 bytes for ordinary `tcopy`.
On Btrfs → Btrfs, unchanged incremental `tcopy --sparse --sync` took 0.067
seconds and content-checking `--checksum` took 0.077 seconds; the latter reads
every byte. These small warm-cache samples do not establish general speed
rankings, especially for cold cache, HDD, NVMe, NFS or device contention.

[benchmark-io-phase4-btrfs.json](benchmark-io-phase4-btrfs.json) and
[benchmark-io-phase4-tmpfs.json](benchmark-io-phase4-tmpfs.json) repeat the
verified 32 MiB syscall experiment (three runs each). Btrfs medians were
0.0289 s buffered read/write, 0.0297 s positioned I/O, 0.0261 s `sendfile`
and 0.00387 s `copy_file_range`; the latter may use filesystem cloning/offload.
On tmpfs, the same methods ranged from 0.00961 to 0.0111 s. The runtime keeps
its buffered/positioned paths by default because the Btrfs offload result does
not imply equivalent physical-copy semantics or portable performance.

```bash
python3 benches/phase4.py --binary target/release/tcopy \
  --directory . --directory /tmp --runs 3 --output docs/benchmark-phase4.json
python3 benches/io_methods.py --directory . --runs 3 \
  --output docs/benchmark-io-phase4-btrfs.json
python3 benches/io_methods.py --directory /tmp --runs 3 \
  --output docs/benchmark-io-phase4-tmpfs.json
```

These directories were the available Btrfs and tmpfs mounts, not controlled
hardware classes. `setfacl` returned `EINVAL` during ACL source setup on this
host, and symlink `user.*` xattrs were unsupported; their conditional tests did
not verify preservation at runtime here. The ownership-failure test runs only
as root. No privileged cache drop or unavailable filesystem mount was attempted.

## Phase 5 measurement

Phase 5 is complete. [benchmark-phase5.md](benchmark-phase5.md) contains the
throughput graphs, full eight-configuration comparison and methodology;
[benchmark-phase5.json](benchmark-phase5.json) retains storage inventory,
individual timings, CPU utilization, read/write bytes and worker telemetry.
The unchanged copy engine was measured on Btrfs over an encrypted NVMe device
on 2026-10-12 UTC, with three randomized iterations per configuration/workload:
96 verified dense copies and 96 empty-file companion copies for separate
creation/metadata overhead measurements.

To repeat the full test with the current release build, run:

```bash
bash tools/benchmark.sh
```

Each run saves `benchmark.json`, `benchmark.md`, SVG graphs, `run.log`, and
`exit-status` in a unique `target/performance/run-*` folder, preserving the
historical reports. An exit status of `0` means the command succeeded; check
the JSON report's `status` is `completed` before using it to update these docs.
The script prints a prompt for a later session to compare and update the results.
This runs the original full workloads and may take as long as the previous test.

The table shows median durable MiB/s, including a common final file/directory
fsync. Sources contain fresh incompressible random data; reflinks and sparse
copies are disabled. Every copy verifies SHA-256, file set, size, ownership,
mode and nanosecond mtime against the same source metadata contract.

| Workload | GNU `cp -a` | `rsync -a` | `tcopy --auto` | Workers 1 | Best parallel setting | Gain over workers 1 |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| 1 × 10 GiB | 708.1 | 558.1 | 634.7 | 654.9 | 16 workers: 657.8 | 1.00× |
| 10 × 1 GiB | 698.3 | 564.6 | 655.7 | 653.2 | 4 workers: 807.2 | 1.24× |
| 100 × 100 MiB | 660.1 | 541.1 | 606.7 | 542.4 | 16 workers: 698.9 | 1.29× |
| 10,000 × 1 MiB | 252.0 | 251.1 | 105.4 | 98.7 | 16 workers: 449.7 | 4.56× |

One file showed no clear parallel gain: without explicit chunks, additional
workers cannot split its transfer. Multiple files benefited, most strongly
at 10,000 files where overlapping metadata/sync waits helped throughput.
More workers were not always faster: four workers gave the highest median
for ten files. Automatic mode reduced its worker limit from four to one in
all 12 dense runs under the existing pressure policy; preparation and
verification I/O can influence that policy's recent pressure window.

These are same-device storage measurements with verified 0% source-page
residency immediately before each dense copy. Device/controller and metadata
caches, other host activity and SSD temperature remain uncontrolled.
Full warm-cache datasets exceeded available memory, and no independent
writable mounted physical-device pair was available for cross-device runs.
The final durability endpoint is common, but TurboCopy's stronger intermediate
per-file sync and atomic publication remain part of its measured tool time.
The report separates copy time, final sync time and empty-file overhead;
three-run observed ranges are not confidence intervals.

The harness uses isolated temporary trees, checks space and storage pressure,
and stops before configured safety thresholds are exceeded. No storage
configuration or user data was changed. Dataset sizes and benchmark parents
are configurable for reproducing the measurements on other hosts:

```bash
cargo build --release
python3 benches/phase5.py --directory . --runs 3 --output docs/benchmark-phase5
```

## Phase 5 follow-up measurement

[benchmark-phase5-20261012T213109Z.md](benchmark-phase5-20261012T213109Z.md) contains the full report and graphs;
[benchmark-phase5-20261012T213109Z.json](benchmark-phase5-20261012T213109Z.json) preserves the raw results from
`run-20261012T213109Z-FHMDaB`. The run started at 2026-10-12 21:31:09 UTC
on the same Btrfs/encrypted-NVMe storage pair, using revision
`dd778d0f70f590674b82162e6fb96bf936f4a8ec`. All 96 dense and 96
empty-file companion copies were verified; the report status is `completed`.
The earlier Phase 5 report above remains the historical baseline.

Median durable MiB/s, with the same common final fsync endpoint:

| Workload | GNU `cp -a` | `rsync -a` | `tcopy --auto` | Workers 1 | Best parallel setting | Gain over workers 1 |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| 1 × 10 GiB | 718.8 | 622.0 | 652.0 | 654.5 | 16 workers: 686.5 | 1.05× |
| 10 × 1 GiB | 667.4 | 603.4 | 749.9 | 630.4 | 16 workers: 770.5 | 1.22× |
| 100 × 100 MiB | 673.5 | 609.0 | 684.8 | 603.2 | 8 workers: 730.3 | 1.21× |
| 10,000 × 1 MiB | 287.7 | 335.1 | 221.8 | 110.6 | 16 workers: 464.6 | 4.20× |

Changes from the original Phase 5 medians (positive means higher throughput):

| Workload | GNU `cp -a` | `rsync -a` | `tcopy --auto` | Workers 1 | Workers 4 | Workers 8 | Workers 16 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 × 10 GiB | +1.5% | +11.4% | +2.7% | -0.1% | +5.7% | -2.9% | +4.4% |
| 10 × 1 GiB | -4.4% | +6.9% | +14.4% | -3.5% | -14.4% | -4.4% | +3.5% |
| 100 × 100 MiB | +2.0% | +12.5% | +12.9% | +11.2% | +4.3% | +8.5% | -2.8% |
| 10,000 × 1 MiB | +14.2% | +33.5% | +110.4% | +12.0% | +4.9% | +5.3% | +3.3% |

Automatic mode retained **4 → 4** workers with zero concurrency adjustments
in all 12 dense copies, compared with **4 → 1** previously. No dense TurboCopy
sample used chunks. At 10,000 files, auto rose from 105.4 to 221.8 MiB/s
(2.10×), close to manual four workers at 224.1 MiB/s, but below both baseline
tools. Sixteen workers reached 464.6 MiB/s, 4.20× workers 1; its observed
303.9–505.1 MiB/s range overlaps rsync’s 270.1–410.8 MiB/s range.
The best median manual setting changed to 16 workers for ten files and eight
workers for 100 files. These three-sample rankings do not establish optimal
settings; the single-file workload still exposes only one active file worker.

Empty-file companion medians for 10,000 files changed from 50.379 to 30.085 s
for auto, 29.485 to 29.724 s for four workers, and 21.690 to 13.100 s for
16 workers. GNU cp changed from 7.086 to 13.738 s and rsync from 13.859 to
1.004 s. The new rsync samples span 0.623–18.022 s, illustrating substantial
variability. These measure creation and metadata durability, not unchanged
incremental copies or data-transfer throughput.

The binaries differ between runs. The retained worker telemetry is consistent
with removal of admission throttling, but this is not a controlled isolation
of that change: available memory and host/cache conditions also differ, and
the baseline tools improved by up to 33.5%. All dense source-residency checks
were 0%; device/controller and metadata caches, host activity and temperature
remain uncontrolled. Warm-cache and cross-device tests remain unsupported.
TurboCopy’s per-file durability and atomic publication costs remain included.

## Phase 5 current automatic-mode rebenchmark

[benchmark-phase5-20261013T015836Z.md](benchmark-phase5-20261013T015836Z.md)
contains the full report, four throughput graphs and automatic-selection table;
[benchmark-phase5-20261013T015836Z.json](benchmark-phase5-20261013T015836Z.json)
is an exact copy of the raw results from `run-20261013T015836Z-8Kq6h4`.
The run started at 2026-10-13 01:58:44 UTC and finished at 03:09:35 UTC.
Both the report status (`completed`) and script exit status (`0`) confirm success:
all 96 dense copies and 96 empty-file companion copies were verified.
The recorded Git revision is `e9b928931f813da91d66865946f1e1d22896329c`;
the JSON also records the binary SHA-256 because a revision alone does not
identify uncommitted working-tree changes.

This repeats the same four workloads, eight configurations and three randomized
iterations on the same Btrfs/encrypted-NVMe storage pair. Every dense source
residency check was **0%** immediately before copying. Median durable MiB/s,
including the same common final fsync endpoint:

| Workload | GNU `cp -a` | `rsync -a` | `tcopy --auto` | Workers 1 | Best manual setting | Gain over workers 1 |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| 1 × 10 GiB | 683.6 | 621.0 | 659.9 | 656.6 | 1 worker: 656.6 | 1.00× |
| 10 × 1 GiB | 609.5 | 598.1 | 636.8 | 629.1 | 8 workers: 680.6 | 1.08× |
| 100 × 100 MiB | 592.0 | 554.9 | 658.7 | 578.9 | 16 workers: 666.1 | 1.15× |
| 10,000 × 1 MiB | 352.3 | 275.4 | 216.4 | 130.4 | 16 workers: 492.2 | 3.77× |

Automatic-mode comparison with both historical Phase 5 runs. Changes use
unrounded medians; ranges are the current run's observed min–max, not confidence
intervals:

| Workload | Original auto MiB/s | Previous follow-up auto MiB/s | Current auto MiB/s (range) | Change vs original | Change vs follow-up |
| --- | ---: | ---: | --- | ---: | ---: |
| 1 × 10 GiB | 634.7 | 652.0 | 659.9 (635.1–669.0) | +4.0% | +1.2% |
| 10 × 1 GiB | 655.7 | 749.9 | 636.8 (623.0–638.6) | -2.9% | -15.1% |
| 100 × 100 MiB | 606.7 | 684.8 | 658.7 (598.4–668.2) | +8.6% | -3.8% |
| 10,000 × 1 MiB | 105.4 | 221.8 | 216.4 (203.4–234.6) | +105.3% | -2.4% |

Automatic mode selected and retained **4 → 4** workers with **zero concurrency
adjustments in all 12 dense copies**. No dense TurboCopy sample used chunks.
The storage classifier was correct for both endpoints in every automatic dense
copy: **Btrfs, NVMe, complete topology, same filesystem and shared storage**.
The reported encrypted mapper matches the mount inventory, whose physical
device and `lsblk` hierarchy identify the non-rotational `nvme0n1`
(HFM001TD3JX013N). Both endpoints report backing device `259:4`.
This validates classification on this storage pair, not on GPFS/NFS or other
untested topologies.

The 10,000-file result preserves the main improvement over the original run:
216.4 versus 105.4 MiB/s (**2.05×**), with the current observed range entirely
above the original 104.6–108.6 MiB/s range. It remains below both current baseline
tools and manual four workers at 242.5 MiB/s. Manual 16 workers reached
492.2 MiB/s, but its 301.6–496.1 MiB/s range overlaps cp's 245.8–434.9 MiB/s
range. These medians do not establish a consistent win or an optimal setting.

There is **no consistent additional automatic-mode gain over the previous
follow-up**. Three of four medians decreased, while the single-file result
increased only 1.2%. The ten-file slowdown has non-overlapping observed ranges
against the follow-up (623.0–638.6 versus 675.0–767.1 MiB/s), but it is not
evidence that a particular code change caused it. Baseline tools also changed:
cp's medians ranged from -12.1% to +22.4% relative to that run, and rsync's from
-17.8% to -0.2%. Initial available memory was about 11.7 GiB, versus 9.1 GiB in
the follow-up and 5.9 GiB originally. Host activity, device/controller caches,
metadata caches and temperature remain uncontrolled.

For 10,000 empty-file companions, median auto time was 28.176 s, versus
30.085 s in the follow-up and 50.379 s originally. Current manual four and
16 workers took 29.753 and 22.788 s; cp and rsync took 11.623 and 12.354 s.
These creation/metadata measurements have uncontrolled metadata caches and are
separate from dense-copy throughput; the full report retains individual samples.

Warm-cache workloads again exceeded half the available RAM, and there was no
independent writable physical-device pair for cross-device testing. GPFS/NFS
remains deferred. The earlier interrupted `benchmark-phase5-current` artifacts
are retained separately and do not contribute samples to this completed run.
