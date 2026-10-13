# TurboCopy Phase 5: large datasets and parallel throughput

Status: **completed**. 

Recorded 2026-10-13T01:58:44.131025+00:00; source revision `e9b928931f813da91d66865946f1e1d22896329c`.

Verified 96 dense copies and 96 empty-file companion copies. The copy engine was unchanged.

## Storage and safety

Inventory, commands, binary hash, individual samples and cache checks are in [benchmark-phase5-20261013T015836Z.json](benchmark-phase5-20261013T015836Z.json). Only owned temporary trees are removed. No storage configuration or global caches are changed. One source and one destination are retained at a time. Free-space reserves, free inodes, available RAM and PSI are checked before and during execution; failures stop the benchmark and retain results.

| Parent | Filesystem / mount | Physical storage | Initial free GiB |
| --- | --- | --- | ---: |
| `/home/ewave/projects/turboCopy` | btrfs / `/home` | nvme0n1 (HFM001TD3JX013N) | 545.59 |

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

![one-10gib, pair 0, storage throughput](benchmark-phase5-20261013T015836Z-0-storage-0.svg)

![ten-1gib, pair 0, storage throughput](benchmark-phase5-20261013T015836Z-0-storage-1.svg)

![hundred-100mib, pair 0, storage throughput](benchmark-phase5-20261013T015836Z-0-storage-2.svg)

![ten-thousand-1mib, pair 0, storage throughput](benchmark-phase5-20261013T015836Z-0-storage-3.svg)

| Pair | Workload | Cache | Tool | n | Durable MiB/s | Copy s | Total s | Final sync s | CPU % |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | one-10gib | storage | GNU cp -a | 3 | 683.6 | 12.399 | 14.979 | 1.311 | 58.7 |
| 0 | one-10gib | storage | rsync -a | 3 | 621.0 | 15.410 | 16.490 | 1.204 | 91.9 |
| 0 | one-10gib | storage | tcopy auto | 3 | 659.9 | 15.505 | 15.517 | 0.011 | 45.5 |
| 0 | one-10gib | storage | tcopy workers 1 | 3 | 656.6 | 15.584 | 15.596 | 0.011 | 44.7 |
| 0 | one-10gib | storage | tcopy workers 2 | 3 | 651.5 | 15.707 | 15.718 | 0.011 | 44.6 |
| 0 | one-10gib | storage | tcopy workers 4 | 3 | 652.8 | 15.683 | 15.685 | 0.011 | 44.9 |
| 0 | one-10gib | storage | tcopy workers 8 | 3 | 642.3 | 15.932 | 15.943 | 0.011 | 42.6 |
| 0 | one-10gib | storage | tcopy workers 16 | 3 | 653.4 | 15.662 | 15.672 | 0.011 | 44.8 |
| 0 | ten-1gib | storage | GNU cp -a | 3 | 609.5 | 14.394 | 16.800 | 2.406 | 45.0 |
| 0 | ten-1gib | storage | rsync -a | 3 | 598.1 | 16.029 | 17.121 | 1.213 | 82.9 |
| 0 | ten-1gib | storage | tcopy auto | 3 | 636.8 | 16.048 | 16.080 | 0.031 | 61.9 |
| 0 | ten-1gib | storage | tcopy workers 1 | 3 | 629.1 | 16.235 | 16.278 | 0.043 | 56.0 |
| 0 | ten-1gib | storage | tcopy workers 2 | 3 | 653.2 | 15.637 | 15.678 | 0.041 | 56.9 |
| 0 | ten-1gib | storage | tcopy workers 4 | 3 | 594.7 | 17.088 | 17.218 | 0.030 | 62.6 |
| 0 | ten-1gib | storage | tcopy workers 8 | 3 | 680.6 | 14.994 | 15.045 | 0.048 | 73.8 |
| 0 | ten-1gib | storage | tcopy workers 16 | 3 | 609.2 | 16.803 | 16.810 | 0.042 | 60.3 |
| 0 | hundred-100mib | storage | GNU cp -a | 3 | 592.0 | 14.367 | 16.893 | 1.718 | 42.6 |
| 0 | hundred-100mib | storage | rsync -a | 3 | 554.9 | 16.921 | 18.022 | 1.441 | 76.6 |
| 0 | hundred-100mib | storage | tcopy auto | 3 | 658.7 | 14.920 | 15.181 | 0.261 | 81.3 |
| 0 | hundred-100mib | storage | tcopy workers 1 | 3 | 578.9 | 17.116 | 17.274 | 0.179 | 50.0 |
| 0 | hundred-100mib | storage | tcopy workers 2 | 3 | 600.5 | 16.475 | 16.653 | 0.179 | 59.3 |
| 0 | hundred-100mib | storage | tcopy workers 4 | 3 | 631.4 | 15.786 | 15.838 | 0.052 | 72.3 |
| 0 | hundred-100mib | storage | tcopy workers 8 | 3 | 625.4 | 15.637 | 15.989 | 0.368 | 78.1 |
| 0 | hundred-100mib | storage | tcopy workers 16 | 3 | 666.1 | 14.617 | 15.012 | 0.395 | 117.4 |
| 0 | ten-thousand-1mib | storage | GNU cp -a | 3 | 352.3 | 17.899 | 28.384 | 10.215 | 36.4 |
| 0 | ten-thousand-1mib | storage | rsync -a | 3 | 275.4 | 20.172 | 36.306 | 15.579 | 62.8 |
| 0 | ten-thousand-1mib | storage | tcopy auto | 3 | 216.4 | 45.110 | 46.216 | 1.050 | 30.2 |
| 0 | ten-thousand-1mib | storage | tcopy workers 1 | 3 | 130.4 | 75.609 | 76.682 | 1.073 | 16.2 |
| 0 | ten-thousand-1mib | storage | tcopy workers 2 | 3 | 151.2 | 65.124 | 66.134 | 1.038 | 19.8 |
| 0 | ten-thousand-1mib | storage | tcopy workers 4 | 3 | 242.5 | 40.208 | 41.238 | 1.040 | 35.6 |
| 0 | ten-thousand-1mib | storage | tcopy workers 8 | 3 | 385.7 | 24.801 | 25.929 | 1.117 | 69.6 |
| 0 | ten-thousand-1mib | storage | tcopy workers 16 | 3 | 492.2 | 19.017 | 20.318 | 1.141 | 128.1 |

## Parallelism analysis

- Pair 0, one-10gib, storage: best parallel setting is **tcopy workers 16**, 1.00× workers 1. Observed ranges overlap or parallelism is slower; no clear gain beyond noise.
- Pair 0, ten-1gib, storage: best parallel setting is **tcopy workers 8**, 1.08× workers 1. Observed ranges overlap or parallelism is slower; no clear gain beyond noise.
- Pair 0, hundred-100mib, storage: best parallel setting is **tcopy workers 16**, 1.15× workers 1. Its entire observed range exceeds workers 1.
- Pair 0, ten-thousand-1mib, storage: best parallel setting is **tcopy workers 16**, 3.77× workers 1. Its entire observed range exceeds workers 1.

One dense file exposes no file-level parallelism in manual worker mode without chunks. More files can occupy more workers; additional workers can also increase same-device contention and per-file fsync costs. Treat these as explanations to check against recorded worker/chunk telemetry, not performance claims for untested storage.

Observed automatic-mode initial → final worker limits: **4 → 4** across 12 verified dense copies. Manual workers retain their limits. The 10,000-file runs spend most of their summed worker time in metadata/sync work, so overlapping those waits produces the largest measured parallel gain. Observed ranges are not confidence intervals.

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
| 0 | ten-thousand-1mib | storage/3 | 4 → 4 | btrfs/nvme (topology complete) / btrfs/nvme (topology complete) | yes |

## Read/write accounting

Medians in GiB. Device totals cover all backing devices and include other host activity; process totals cover the tool and supervisor, ending before the final external fsync.

| Pair/cache | Workload | Tool | Process read GiB | Process write GiB | Device read GiB | Device write GiB |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| 0/storage | one-10gib | GNU cp -a | 10.000 | 10.000 | 10.000 | 10.026 |
| 0/storage | one-10gib | rsync -a | 10.000 | 10.000 | 10.000 | 10.036 |
| 0/storage | one-10gib | tcopy auto | 10.000 | 10.010 | 10.000 | 10.040 |
| 0/storage | one-10gib | tcopy workers 1 | 10.000 | 10.003 | 10.000 | 10.036 |
| 0/storage | one-10gib | tcopy workers 2 | 10.000 | 10.010 | 10.000 | 10.054 |
| 0/storage | one-10gib | tcopy workers 4 | 10.000 | 10.006 | 10.000 | 10.029 |
| 0/storage | one-10gib | tcopy workers 8 | 10.000 | 10.009 | 10.000 | 10.042 |
| 0/storage | one-10gib | tcopy workers 16 | 10.000 | 10.005 | 10.000 | 10.037 |
| 0/storage | ten-1gib | GNU cp -a | 10.000 | 10.000 | 10.000 | 10.038 |
| 0/storage | ten-1gib | rsync -a | 10.000 | 10.000 | 10.000 | 10.057 |
| 0/storage | ten-1gib | tcopy auto | 10.000 | 10.011 | 10.000 | 10.056 |
| 0/storage | ten-1gib | tcopy workers 1 | 10.000 | 10.014 | 10.000 | 10.041 |
| 0/storage | ten-1gib | tcopy workers 2 | 10.000 | 10.014 | 10.000 | 10.032 |
| 0/storage | ten-1gib | tcopy workers 4 | 10.000 | 10.014 | 10.000 | 10.051 |
| 0/storage | ten-1gib | tcopy workers 8 | 10.000 | 10.014 | 10.000 | 10.037 |
| 0/storage | ten-1gib | tcopy workers 16 | 10.000 | 10.015 | 10.000 | 10.052 |
| 0/storage | hundred-100mib | GNU cp -a | 9.766 | 9.766 | 9.766 | 9.821 |
| 0/storage | hundred-100mib | rsync -a | 9.766 | 9.766 | 9.766 | 9.802 |
| 0/storage | hundred-100mib | tcopy auto | 9.766 | 9.785 | 9.766 | 9.834 |
| 0/storage | hundred-100mib | tcopy workers 1 | 9.766 | 9.796 | 9.766 | 9.849 |
| 0/storage | hundred-100mib | tcopy workers 2 | 9.766 | 9.795 | 9.766 | 9.847 |
| 0/storage | hundred-100mib | tcopy workers 4 | 9.766 | 9.787 | 9.766 | 9.836 |
| 0/storage | hundred-100mib | tcopy workers 8 | 9.766 | 9.783 | 9.766 | 9.828 |
| 0/storage | hundred-100mib | tcopy workers 16 | 9.766 | 9.781 | 9.766 | 9.819 |
| 0/storage | ten-thousand-1mib | GNU cp -a | 9.766 | 9.770 | 9.766 | 10.279 |
| 0/storage | ten-thousand-1mib | rsync -a | 9.766 | 9.771 | 9.766 | 10.404 |
| 0/storage | ten-thousand-1mib | tcopy auto | 9.766 | 10.686 | 9.766 | 11.673 |
| 0/storage | ten-thousand-1mib | tcopy workers 1 | 9.766 | 11.426 | 9.766 | 13.190 |
| 0/storage | ten-thousand-1mib | tcopy workers 2 | 9.766 | 11.281 | 9.766 | 12.901 |
| 0/storage | ten-thousand-1mib | tcopy workers 4 | 9.766 | 10.711 | 9.766 | 11.722 |
| 0/storage | ten-thousand-1mib | tcopy workers 8 | 9.766 | 10.339 | 9.766 | 10.960 |
| 0/storage | ten-thousand-1mib | tcopy workers 16 | 9.766 | 10.116 | 9.766 | 10.502 |

## Creation and metadata overhead

| Pair | Workload | Tool | Median empty-tree s | Files/s | Final sync s |
| --- | --- | --- | ---: | ---: | ---: |
| 0 | one-10gib | GNU cp -a | 0.012 | 82.9 | 0.011 |
| 0 | one-10gib | tcopy workers 16 | 0.027 | 36.5 | 0.011 |
| 0 | one-10gib | tcopy workers 8 | 0.027 | 37.0 | 0.011 |
| 0 | one-10gib | tcopy workers 2 | 0.027 | 37.2 | 0.011 |
| 0 | one-10gib | tcopy auto | 0.027 | 37.1 | 0.011 |
| 0 | one-10gib | tcopy workers 1 | 0.027 | 37.3 | 0.011 |
| 0 | one-10gib | rsync -a | 0.057 | 17.5 | 0.012 |
| 0 | one-10gib | tcopy workers 4 | 0.027 | 36.8 | 0.011 |
| 0 | ten-1gib | tcopy workers 1 | 0.021 | 477.9 | 0.007 |
| 0 | ten-1gib | rsync -a | 0.054 | 184.5 | 0.009 |
| 0 | ten-1gib | tcopy workers 16 | 0.013 | 773.6 | 0.007 |
| 0 | ten-1gib | tcopy workers 4 | 0.047 | 212.4 | 0.007 |
| 0 | ten-1gib | tcopy workers 2 | 0.020 | 492.9 | 0.007 |
| 0 | ten-1gib | GNU cp -a | 0.008 | 1187.5 | 0.007 |
| 0 | ten-1gib | tcopy workers 8 | 0.015 | 661.8 | 0.008 |
| 0 | ten-1gib | tcopy auto | 0.017 | 605.8 | 0.007 |
| 0 | hundred-100mib | tcopy workers 16 | 0.477 | 209.5 | 0.366 |
| 0 | hundred-100mib | tcopy workers 1 | 1.105 | 90.5 | 0.372 |
| 0 | hundred-100mib | rsync -a | 0.421 | 237.7 | 0.372 |
| 0 | hundred-100mib | tcopy auto | 0.733 | 136.4 | 0.371 |
| 0 | hundred-100mib | GNU cp -a | 0.377 | 265.6 | 0.371 |
| 0 | hundred-100mib | tcopy workers 4 | 0.730 | 137.1 | 0.370 |
| 0 | hundred-100mib | tcopy workers 2 | 1.078 | 92.8 | 0.372 |
| 0 | hundred-100mib | tcopy workers 8 | 0.562 | 178.0 | 0.367 |
| 0 | ten-thousand-1mib | rsync -a | 12.354 | 809.4 | 12.032 |
| 0 | ten-thousand-1mib | tcopy workers 8 | 23.349 | 428.3 | 14.827 |
| 0 | ten-thousand-1mib | tcopy workers 4 | 29.753 | 336.1 | 7.533 |
| 0 | ten-thousand-1mib | tcopy workers 1 | 59.163 | 169.0 | 5.603 |
| 0 | ten-thousand-1mib | tcopy workers 2 | 61.575 | 162.4 | 3.739 |
| 0 | ten-thousand-1mib | tcopy workers 16 | 22.788 | 438.8 | 14.828 |
| 0 | ten-thousand-1mib | GNU cp -a | 11.623 | 860.4 | 11.388 |
| 0 | ten-thousand-1mib | tcopy auto | 28.176 | 354.9 | 5.745 |

Source generation, file creation, metadata setup and sync times are recorded separately in `datasets`; generation time includes randomness and pressure checks. Creation measures file open only; these setup timers do not include every setup operation.

## Unsupported and uncontrolled conditions

- pair 0, one-10gib: warm cache unsupported; source exceeds half available RAM
- pair 0, ten-1gib: warm cache unsupported; source exceeds half available RAM
- pair 0, hundred-100mib: warm cache unsupported; source exceeds half available RAM
- pair 0, ten-thousand-1mib: warm cache unsupported; source exceeds half available RAM

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
