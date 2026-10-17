# Phase 4 specification

Phase 4 adds explicit Linux copy options while retaining the Phase 1 atomic
replacement, bounded worker pool, and conservative default behavior.

| Capability | Acceptance criterion |
| --- | --- |
| Sparse files | `--sparse` uses `SEEK_DATA`/`SEEK_HOLE` for regular files, keeps logical length and byte content, and falls back to ordinary transfer when unsupported. Without the flag, contents remain correct even if holes become allocated. |
| Reflinks | `--reflink` attempts `FICLONE` into a private temporary file. Unsupported clone operations fall back to ordinary copying; other errors fail safely. Cloning is never implicit. |
| Resume | `--resume` retains a private partial regular file after interruption or transfer error. A later run accepts it only when the recorded source identity and destination name match and every retained byte equals the current source. Stale or malformed state fails closed; the published destination is never the partial file. Without `--resume`, failed temporary files are removed. |
| Extended metadata | `--preserve-extended` requests owner/group and Linux extended attributes (including ACL xattrs) for regular files, directories, and links where supported. Requested preservation errors fail the copy. Default behavior keeps basic mode and mtime only. |
| Checksum sync | `--sync --checksum` skips a regular file only when sizes and all bytes match; no hash dependency or cache. `--checksum` requires `--sync`. Ordinary `--sync` keeps its size-only behavior. |
| Linux I/O experiments | Benchmark buffered read/write, positioned read/write, `copy_file_range`, `sendfile`, sparse seeking, and reflinks on available filesystems. Runtime uses the proven existing buffered/positioned paths, with optional sparse and clone paths. `io_uring` is deferred until measurements justify its complexity. |
| Filesystem benchmarks | Record environment, commands, available filesystem types, first-copy and incremental samples, durability and content checks, and unavailable configurations. Do not claim general speed advantages from one host. |

The resume state is an optimization, not a backup. Resume compares the entire
retained prefix before use; this can take as much reading as a new copy. It
rejects symlinked state and source identity changes. A concurrent hostile writer
with permission to alter the destination directory can interfere with any copy,
so the directory itself remains the trust boundary. Empty files, source changes
during transfer, cancellation, unsupported filesystems, and existing destination
symlinks must retain the existing safe failure behavior.
`--resume` reserves the `.tcopy-resume` entry in each destination directory and
cannot be combined with `--sparse` or `--reflink`; its partial is sequential.
It retains failed transfer progress on a best-effort basis, then verifies all
retained bytes on restart. Initialization and publication interruptions now recover automatically when
state and retained contents can be validated; see [resume recovery](resume-recovery.md).
Corrupt or stale checkpoints still fail closed. Retained bytes are not a durability guarantee.

Implementation slices: CLI and checksum sync; optional clone and sparse transfer;
validated resume state; extended metadata; filesystem benchmark matrix. Tests
cover each opt-in path and fallback, with the existing regressions retained.

## Completed verification

`cargo fmt --check`, strict all-target Clippy, and `cargo test` pass on the
Phase 4 change (28 unit, 29 copy integration, four extended-metadata integration
tests). An intentional inversion of the checksum equality guard caused the
focused dry-run regression to fail, then the guard was restored. Resume tests
cover an interrupted transfer, valid retry, changed source, corrupt retained
bytes, symlinked sidecar, hardlinked partial, concurrent lock, and the reserved
destination name. The filesystem benchmark recorded 198 verified warm-cache
copies across available Btrfs/tmpfs pairs; [results and limitations](performance.md)
retain individual samples and syscall experiments.

ACL preservation was not exercised at runtime here: `setfacl` source setup
returned `EINVAL`. This filesystem also rejected symlink `user.*` xattrs. The
ownership-failure test runs only when invoked as root. NFS, HDD, independent
NVMe pairs, cold caches, crash injection, Rust 1.75 and AArch64 remain untested.
