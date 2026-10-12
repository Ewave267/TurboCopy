# Phase 1 specification

Implement the correctness-first phase of `project.md` for Linux x86-64 and
AArch64, using Rust 1.75+ and the standard library. No third-party dependencies.

## Acceptance criteria

| Capability | Observable result |
| --- | --- |
| CLI | `tcopy [--sync] [--dry-run] SOURCE... DESTINATION`; help, version, `--`, invalid options and missing operands handled. |
| Mapping | One source to an absent destination uses that exact name; an existing destination directory receives the source basename. Multiple sources require an existing destination directory. Trailing slashes on real directories do not change this mapping. Explicit `.` means directory contents. |
| Traversal | Regular files, empty directories, arbitrary Unix filenames and nested trees are copied; symlinks (including dangling links) are recreated. FIFOs, sockets and devices fail explicitly. Hard links become independent copies. |
| Overwrite | Existing regular files or symlinks are replaced atomically. Directories are merged, with extra destination entries retained. Type conflicts with directories fail. |
| Sync | Skip only regular destination files of matching relative path and size. Same-size different contents are intentionally skipped, with a CLI warning. Links are recreated. |
| Safety | Reject self-copy, hard-link aliases, destination inside source, destination ancestors containing the source, and colliding source roots before writes. Never traverse destination symlinks or replace special destination files. Refuse regular-file paths ending in `/`. |
| Atomicity | Use an exclusively created, owner-only temporary file in the opened destination directory; copy, validate source, set metadata, sync file, rename, sync parent. A failed copy preserves the old destination; ordinary errors/cancellation remove owned temporary files. |
| Metadata | Preserve regular-file and directory permission bits and nanosecond modification timestamps; preserve symlink modification timestamps. Finalize directories after descendant workers complete. |
| Parallelism | Up to four workers (bounded by available CPU count), 64 queued tasks, one 128 KiB buffer per worker; stream traversal rather than store the tree. |
| Failures | Report each failed entry and a final copied/skipped/bytes/failures summary; exit 1 on failure, 2 on CLI errors, 128+signal on SIGINT/SIGTERM. |
| Dry run | Traverse and validate without creating directories, temporary files, or changing metadata. Report planned files/bytes and sync skips. |
| Verification | Cargo integration tests exercise the real CLI and filesystem, including data-preserving failures and cancellation. Run formatting, Clippy and a release build. |

## Implementation order

1. CLI, safety helpers and atomic regular-file copy, with tests.
2. Streaming directory scanner, bounded scheduler, sync and dry run.
3. Links, directory metadata, cancellation, failure regression tests.
4. Review, build checks, benchmark harness and Graphify architecture index.

## Commands and layout

`cargo build --release`, `cargo test`, `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`,
`python3 benches/compare.py --binary target/release/tcopy`.

`src/cli.rs` parses OS strings; `src/copy_engine.rs` copies one task;
`src/scanner.rs` maps and traverses; `src/scheduler.rs` bounds concurrency;
`src/fsutil.rs` anchors destination paths and handles Linux interfaces;
`src/main.rs` handles exit status. Integration tests belong in `tests/`.
Use ordinary `io::Result`, descriptive snake_case names and `cargo fmt`.

## Boundaries and assumptions

Always validate paths before writes, retain error handling, and test filesystem
outcomes. Never delete unrelated files or follow destination directory symlinks.
Phase 2–4 flags are rejected rather than accepted without implementing them.
`--checksum` is deferred: Phase 1 specifically implements size-only sync.
Manual worker controls and JSON/progress reporting belong to Phase 2.

Destination parent directories must already exist. Linux `/proc/self/fd` must
be mounted. Descriptor anchoring protects against destination symlink swaps;
a privileged adversary who can relocate opened directories or mount filesystems
is outside the threat model. Sources should be stable: size/mtime/ctime/inode
checks detect practical changes, but do not guarantee a snapshot. Sparse file
contents are preserved with buffered I/O; holes may consume physical storage.
Permissions and mtimes are preserved; ownership, ACLs, xattrs, hard-link identity
and symlink permission bits are not. No speed claim is made.

Atomicity is per file, not per tree. Directory metadata errors and parent fsync
errors are failures even if file contents were already published. SIGKILL,
power loss or process crash may leave recognizable `.tcopy-*.tmp` files. Existing
destination data is never truncated in place. Directory creation is not a tree
transaction; completed files survive cancellation. Filesystems, especially NFS,
can offer weaker durability than local Linux filesystems.

## Verification record

On 2026-10-11, Rust/Cargo 1.95.0 on Linux x86-64 passed `cargo test`
(4 unit tests and 21 CLI/filesystem tests), `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, and `cargo build --release`.
Inverting the sync condition made its regression test fail; the mutation was
restored before final checks. The release benchmark checks contents and metadata
against `cp` and `rsync`; results live in [performance.md](performance.md).

AArch64, real filesystem exhaustion, power-loss recovery, NFS and HDD behavior
were not exercised here. ENOSPC is injected at the writer boundary, and a kernel
file-size limit verifies failed-write rollback against the real CLI. Existing
bind-mount ancestry is checked by inode identity, but a privileged mount/rename
adversary and a filesystem snapshot guarantee remain outside the contract.
