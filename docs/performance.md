# Benchmark methodology and results

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
