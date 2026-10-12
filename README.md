# TurboCopy

TurboCopy is a Linux command-line file copier with bounded file and chunk
parallelism and atomic destination replacement. This repository implements
**Phases 1 and 2 — Correctness-first MVP and Performance** of [project.md](project.md).

## Build and use

Requires Rust 1.75+, Linux x86-64 or AArch64, and mounted `/proc/self/fd`.
There are no third-party Rust dependencies.

```bash
cargo build --release
./target/release/tcopy source.bin destination.bin
./target/release/tcopy source_dir/ destination_dir/
./target/release/tcopy first.bin second.bin existing_destination_dir/
./target/release/tcopy source_dir/ destination_dir/ --sync
./target/release/tcopy source_dir/ destination_dir/ --dry-run
./target/release/tcopy large.bin destination.bin --workers 4 --chunk-size 8M --progress
./target/release/tcopy source_dir/ destination_dir/ --auto --progress
./target/release/tcopy source_dir/ destination_dir/ --tune --json
./target/release/tcopy source_dir/ destination_dir/ --auto --no-tune
./target/release/tcopy --help
```

A new destination takes the exact supplied name. An existing destination
directory receives the source basename, as with `cp -r`. A trailing slash on a
real directory does not switch to copying only its contents; use `source_dir/.`
for that. Multiple sources need an existing destination directory. Destination
parents must already exist. Use `--` before paths beginning with `-`.

Files are overwritten by default; destination directories are merged and extra
entries retained. Source links are recreated, including dangling links and links
to directories. Destination directory symlinks are refused, including symlink
components in parent paths. Final destination file symlinks are replaced;
their targets are never opened for writing. Special sources and destinations
fail explicitly. A regular-file path must not end in `/`.

`--sync` skips regular files at the corresponding path when their sizes match.
**Matching sizes do not guarantee matching contents.** It does not compare
timestamps or hashes, and it never deletes extra destination files.
`--dry-run` reports planned files, bytes and skips without destination writes.

Regular files and directories retain permission bits and modification times;
links retain their targets and modification times. Hard links become independent
files. Sparse-file contents are retained, but holes may expand. Ownership, ACLs
and extended attributes are reserved for Phase 4.

The default uses up to four file workers and a 64-entry queue. `--workers N`
sets a shared limit of 1–128 active file/chunk slots. `--chunk-size SIZE` opts
into parallel positioned copying for files at least 64 MiB with multiple chunks
and idle slots. Sizes accept bytes or binary K/M/G and KiB/MiB/GiB suffixes,
from 1 KiB to 64 MiB. Known HDDs and network/unsupported filesystems retain
buffered copying; unavailable helpers also use that path. Extra workers can make copying slower.

`--auto` selects a CPU/memory-bounded worker ceiling from storage profiles: one
slot for shared or unknown-topology HDDs, two for independent HDDs, network or
unknown storage, four for shared fast storage, and up to eight for independent
fast storage. Automatic chunks require known SSD/NVMe/memory local storage,
confirmed independence, and files at least 256 MiB; each file selects 4–16 MiB
ranges from its length. A manual `--chunk-size` retains the 64 MiB threshold and
Phase 2 storage guards. Mounted subdirectories are checked per file.

`--tune` implies `--auto`, starts at one slot and observes the real copy workload
in up to eight one-second windows (about eight seconds, subject to scheduling). It tries increasing limits,
requires occupied slots and meaningful work, and stops when a candidate gains
less than 5%. It creates no extra benchmark files. Short copies can finish before
calibration; `insufficient-work` means no measurement was made.
`--auto` and `--tune` reduce admission under CPU/I/O pressure and recover gradually
after pressure subsides. Running files/helpers finish with their existing permits,
so a single large file is not repartitioned mid-copy. `--no-tune` fixes the selected
configuration; combining it with `--tune` is an error. Explicit `--workers` fixes
the admission limit and explicit `--chunk-size` fixes range size.

The hardware profile reads logical/physical CPUs, MemAvailable, visible cgroup v2
CPU/memory limits and Linux pressure information. Automatic buffer allocation
uses at most 1/64 of available memory, with a one-slot floor; thread stacks and
filesystem caches are additional. Missing topology/limits remain advisory;
legacy cgroup v1 limits are not read. Tuning is a heuristic and has no persistent
cache or guarantee of optimal throughput.

Each file worker and temporary chunk helper uses a 128 KiB buffer; at most
`2 * workers - 1` such buffers exist. Chunk size does not allocate a chunk-sized
buffer. Directory traversal streams instead of retaining the entire tree.
Storage reporting identifies filesystem, mount device, visible physical backing
disks and SSD/NVMe/HDD/memory/unknown classification. Device-mapper layers and
partitions are traced through sysfs, including mapper names whose `/dev` nodes
are unavailable. Btrfs independence is reported only when sysfs exposes every
member and confirms none are missing. Incomplete topology cannot prove physical
independence; detection is advisory.

`--progress` prints live write counts and throughput to stderr every 250 ms.
Traversal discovers files as it runs, so no total-size estimate or ETA is shown.
`--json` emits one final JSON object to stdout; dry-run actions and diagnostics
use stderr. It includes committed bytes, transferred bytes (including failed
attempts), elapsed time, throughput, configured workers/chunks, peak active slots,
worker utilization, operation timings and source/destination storage pairs.
Adaptive reports add final admission limit, adjustment/sample counts, tuning state,
chunk policy, hardware information, advisory destination free space and counts of
small (<1 MiB), medium and large (>=64 MiB) regular files. The calibration score
is attempted bytes plus 4 KiB per committed file, divided by the sample duration;
`tuning_best_score` is this score, not physical device throughput. `workers` is
the configured ceiling; `final_workers` can be lower. The utilization denominator
remains the ceiling. For automatic chunks, `chunk_size` is the root profile hint
and the per-file size may differ; `chunk_policy` distinguishes automatic selection.
Dry-run bytes are planned bytes and its throughput is zero. Metadata/transfer
timers accumulate operation wall time across workers and traversal and can exceed
elapsed time; utilization
measures occupied slots rather than CPU load. Ordinary output reports the same
core totals in readable form.

Failures appear on stderr and in the final summary. Exit statuses: 0 success,
1 copy/metadata failure, 2 CLI misuse, 130 SIGINT, 143 SIGTERM.

## Reliability limits

File data and metadata are synced before atomic rename, and the parent directory
is synced after publication. A failed transfer preserves the previous destination
and removes its temporary file. A parent-sync error is reported even if the new
file was already published. Guarantees depend on filesystem behavior; a whole
directory copy is not transactional. Finished files survive cancellation.

SIGINT and SIGTERM stop copying at buffer boundaries and clean up temporary
files. A blocking filesystem syscall can delay cancellation. Crashes, power loss
and SIGKILL can leave `.tcopy-*.tmp` files; these are never presented as completed
destination files. Temporary names are never automatically swept on startup.
Source identity, size, mtime and ctime changes are checked, but copying a live
source is not a filesystem snapshot. Keep source data stable for consistent copies.

## Verify

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
python3 benches/compare.py --binary target/release/tcopy --output /tmp/tcopy-benchmark.json
python3 benches/io_methods.py --directory . --output /tmp/tcopy-io-benchmark.json
graphify extract . --code-only --max-workers 1
```

Run permission tests as an unprivileged user. Disk-full propagation is tested
with an injected ENOSPC writer and positioned writes to `/dev/full`; a kernel
write-limit failure verifies atomic rollback. Tests also cover cancellation, symlink swaps, changing sources,
metadata, sync, arbitrary Unix filenames, and wide/deep trees.
The benchmark harness requires Python 3.11+.
The checked-in [Graphify index](graphify-out/GRAPH_REPORT.md) records code
relationships; [graph.html](graphify-out/graph.html) provides an interactive view.

See [Phase 1 specification](docs/phase1.md), [Phase 2 specification](docs/phase2.md), [Phase 3 specification](docs/phase3.md),
[architecture](docs/architecture.md), [atomic copy decision](docs/decisions/001-correctness-first.md),
[chunk scheduling decision](docs/decisions/002-manual-chunks.md),
[adaptation decision](docs/decisions/003-workload-adaptation.md), and
[benchmark methodology/results](docs/performance.md).

## Roadmap

1. **Correctness-first MVP:** CLI, recursion, overwrite, sync, atomic replacement,
   basic metadata, bounded file-level parallelism and tests.
2. **Performance:** storage detection, worker/chunk controls, chunk parallelism,
   syscall benchmarks, progress and throughput metrics.
3. **Hardware adaptation:** automatic configuration, active tuning, feedback
   adjustment, storage/workload profiles.
4. **Advanced functionality:** sparse optimization, reflinks, advanced Linux I/O,
   resume, extended metadata and extensive filesystem benchmarks.

Phases 1–3 are implemented. `--checksum` remains unimplemented and returns a CLI
error. Experimental copy syscalls are benchmarked
separately; production copying does not implicitly clone or offload files.
