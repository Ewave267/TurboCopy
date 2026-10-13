# TurboCopy Phase 5: large datasets and parallel throughput

Status: **stopped**. signal 15

Recorded 2026-10-13T01:03:09.051985+00:00; source revision `e9b928931f813da91d66865946f1e1d22896329c`.

Verified 84 dense copies and 72 empty-file companion copies. The copy engine was unchanged.

## Storage and safety

Inventory, commands, binary hash, individual samples and cache checks are in [benchmark-phase5-current.json](benchmark-phase5-current.json). Only owned temporary trees are removed. No storage configuration or global caches are changed. One source and one destination are retained at a time. Free-space reserves, free inodes, available RAM and PSI are checked before and during execution; failures stop the benchmark and retain results.

| Parent | Filesystem / mount | Physical storage | Initial free GiB |
| --- | --- | --- | ---: |
| `/home/ewave/projects/turboCopy` | btrfs / `/home/ewave/projects/turboCopy` | nvme0n1 (HFM001TD3JX013N) | 545.59 |

Free space must exceed the dataset working set plus the larger of 20 GiB and 10% of filesystem capacity. The guard stops below 512 MiB available RAM, or after 10 seconds with I/O full PSI avg10 ≥50% or memory full PSI avg10 ≥10%. Existing compression options remain in place; dense random data avoids a compression advantage. Mount options and Btrfs allocation estimates are retained in JSON.

Cross-device storage benchmark unsupported: no independent writable mounted physical-device pair was selected. tmpfs is not physical storage.

## Method

Defaults: 1 × 10 GiB, 10 × 1 GiB, 100 × 100 MiB (9.765625 GiB), 10,000 × 1 MiB (9.765625 GiB). Fresh OS random bytes are written throughout each file, with no repeated blocks or sparse seeks; allocation and sampled zlib ratios are checked. Sources use identical modes, ownership and nanosecond mtimes, with no intentional ACLs, hard links or user xattrs. Every destination is new. SHA-256, file set, size, mode, uid/gid and mtime are verified outside the measured interval. Atime/ctime and security labels are outside the shared preservation contract; rsync -a and existing default TurboCopy do not preserve all of those fields.

GNU `cp -a --reflink=never --sparse=never`, `rsync -a`, `tcopy --auto`, and `tcopy --workers N` for N = 1, 2, 4, 8, 16 are compared. TurboCopy also emits JSON. Order is shuffled with a recorded seed for each of at least three iterations. No reflink, sparse, resume, incremental or explicit chunk options are enabled.

The common durability endpoint is tool completion plus fsync of every destination file and directory, including the containing directory. Total time and throughput include this final barrier, whose time is also recorded separately. TurboCopy's existing per-file sync and atomic publication remain included in its tool time; intermediate failure guarantees are stronger and cannot be equalized without changing the engine. Final metadata and durability requirements are identical. The common final tree fsync is serial, so tools that already flushed in parallel can spend less time in that barrier; copy-only time is reported separately.

Storage runs request POSIX_FADV_DONTNEED on clean source files and require ≤1% source-page residency via mincore immediately before copying. Warm runs read the entire source and require ≥99% residency. Warm runs that cannot fit within half the currently available memory are skipped. Uncontrolled samples never enter the comparison. Metadata, controller/device caches, CPU frequency, SSD temperature, compression mount options and other host activity remain uncontrolled. Storage runs measure verified source page-cache eviction, not a fully cold device.

wait4 captures tool/descendant CPU time; CPU percent uses one core = 100%. A dedicated supervisor reads `/proc/self/io` after waiting for the tool, including waited descendants and a small amount of supervisor I/O. These are process-attributed storage bytes and logical syscall counters, not exact physical traffic; delayed writeback and metadata accounting can differ. Physical device sector deltas cover tool execution plus final sync and include other host I/O. Verification and source preparation are excluded. Empty-file companion copies measure creation/metadata/sync overhead separately; their cache is explicitly uncontrolled and they are not subtracted from dense-copy times. TurboCopy's metadata/transfer timers are sums across concurrent workers, not wall time.

## Measured throughput

Only verified, controlled groups with all requested iterations are graphed. Bars are median durable MiB/s; whiskers are observed min–max across iterations. Workload graphs share a scale within each pair/cache condition. All raw I/O/CPU metrics are in JSON.

![one-10gib, pair 0, storage throughput](benchmark-phase5-current-0-storage-0.svg)

![ten-1gib, pair 0, storage throughput](benchmark-phase5-current-0-storage-1.svg)

![hundred-100mib, pair 0, storage throughput](benchmark-phase5-current-0-storage-2.svg)

| Pair | Workload | Cache | Tool | n | Durable MiB/s | Copy s | Total s | Final sync s | CPU % |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | one-10gib | storage | GNU cp -a | 3 | 698.4 | 13.323 | 14.663 | 1.340 | 51.0 |
| 0 | one-10gib | storage | rsync -a | 3 | 598.2 | 15.696 | 17.118 | 1.146 | 92.5 |
| 0 | one-10gib | storage | tcopy auto | 3 | 670.2 | 15.268 | 15.279 | 0.011 | 46.5 |
| 0 | one-10gib | storage | tcopy workers 1 | 3 | 686.8 | 14.899 | 14.909 | 0.011 | 45.8 |
| 0 | one-10gib | storage | tcopy workers 2 | 3 | 665.8 | 15.368 | 15.380 | 0.011 | 44.9 |
| 0 | one-10gib | storage | tcopy workers 4 | 3 | 646.3 | 15.834 | 15.845 | 0.011 | 42.7 |
| 0 | one-10gib | storage | tcopy workers 8 | 3 | 654.3 | 15.640 | 15.651 | 0.011 | 45.4 |
| 0 | one-10gib | storage | tcopy workers 16 | 3 | 651.9 | 15.696 | 15.707 | 0.011 | 42.3 |
| 0 | ten-1gib | storage | GNU cp -a | 3 | 616.4 | 14.565 | 16.613 | 1.355 | 43.9 |
| 0 | ten-1gib | storage | rsync -a | 3 | 602.5 | 15.846 | 16.996 | 1.150 | 84.1 |
| 0 | ten-1gib | storage | tcopy auto | 3 | 692.9 | 14.732 | 14.779 | 0.029 | 67.1 |
| 0 | ten-1gib | storage | tcopy workers 1 | 3 | 638.4 | 15.997 | 16.040 | 0.043 | 54.6 |
| 0 | ten-1gib | storage | tcopy workers 2 | 3 | 683.6 | 14.937 | 14.979 | 0.043 | 60.7 |
| 0 | ten-1gib | storage | tcopy workers 4 | 3 | 617.1 | 16.578 | 16.594 | 0.043 | 60.6 |
| 0 | ten-1gib | storage | tcopy workers 8 | 3 | 652.6 | 15.645 | 15.692 | 0.047 | 60.3 |
| 0 | ten-1gib | storage | tcopy workers 16 | 3 | 651.5 | 15.670 | 15.718 | 0.044 | 68.3 |
| 0 | hundred-100mib | storage | GNU cp -a | 3 | 605.3 | 14.421 | 16.521 | 1.579 | 44.8 |
| 0 | hundred-100mib | storage | rsync -a | 3 | 551.5 | 16.785 | 18.131 | 1.442 | 79.4 |
| 0 | hundred-100mib | storage | tcopy auto | 3 | 648.3 | 15.082 | 15.425 | 0.343 | 90.5 |
| 0 | hundred-100mib | storage | tcopy workers 1 | 3 | 565.7 | 17.323 | 17.678 | 0.371 | 49.7 |
| 0 | hundred-100mib | storage | tcopy workers 2 | 3 | 595.0 | 16.682 | 16.806 | 0.208 | 59.4 |
| 0 | hundred-100mib | storage | tcopy workers 4 | 3 | 637.5 | 15.649 | 15.685 | 0.176 | 75.6 |
| 0 | hundred-100mib | storage | tcopy workers 8 | 3 | 636.8 | 15.290 | 15.704 | 0.414 | 92.1 |
| 0 | hundred-100mib | storage | tcopy workers 16 | 3 | 610.9 | 16.007 | 16.368 | 0.362 | 93.8 |

## Parallelism analysis

- Pair 0, one-10gib, storage: best parallel setting is **tcopy workers 2**, 0.97× workers 1. Observed ranges overlap or parallelism is slower; no clear gain beyond noise.
- Pair 0, ten-1gib, storage: best parallel setting is **tcopy workers 2**, 1.07× workers 1. Observed ranges overlap or parallelism is slower; no clear gain beyond noise.
- Pair 0, hundred-100mib, storage: best parallel setting is **tcopy workers 4**, 1.13× workers 1. Its entire observed range exceeds workers 1.

One dense file exposes no file-level parallelism in manual worker mode without chunks. More files can occupy more workers; additional workers can also increase same-device contention and per-file fsync costs. Treat these as explanations to check against recorded worker/chunk telemetry, not performance claims for untested storage.

Observed automatic-mode initial → final worker limits: **4 → 4** across 11 verified dense copies. Manual workers retain their limits. The 10,000-file runs spend most of their summed worker time in metadata/sync work, so overlapping those waits produces the largest measured parallel gain. Observed ranges are not confidence intervals.

## Automatic selection and filesystem classification

Filesystem correctness compares reported types with the independent mount inventory. Device kinds and topology completeness are reported separately; matching filesystem types alone does not validate physical storage classification.

| Pair | Workload | Cache/run | Workers initial → final | Source / destination classification | Filesystem types correct |
| --- | --- | --- | --- | --- | --- |
| 0 | one-10gib | storage/1 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |
| 0 | one-10gib | storage/2 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |
| 0 | one-10gib | storage/3 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |
| 0 | ten-1gib | storage/1 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |
| 0 | ten-1gib | storage/2 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |
| 0 | ten-1gib | storage/3 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |
| 0 | hundred-100mib | storage/1 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |
| 0 | hundred-100mib | storage/2 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |
| 0 | hundred-100mib | storage/3 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |
| 0 | ten-thousand-1mib | storage/1 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |
| 0 | ten-thousand-1mib | storage/2 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |

## Read/write accounting

Medians in GiB. Device totals cover all backing devices and include other host activity; process totals cover the tool and supervisor, ending before the final external fsync.

| Pair/cache | Workload | Tool | Process read GiB | Process write GiB | Device read GiB | Device write GiB |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| 0/storage | one-10gib | GNU cp -a | 10.000 | 10.000 | 10.004 | 10.051 |
| 0/storage | one-10gib | rsync -a | 10.000 | 10.000 | 10.002 | 10.034 |
| 0/storage | one-10gib | tcopy auto | 10.000 | 10.013 | 10.001 | 10.048 |
| 0/storage | one-10gib | tcopy workers 1 | 10.000 | 10.005 | 10.001 | 10.045 |
| 0/storage | one-10gib | tcopy workers 2 | 10.000 | 10.010 | 10.002 | 10.046 |
| 0/storage | one-10gib | tcopy workers 4 | 10.000 | 10.010 | 10.000 | 10.048 |
| 0/storage | one-10gib | tcopy workers 8 | 10.000 | 10.010 | 10.006 | 10.032 |
| 0/storage | one-10gib | tcopy workers 16 | 10.000 | 10.011 | 10.003 | 10.065 |
| 0/storage | ten-1gib | GNU cp -a | 10.000 | 10.000 | 10.000 | 10.051 |
| 0/storage | ten-1gib | rsync -a | 10.000 | 10.000 | 10.000 | 10.046 |
| 0/storage | ten-1gib | tcopy auto | 10.000 | 10.013 | 10.001 | 10.060 |
| 0/storage | ten-1gib | tcopy workers 1 | 10.000 | 10.013 | 10.000 | 10.041 |
| 0/storage | ten-1gib | tcopy workers 2 | 10.000 | 10.015 | 10.000 | 10.045 |
| 0/storage | ten-1gib | tcopy workers 4 | 10.000 | 10.013 | 10.000 | 10.056 |
| 0/storage | ten-1gib | tcopy workers 8 | 10.000 | 10.014 | 10.000 | 10.031 |
| 0/storage | ten-1gib | tcopy workers 16 | 10.000 | 10.014 | 10.001 | 10.051 |
| 0/storage | hundred-100mib | GNU cp -a | 9.766 | 9.766 | 9.768 | 9.824 |
| 0/storage | hundred-100mib | rsync -a | 9.766 | 9.766 | 9.767 | 9.817 |
| 0/storage | hundred-100mib | tcopy auto | 9.766 | 9.784 | 9.768 | 9.835 |
| 0/storage | hundred-100mib | tcopy workers 1 | 9.766 | 9.794 | 9.772 | 9.844 |
| 0/storage | hundred-100mib | tcopy workers 2 | 9.766 | 9.793 | 9.767 | 9.843 |
| 0/storage | hundred-100mib | tcopy workers 4 | 9.766 | 9.787 | 9.770 | 9.837 |
| 0/storage | hundred-100mib | tcopy workers 8 | 9.766 | 9.781 | 9.769 | 9.821 |
| 0/storage | hundred-100mib | tcopy workers 16 | 9.766 | 9.781 | 9.769 | 9.826 |

## Creation and metadata overhead

| Pair | Workload | Tool | Median empty-tree s | Files/s | Final sync s |
| --- | --- | --- | ---: | ---: | ---: |
| 0 | one-10gib | GNU cp -a | 0.013 | 77.6 | 0.012 |
| 0 | one-10gib | tcopy workers 16 | 0.028 | 35.7 | 0.011 |
| 0 | one-10gib | tcopy workers 8 | 0.028 | 36.1 | 0.011 |
| 0 | one-10gib | tcopy workers 2 | 0.028 | 36.2 | 0.011 |
| 0 | one-10gib | tcopy auto | 0.028 | 36.2 | 0.011 |
| 0 | one-10gib | tcopy workers 1 | 0.028 | 35.9 | 0.011 |
| 0 | one-10gib | rsync -a | 0.057 | 17.6 | 0.012 |
| 0 | one-10gib | tcopy workers 4 | 0.028 | 36.0 | 0.011 |
| 0 | ten-1gib | tcopy workers 1 | 0.125 | 80.2 | 0.043 |
| 0 | ten-1gib | rsync -a | 0.091 | 110.2 | 0.045 |
| 0 | ten-1gib | tcopy workers 16 | 0.067 | 150.1 | 0.042 |
| 0 | ten-1gib | tcopy workers 4 | 0.094 | 106.7 | 0.043 |
| 0 | ten-1gib | tcopy workers 2 | 0.118 | 84.6 | 0.043 |
| 0 | ten-1gib | GNU cp -a | 0.044 | 225.8 | 0.043 |
| 0 | ten-1gib | tcopy workers 8 | 0.078 | 127.8 | 0.042 |
| 0 | ten-1gib | tcopy auto | 0.091 | 109.5 | 0.043 |
| 0 | hundred-100mib | tcopy workers 16 | 0.456 | 219.2 | 0.350 |
| 0 | hundred-100mib | tcopy workers 1 | 1.177 | 85.0 | 0.383 |
| 0 | hundred-100mib | rsync -a | 0.401 | 249.5 | 0.353 |
| 0 | hundred-100mib | tcopy auto | 0.699 | 143.1 | 0.349 |
| 0 | hundred-100mib | GNU cp -a | 0.357 | 280.4 | 0.353 |
| 0 | hundred-100mib | tcopy workers 4 | 0.697 | 143.5 | 0.348 |
| 0 | hundred-100mib | tcopy workers 2 | 1.088 | 91.9 | 0.366 |
| 0 | hundred-100mib | tcopy workers 8 | 0.532 | 188.0 | 0.345 |

Source generation, file creation, metadata setup and sync times are recorded separately in `datasets`; generation time includes randomness and pressure checks. Creation measures file open only; these setup timers do not include every setup operation.

## Unsupported and uncontrolled conditions

- pair 0, one-10gib: warm cache unsupported; source exceeds half available RAM
- pair 0, ten-1gib: warm cache unsupported; source exceeds half available RAM
- pair 0, hundred-100mib: warm cache unsupported; source exceeds half available RAM
- pair 0, ten-thousand-1mib: warm cache unsupported; source exceeds half available RAM
- Detailed Btrfs per-device allocation is unavailable: mapped /dev nodes are hidden in this execution namespace. The userspace MISSING text reflects that visibility limit; the live mount, capacity estimates and backing NVMe remain visible through the kernel.

## Reproduce

```bash
cargo build --release
python3 benches/phase5.py --directory . --runs 3 --output docs/benchmark-phase5
# Repeat --directory for independently mounted writable physical devices.
# Override defaults with repeatable --dataset NAME:COUNT:BYTES.
# --inventory-only detects storage and checks capacity without making datasets.
python3 -m unittest discover -s benches -p 'test_phase5.py'
```

The JSON records the exact invocation, reserve thresholds and seed. Custom small datasets are validation runs and do not substitute for the default large workloads.

Sources: [GNU cp](https://www.gnu.org/software/coreutils/manual/html_node/cp-invocation.html), [posix_fadvise](https://man7.org/linux/man-pages/man2/posix_fadvise.2.html), [mincore](https://man7.org/linux/man-pages/man2/mincore.2.html), [proc_pid_io](https://man7.org/linux/man-pages/man5/proc_pid_io.5.html).
