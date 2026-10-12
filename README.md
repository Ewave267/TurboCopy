# TurboCopy

TurboCopy is a Linux command-line file copier with bounded file-level
parallelism and atomic destination replacement. This repository implements
**Phase 1 — Correctness-first MVP** of [project.md](project.md).

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
and extended attributes are outside Phase 1.

Up to four file workers share a 64-entry queue. Each worker uses a 128 KiB
buffer; directory traversal streams instead of retaining the entire tree.
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
graphify extract . --code-only --max-workers 1
```

Run permission tests as an unprivileged user. Disk-full propagation is tested
with an injected ENOSPC writer; a real kernel write-limit failure verifies atomic
rollback. Tests also cover cancellation, symlink swaps, changing sources,
metadata, sync, arbitrary Unix filenames, and wide/deep trees.
The benchmark harness requires Python 3.11+.
The checked-in [Graphify index](graphify-out/GRAPH_REPORT.md) records code
relationships; [graph.html](graphify-out/graph.html) provides an interactive view.

See [specification](docs/phase1.md), [architecture](docs/architecture.md),
[design decision](docs/decisions/001-correctness-first.md), and
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

Hardware flags (`--auto`, `--tune`, `--no-tune`), worker/chunk overrides,
`--checksum`, and `--json` are not implemented in Phase 1 and return CLI errors.
