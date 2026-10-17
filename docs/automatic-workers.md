# Automatic worker selection — Task 3

The pre-change revision is `a9c71f9`. Its release binary was retained before
implementation. Measurements reproduced the auto/manual gap on the available
Btrfs/NVMe storage: five randomized repetitions of 4,096 × 1 MiB gave automatic
four workers 232.3 MiB/s median (199.1–240.6), versus manual 16 workers
563.0 MiB/s (472.5–727.2). Manual four workers matched automatic mode at
232.6 MiB/s. These are new measurements, not the historical 10,000-file result.

Worker utilization was near saturation while process CPU remained below the
machine's capacity. Summed metadata/sync time dominated transfer time; sampled
worker waits included `wait_log_commit`, `folio_wait_bit_common` and
`barrier_all_devices`. Both the shared-fast-storage ceiling of four and the
physical-core ceiling of four prevented useful overlap of those waits.
The copy queue already supplies enough work; changing the queue or adding
another copy engine was unnecessary.

## Policy

For ordinary durable local copies, examine at most 64 source entries across the
planned roots, stopping once 16 regular files of at most 1 MiB are found. Only
immediate children are inspected; the probe does not recurse or read contents.
Directory handles anchor the probe, and symlinks are not followed. Entries that
cannot be inspected contribute no evidence; insufficient evidence keeps the
conservative policy. The ordinary scan still reports source errors. There is no timed calibration, trial copy or added write.

Trees with nested source or destination mounts retain the old limit, even when
the root is local NVMe. This prevents an inherited higher limit on a network or
other unmeasured subtree; no additional traversal is needed to check mount inventory.
Mount configuration is assumed stable during the copy.

The higher limit requires **every** source/destination pair to be confirmed
NVMe with complete topology, known shared storage, and an existing supported
local fast-filesystem classification. It does not target Btrfs by name: ext4,
XFS, F2FS and ZFS qualify under the same storage/workload checks. Unit fixtures
cover eligibility; they do not establish performance on those other filesystems.

| Case | Automatic file-worker ceiling before CPU/memory bounds |
| --- | ---: |
| Sampled small-file workload, confirmed shared NVMe, durable ordinary copy | 16; additionally at most twice available logical CPUs |
| Other known fast shared storage, including an unsampled NVMe workload | 4; physical-core bound |
| Known independent fast storage | 8; physical-core bound |
| HDD with known independent endpoints | 2; physical-core bound |
| HDD with shared/unknown relationship | 1 |
| Network, unknown or unsupported storage | 2; physical-core bound |
| Native GPFS profile | Existing memory-bound limit up to 32 |
| Native SSH | Existing fixed default up to 4 |

`--sync`, `--resume`, `--no-fsync`, dry-run, and explicit workers bypass the new probe and
retain their previous selection. Their workload need not include the measured
durability waits or has additional checkpoint resources. Explicit workers/chunks retain precedence, and the existing
large-file chunk rules are unchanged. Source samples can be unrepresentative,
especially for nested or heterogeneous trees; this is a bounded heuristic, not
an optimal-concurrency claim. Unknown and independent storage are deliberately
not given a higher unmeasured limit.

The existing buffer budget remains: at most one sixty-fourth of available
memory divided by 256 KiB per slot, with a minimum of one slot. Available memory
and logical CPU counts retain their existing cgroup/affinity restrictions. The
new ceiling is 16 even on larger machines. Sixteen 128 KiB file buffers plus
possible helper buffers reserve at most 4 MiB; thread stacks, directory handles
and the existing 64-task queue add overhead. Normal copies open at most two
file-data descriptors per active file, plus directory/traversal descriptors;
resume has additional bounded state/directory descriptors. No unbounded pool
or additional queue was introduced.

Selection happens once. Default mode has no worker-count oscillation and starts
no new monitor thread. The existing optional `--tune` controller can still
calibrate within the selected ceiling; `--no-tune` retains a fixed selection.
Human reports and JSON expose `worker_policy` (`nvme-small-files`,
`conservative`, `gpfs`, `manual`, or `ssh-fixed`), alongside selected/final workers,
peak active workers, hardware, storage and adjustment counters.

## Final results (2026-10-17)

Five randomized before/after repetitions per workload; same encrypted Btrfs/NVMe device,
four physical/eight logical CPUs, Rust 1.95.0, source pages evicted. Rates are durable MiB/s
with observed min–max ranges, except the short row, which reports milliseconds. These ranges
are not confidence intervals. The parent revision was rebuilt independently and reproduced
the retained baseline binary byte-for-byte; [provenance](benchmarks/task3/provenance.json)
records both binary and source hashes.

| Workload | Before median (range) | After median (range) | Workers before → after |
| --- | --- | --- | --- |
| 1 × 512 MiB | 958.02 (922.19–999.65) | 1006.39 (985.62–1016.61) | 4 → 4 |
| 8 × 64 MiB | 1408.87 (1376.70–1432.99) | 1417.52 (1388.83–1434.52) | 4 → 4 |
| 4,096 × 64 KiB | 20.49 (16.28–32.60) | 63.34 (51.96–101.88) | 4 → 16 |
| 4,096 × 1 MiB | 209.34 (197.21–214.97) | 567.07 (433.30–579.13) | 4 → 16 |
| Mixed: 2,180 files / 392 MiB | 47.97 (43.44–189.98) | 479.78 (134.24–488.87) | 4 → 16 |
| 1 × 4 KiB (milliseconds) | 6.39 (5.50–7.75) | 6.95 (5.53–8.44) | 4 → 4 |

The 1 MiB-file workload improved **2.71×** and the 64 KiB-file workload **3.09×**;
both final observed throughput ranges are disjoint. Configured/peak workers were 16 and
adjustment counters stayed zero. Large-file selection stayed four (single-file peak one);
the several-large case changed only 0.6%. The short-copy median increased 0.56 ms with
overlapping ranges; no calibration is performed.

Mixed-size results are particularly unstable. The initial three-run experiment showed a
23.5% median slowdown, while the five-run final confirmation showed a large median gain.
Ranges overlap in both experiments, and their groups are not pooled. The initial
several-large median slowdown also did not reproduce. These observations support the
homogeneous many-file improvement much more strongly than any claimed mixed-size gain.
There was no persistent material regression in the repeated controls.

For 4,096 × 1 MiB, median process CPU rose from **64.3% to 201.6%** (100% is one logical CPU),
sampled peak RSS from **3.52 to 5.30 MiB**, and sampled peak descriptors from **16 to 40**.
Files/s rose from **209.3 to 567.1**; the 64 KiB workload rose from **327.8 to 1,013.5 files/s**.
This buys throughput with more bounded CPU/memory/descriptor use, rather than free resources.
All other per-workload resource observations and files/s are retained in the raw reports.

Warm tmpfs controls retained four workers and do not test physical storage throughput.
The first three-run mixed control had a 5.37 ms median increase, so it was repeated nine
times: **54.74 ms before → 55.45 ms after** (+1.3%), with overlapping ranges
(51.73–73.84 versus 52.92–72.85 ms). The small-file and short tmpfs controls showed no
persistent regression. This is evidence about unchanged selection on tmpfs, not a
claim of a new memory-filesystem optimization.

### Baseline tools and manual workers

The initial candidate matrix compared all seven configurations on identical sources
within each workload (three shuffled repetitions). The subsequent mount/resume guards
did not change eligibility or worker counts on these datasets. These earlier measurements
are retained separately from the five-run final confirmation; do not mix their medians.

| Workload | Auto before | Initial auto candidate | Manual 4 | Manual 8 | Manual 16 | GNU cp | rsync |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 × 512 MiB | 996.3 | 1004.9 | 1003.6 | 1004.4 | 996.1 | 619.2 | 703.3 |
| 8 × 64 MiB | 1051.4 | 731.2 | 733.9 | 1355.7 | 863.9 | 558.5 | 643.5 |
| 4,096 × 64 KiB | 23.2 | 71.2 | 17.9 | 28.7 | 195.5 | 39.3 | 42.6 |
| 4,096 × 1 MiB | 229.1 | 505.9 | 199.1 | 364.8 | 524.4 | 304.6 | 165.0 |
| Mixed: 2,180 files / 392 MiB | 174.1 | 133.3 | 43.9 | 169.9 | 376.9 | 37.2 | 62.5 |

Units are durable MiB/s. TurboCopy is not universally faster than cp or rsync;
in particular these results vary substantially by run and workload.

### Artifacts and validation

- [Pre-change Phase 5 preflight (deliberately stopped)](benchmarks/task3/preflight-stopped.json).
- [Five-run baseline gap](benchmarks/task3/baseline-gap.md) ([raw JSON](benchmarks/task3/baseline-gap.json)).
- [Initial all-tool candidate comparison](benchmarks/task3/candidate-all-tools.md) ([raw JSON](benchmarks/task3/candidate-all-tools.json)).
- [Final guarded build confirmation](benchmarks/task3/final-confirmation.md) ([raw JSON](benchmarks/task3/final-confirmation.json)).
- [Tmpfs controls](benchmarks/task3/tmpfs-control.md) and [nine-run mixed follow-up](benchmarks/task3/tmpfs-mixed-confirmation.md).

The complete matrices contain **252 verified benchmark copies**, plus the deliberately
stopped preflight. All final 60 NVMe and 36 tmpfs comparisons passed exact contents/metadata
checks. The final guarded build passed **103 Rust tests**, formatting, strict all-target
Clippy and release build; **10 Python benchmark tests** passed. No durability or integrity
tests were disabled. Root-only ownership tests retain their existing privilege guards.

The early wait sampler also counted the coordinator when its executable name began
with `tcopy-`; the final sampler limits observations to numeric worker thread names.
Only final worker-wait samples are used for detailed worker interpretation. A first
tmpfs setup stopped before allocating data because the initial reserve check ignored
the configured reserve; it was fixed and regression-tested. NVMe defaults were unaffected.

Attempts: a blanket higher fast-storage limit was rejected in favor of workload evidence;
new active adaptation was not justified; the static bounded probe was retained after
the large homogeneous gains reproduced. Mixed/large and tmpfs follow-ups addressed noisy
initial apparent regressions rather than selectively hiding those samples.

## Measurement method

`benches/worker_selection.py` reuses Phase 5's incompressible dense generation,
cache-residency verification, free-space/memory/pressure guards, subprocess
accounting and exact SHA-256/metadata manifests. It adds mixed sizes and
interleaves the retained before/after binaries, manual 4/8/16, GNU cp and rsync
with a recorded seed. It synchronizes setup directory changes before timing.

TurboCopy's command duration includes its own file and parent fsync barriers.
GNU cp and rsync durations include an external file/directory fsync pass to
supply end-of-run durability. TurboCopy is not charged a second, redundant
sync pass in this paired matrix. This differs from the older Phase 5 reports,
which retain the common extra sync cost for every tool; do not mix their rates.
Per-file atomic replacement and intermediate crash guarantees still differ
between tools. There is no claim of universal superiority over cp or rsync.

Storage-mode samples require at most 1% resident source pages immediately
before launch. Controller/device and metadata caches, temperature, and other
host activity are uncontrolled. Warm-cache tmpfs checks are recorded separately;
they represent memory/filesystem overhead, not a second physical storage device.
Raw JSON records each sample, median/range, throughput, files/s, CPU, resource
usage, worker telemetry, binary checksums and filesystem options. CPU is process
user/system time plus the final external sync's CPU time; kernel background
work is not fully attributed. Actual-executable RSS, file-descriptor and wait
samples are taken every 100 ms. Sampled peaks may miss brief peaks; child-rusage
maximum RSS is also retained but can include pre-exec Python memory.

The original unmodified Phase 5 preflight was deliberately stopped after
collecting the relevant initial groups. Its stopped status and partial groups
remain explicit; the completed paired matrix supplies the final comparison.

## Reproduce

Retain a release binary built from `a9c71f9` as `target/task3/tcopy-before`, then
build this change as `target/task3/tcopy-final`. Do not run other disk-heavy work
concurrently. The default dataset matrix needs about 8 GiB for its largest
source/destination pair, plus a 20 GiB / 10% storage reserve and at least 512 MiB
available memory. Dataset preparation, verification and cleanup are outside the
reported interval. The benchmark removes only its owned temporary trees.

```bash
python3 benches/worker_selection.py --before target/task3/tcopy-before \
  --after target/task3/tcopy-final --runs 3 --output target/task3/paired
python3 benches/worker_selection.py --before target/task3/tcopy-before \
  --after target/task3/tcopy-final --auto-only --runs 5 --output target/task3/final
python3 benches/worker_selection.py --before target/task3/tcopy-before \
  --after target/task3/tcopy-final --auto-only --directory /tmp --cache warm --reserve-gib 1 \
  --workload many-small --workload mixed --workload short --runs 3 \
  --output target/task3/memory
python3 benches/worker_selection.py --before target/task3/tcopy-before \
  --after target/task3/tcopy-final --auto-only --directory /tmp --cache warm --reserve-gib 1 \
  --workload mixed --runs 9 --output target/task3/memory-mixed
```

Run `python3 -m unittest discover -s benches -p 'test_*.py'`, `cargo test --locked`,
`cargo fmt --check`, and strict all-target Clippy for correctness checks. Existing
resume, metadata, atomicity, failure and native-transport tests stay enabled.
New tests cover probe bounds, no recursion/symlink following, storage eligibility,
CPU/memory ceilings, and manual overrides. No copy or fsync barriers were removed.

## Limits

Only one writable physical device is available here: encrypted NVMe under Btrfs,
on an i7-1195G7 with four physical/eight logical CPUs. Independent physical-device
pairs and authorized GPFS/NFS mounts are unavailable. Their prior selection is
unchanged; no performance or correctness validation on those environments is
claimed. The benchmark uses reduced datasets rather than the historical
10 GiB-per-workload suite. Short transfers have sampling/startup noise, not a
performance guarantee. Other storage, very deep trees and workloads beyond the
sample's size/count limits may still benefit from explicit worker settings.
