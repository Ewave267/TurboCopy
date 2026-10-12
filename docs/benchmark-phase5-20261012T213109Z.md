# TurboCopy Phase 5: large datasets and parallel throughput

Status: **completed**. 

Recorded 2026-10-12T21:31:09.311244+00:00; source revision `dd778d0f70f590674b82162e6fb96bf936f4a8ec`.

Verified 96 dense copies and 96 empty-file companion copies. This is a follow-up measurement after the automatic admission policy changed; see [the comparison](performance.md#phase-5-follow-up-measurement).

## Storage and safety

Inventory, commands, binary hash, individual samples and cache checks are in [benchmark-phase5-20261012T213109Z.json](benchmark-phase5-20261012T213109Z.json). Only owned temporary trees are removed. No storage configuration or global caches are changed. One source and one destination are retained at a time. Free-space reserves, free inodes, available RAM and PSI are checked before and during execution; failures stop the benchmark and retain results.

| Parent | Filesystem / mount | Physical storage | Initial free GiB |
| --- | --- | --- | ---: |
| `/home/ewave/projects/turboCopy` | btrfs / `/home` | nvme0n1 (HFM001TD3JX013N) | 545.70 |

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

![one-10gib, pair 0, storage throughput](benchmark-phase5-20261012T213109Z-0-storage-0.svg)

![ten-1gib, pair 0, storage throughput](benchmark-phase5-20261012T213109Z-0-storage-1.svg)

![hundred-100mib, pair 0, storage throughput](benchmark-phase5-20261012T213109Z-0-storage-2.svg)

![ten-thousand-1mib, pair 0, storage throughput](benchmark-phase5-20261012T213109Z-0-storage-3.svg)

| Pair | Workload | Cache | Tool | n | Durable MiB/s | Copy s | Total s | Final sync s | CPU % |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | one-10gib | storage | GNU cp -a | 3 | 718.8 | 13.012 | 14.245 | 1.206 | 52.3 |
| 0 | one-10gib | storage | rsync -a | 3 | 622.0 | 15.462 | 16.463 | 1.028 | 89.3 |
| 0 | one-10gib | storage | tcopy auto | 3 | 652.0 | 15.694 | 15.706 | 0.011 | 43.7 |
| 0 | one-10gib | storage | tcopy workers 1 | 3 | 654.5 | 15.635 | 15.646 | 0.011 | 44.4 |
| 0 | one-10gib | storage | tcopy workers 2 | 3 | 654.7 | 15.631 | 15.642 | 0.011 | 42.7 |
| 0 | one-10gib | storage | tcopy workers 4 | 3 | 666.9 | 15.344 | 15.355 | 0.011 | 45.6 |
| 0 | one-10gib | storage | tcopy workers 8 | 3 | 632.3 | 16.185 | 16.196 | 0.011 | 45.6 |
| 0 | one-10gib | storage | tcopy workers 16 | 3 | 686.5 | 14.905 | 14.916 | 0.011 | 48.4 |
| 0 | ten-1gib | storage | GNU cp -a | 3 | 667.4 | 14.354 | 15.344 | 0.990 | 46.5 |
| 0 | ten-1gib | storage | rsync -a | 3 | 603.4 | 16.041 | 16.971 | 0.930 | 87.6 |
| 0 | ten-1gib | storage | tcopy auto | 3 | 749.9 | 13.612 | 13.655 | 0.041 | 71.0 |
| 0 | ten-1gib | storage | tcopy workers 1 | 3 | 630.4 | 16.195 | 16.243 | 0.047 | 54.5 |
| 0 | ten-1gib | storage | tcopy workers 2 | 3 | 675.7 | 15.102 | 15.154 | 0.043 | 59.9 |
| 0 | ten-1gib | storage | tcopy workers 4 | 3 | 690.6 | 14.767 | 14.827 | 0.049 | 70.2 |
| 0 | ten-1gib | storage | tcopy workers 8 | 3 | 721.4 | 14.154 | 14.195 | 0.041 | 75.7 |
| 0 | ten-1gib | storage | tcopy workers 16 | 3 | 770.5 | 13.243 | 13.291 | 0.045 | 87.2 |
| 0 | hundred-100mib | storage | GNU cp -a | 3 | 673.5 | 13.272 | 14.849 | 1.577 | 48.8 |
| 0 | hundred-100mib | storage | rsync -a | 3 | 609.0 | 15.467 | 16.421 | 0.955 | 87.4 |
| 0 | hundred-100mib | storage | tcopy auto | 3 | 684.8 | 14.170 | 14.602 | 0.377 | 96.5 |
| 0 | hundred-100mib | storage | tcopy workers 1 | 3 | 603.2 | 16.331 | 16.578 | 0.247 | 56.4 |
| 0 | hundred-100mib | storage | tcopy workers 2 | 3 | 702.8 | 14.083 | 14.228 | 0.145 | 73.6 |
| 0 | hundred-100mib | storage | tcopy workers 4 | 3 | 704.2 | 13.835 | 14.201 | 0.366 | 94.1 |
| 0 | hundred-100mib | storage | tcopy workers 8 | 3 | 730.3 | 13.341 | 13.693 | 0.333 | 100.2 |
| 0 | hundred-100mib | storage | tcopy workers 16 | 3 | 679.7 | 14.292 | 14.713 | 0.382 | 106.0 |
| 0 | ten-thousand-1mib | storage | GNU cp -a | 3 | 287.7 | 18.979 | 34.756 | 15.186 | 35.9 |
| 0 | ten-thousand-1mib | storage | rsync -a | 3 | 335.1 | 20.547 | 29.839 | 6.970 | 64.3 |
| 0 | ten-thousand-1mib | storage | tcopy auto | 3 | 221.8 | 44.054 | 45.095 | 1.041 | 34.3 |
| 0 | ten-thousand-1mib | storage | tcopy workers 1 | 3 | 110.6 | 89.393 | 90.428 | 1.033 | 14.4 |
| 0 | ten-thousand-1mib | storage | tcopy workers 2 | 3 | 136.3 | 72.325 | 73.348 | 1.022 | 18.8 |
| 0 | ten-thousand-1mib | storage | tcopy workers 4 | 3 | 224.1 | 43.542 | 44.618 | 1.075 | 42.1 |
| 0 | ten-thousand-1mib | storage | tcopy workers 8 | 3 | 347.8 | 27.346 | 28.749 | 1.097 | 65.1 |
| 0 | ten-thousand-1mib | storage | tcopy workers 16 | 3 | 464.6 | 19.328 | 21.525 | 1.129 | 144.9 |

## Parallelism analysis

- Pair 0, one-10gib, storage: best parallel setting is **tcopy workers 16**, 1.05× workers 1. Observed ranges overlap or parallelism is slower; no clear gain beyond noise.
- Pair 0, ten-1gib, storage: best parallel setting is **tcopy workers 16**, 1.22× workers 1. Observed ranges overlap or parallelism is slower; no clear gain beyond noise.
- Pair 0, hundred-100mib, storage: best parallel setting is **tcopy workers 8**, 1.21× workers 1. Its entire observed range exceeds workers 1.
- Pair 0, ten-thousand-1mib, storage: best parallel setting is **tcopy workers 16**, 4.20× workers 1. Its entire observed range exceeds workers 1.

One dense file exposes no file-level parallelism in manual worker mode without chunks. More files can occupy more workers; additional workers can also increase same-device contention and per-file fsync costs. Treat these as explanations to check against recorded worker/chunk telemetry, not performance claims for untested storage.

Observed automatic-mode initial → final worker limits: **4 → 4** across 12 verified dense copies. These reflect the revised policy retaining admission limits without PSI-based throttling. Manual workers also retain their limits. The 10,000-file runs spend most of their summed worker time in metadata/sync work, so overlapping those waits produces the largest measured parallel gain. Observed ranges are not confidence intervals.

## Read/write accounting

Medians in GiB. Device totals cover all backing devices and include other host activity; process totals cover the tool and supervisor, ending before the final external fsync.

| Pair/cache | Workload | Tool | Process read GiB | Process write GiB | Device read GiB | Device write GiB |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| 0/storage | one-10gib | GNU cp -a | 10.000 | 10.000 | 10.003 | 10.030 |
| 0/storage | one-10gib | rsync -a | 10.000 | 10.000 | 10.000 | 10.042 |
| 0/storage | one-10gib | tcopy auto | 10.000 | 10.007 | 10.001 | 10.042 |
| 0/storage | one-10gib | tcopy workers 1 | 10.000 | 10.010 | 10.000 | 10.038 |
| 0/storage | one-10gib | tcopy workers 2 | 10.000 | 10.010 | 10.001 | 10.042 |
| 0/storage | one-10gib | tcopy workers 4 | 10.000 | 10.011 | 10.003 | 10.047 |
| 0/storage | one-10gib | tcopy workers 8 | 10.000 | 10.006 | 10.028 | 10.041 |
| 0/storage | one-10gib | tcopy workers 16 | 10.000 | 10.012 | 10.009 | 10.043 |
| 0/storage | ten-1gib | GNU cp -a | 10.000 | 10.000 | 10.007 | 10.053 |
| 0/storage | ten-1gib | rsync -a | 10.000 | 10.000 | 10.000 | 10.049 |
| 0/storage | ten-1gib | tcopy auto | 10.000 | 10.013 | 10.001 | 10.042 |
| 0/storage | ten-1gib | tcopy workers 1 | 10.000 | 10.013 | 10.002 | 10.031 |
| 0/storage | ten-1gib | tcopy workers 2 | 10.000 | 10.013 | 10.003 | 10.031 |
| 0/storage | ten-1gib | tcopy workers 4 | 10.000 | 10.013 | 10.005 | 10.048 |
| 0/storage | ten-1gib | tcopy workers 8 | 10.000 | 10.014 | 10.001 | 10.052 |
| 0/storage | ten-1gib | tcopy workers 16 | 10.000 | 10.014 | 10.004 | 10.048 |
| 0/storage | hundred-100mib | GNU cp -a | 9.766 | 9.766 | 9.766 | 9.821 |
| 0/storage | hundred-100mib | rsync -a | 9.766 | 9.766 | 9.769 | 9.804 |
| 0/storage | hundred-100mib | tcopy auto | 9.766 | 9.784 | 9.771 | 9.817 |
| 0/storage | hundred-100mib | tcopy workers 1 | 9.766 | 9.794 | 9.771 | 9.859 |
| 0/storage | hundred-100mib | tcopy workers 2 | 9.766 | 9.793 | 9.772 | 9.856 |
| 0/storage | hundred-100mib | tcopy workers 4 | 9.766 | 9.786 | 9.768 | 9.820 |
| 0/storage | hundred-100mib | tcopy workers 8 | 9.766 | 9.780 | 9.768 | 9.824 |
| 0/storage | hundred-100mib | tcopy workers 16 | 9.766 | 9.782 | 9.772 | 9.832 |
| 0/storage | ten-thousand-1mib | GNU cp -a | 9.766 | 9.770 | 9.766 | 10.328 |
| 0/storage | ten-thousand-1mib | rsync -a | 9.766 | 9.770 | 9.767 | 10.054 |
| 0/storage | ten-thousand-1mib | tcopy auto | 9.767 | 10.748 | 9.769 | 11.806 |
| 0/storage | ten-thousand-1mib | tcopy workers 1 | 9.766 | 11.435 | 9.779 | 13.239 |
| 0/storage | ten-thousand-1mib | tcopy workers 2 | 9.766 | 11.271 | 9.777 | 12.903 |
| 0/storage | ten-thousand-1mib | tcopy workers 4 | 9.767 | 10.756 | 9.771 | 11.821 |
| 0/storage | ten-thousand-1mib | tcopy workers 8 | 9.766 | 10.323 | 9.767 | 10.936 |
| 0/storage | ten-thousand-1mib | tcopy workers 16 | 9.766 | 10.131 | 9.770 | 10.548 |

## Creation and metadata overhead

| Pair | Workload | Tool | Median empty-tree s | Files/s | Final sync s |
| --- | --- | --- | ---: | ---: | ---: |
| 0 | one-10gib | GNU cp -a | 0.012 | 84.6 | 0.011 |
| 0 | one-10gib | tcopy workers 16 | 0.027 | 37.2 | 0.011 |
| 0 | one-10gib | tcopy workers 8 | 0.027 | 37.4 | 0.011 |
| 0 | one-10gib | tcopy workers 2 | 0.027 | 37.4 | 0.011 |
| 0 | one-10gib | tcopy auto | 0.027 | 37.6 | 0.011 |
| 0 | one-10gib | tcopy workers 1 | 0.027 | 37.6 | 0.011 |
| 0 | one-10gib | rsync -a | 0.057 | 17.6 | 0.012 |
| 0 | one-10gib | tcopy workers 4 | 0.027 | 37.4 | 0.011 |
| 0 | ten-1gib | tcopy workers 1 | 0.022 | 459.9 | 0.007 |
| 0 | ten-1gib | rsync -a | 0.053 | 187.6 | 0.008 |
| 0 | ten-1gib | tcopy workers 16 | 0.013 | 795.3 | 0.007 |
| 0 | ten-1gib | tcopy workers 4 | 0.017 | 599.6 | 0.007 |
| 0 | ten-1gib | tcopy workers 2 | 0.020 | 492.9 | 0.007 |
| 0 | ten-1gib | GNU cp -a | 0.008 | 1198.7 | 0.007 |
| 0 | ten-1gib | tcopy workers 8 | 0.015 | 647.6 | 0.007 |
| 0 | ten-1gib | tcopy auto | 0.016 | 630.4 | 0.007 |
| 0 | hundred-100mib | tcopy workers 16 | 0.482 | 207.5 | 0.370 |
| 0 | hundred-100mib | tcopy workers 1 | 0.173 | 578.9 | 0.056 |
| 0 | hundred-100mib | rsync -a | 0.106 | 940.8 | 0.058 |
| 0 | hundred-100mib | tcopy auto | 0.197 | 507.1 | 0.058 |
| 0 | hundred-100mib | GNU cp -a | 0.366 | 273.2 | 0.363 |
| 0 | hundred-100mib | tcopy workers 4 | 0.114 | 876.9 | 0.056 |
| 0 | hundred-100mib | tcopy workers 2 | 0.168 | 594.9 | 0.056 |
| 0 | hundred-100mib | tcopy workers 8 | 0.560 | 178.6 | 0.364 |
| 0 | ten-thousand-1mib | rsync -a | 1.004 | 9956.1 | 0.672 |
| 0 | ten-thousand-1mib | tcopy workers 8 | 29.458 | 339.5 | 22.905 |
| 0 | ten-thousand-1mib | tcopy workers 4 | 29.724 | 336.4 | 7.464 |
| 0 | ten-thousand-1mib | tcopy workers 1 | 60.616 | 165.0 | 2.621 |
| 0 | ten-thousand-1mib | tcopy workers 2 | 58.820 | 170.0 | 6.025 |
| 0 | ten-thousand-1mib | tcopy workers 16 | 13.100 | 763.4 | 5.500 |
| 0 | ten-thousand-1mib | GNU cp -a | 13.738 | 727.9 | 13.494 |
| 0 | ten-thousand-1mib | tcopy auto | 30.085 | 332.4 | 12.432 |

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
