# Metadata synchronization

Task 2 (2026-10-17) reproduced identical-content files remaining at destination
mode 0400 rather than source mode 0644. Both local and native transport tests
failed before the fix: their sync branches returned `Outcome::Skipped` without
calling metadata preservation.

## Contract

Ordinary copying still atomically replaces regular files and preserves mode and
modification time. Sync now means **skip eligible contents, reconcile requested
metadata**. This intentionally changes the former behavior where a skipped
regular file's metadata was left untouched. No flags or defaults were added.

| Option | Contents | Regular-file metadata |
| --- | --- | --- |
| Default copy | Copy source bytes through an atomic stage | Preserve mode (including permission/special bits) and mtime |
| `--sync` | Keep destination bytes when path and size match, even if different | Reconcile mode and nanosecond mtime |
| `--sync --checksum` | Keep destination bytes only after full byte comparison | Same metadata contract as `--sync` |
| Local `--preserve-extended` | Does not strengthen the content comparison | Also reconcile uid/gid and Linux xattrs, including supported ACL xattrs; remove attributes absent from the source |
| `--dry-run` | Preview only | No mutation, even when preview says `skip` |
| `--no-fsync` | Keep atomic replacement | Omit durability barriers; metadata is not promised crash-durable |

SSH supports the same mode/mtime contract for uploads, downloads and relays.
Extended owner/group/xattr/ACL preservation remains explicitly unsupported over
SSH. Atime, ctime and birth time are not synchronized. Content comparisons may
change atime. Without `--preserve-extended`, ownership and xattr/ACL equality
are not requested; setting mode can adjust the existing ACL mask under Linux.
Directory and symbolic-link preservation follows the existing copy rules; only
regular files qualify for the sync content skip.

## Implementation and failures

For a single-link file, metadata changes use the opened destination descriptor,
without truncation or file-content writes. Matching basic metadata avoids even
opening the destination for metadata work. Extended metadata comparison reuses
the existing checked xattr readers, reads no additional file contents, and
avoids mutations when attributes already match. Existing preservation routines
apply requested ownership, mode, xattrs and timestamps and sync the file. Source
stability and destination identity are checked around synchronization. Read-only
regular files can be updated when ownership and read access permit it.

A hardlinked destination with metadata differences uses the existing private
stage and atomic rename helpers. The stage copies the **destination's existing
bytes**, applies the requested source metadata and syncs before publication,
then syncs its parent. This preserves size-only sync semantics and leaves aliases'
contents and metadata untouched. It necessarily copies data for this case;
unspecified ownership/xattrs follow ordinary replacement behavior. No source
content reads are added to plain sync. The transfer uses bounded existing
buffers and cancellation checks.

`Skipped` and the skipped-file count describe contents retained by the sync
comparison, including metadata-only updates. The transferred-byte counter is
zero for an in-place update and includes the local staging bytes for hardlink
replacement. Failures are reported and produce nonzero CLI status rather than
being counted as successful skips. Stages are cleaned up on failure.

File-content publication remains atomic. A sequence of in-place metadata
syscalls is not a transaction: an error or crash may leave some metadata
updated while file contents remain intact. Retry sync to finish reconciliation.
The fsync barrier makes successful metadata updates durable where supported;
`--no-fsync` intentionally weakens that guarantee. Stable source/destination
trees and the existing destination-directory trust boundary are still required;
this does not provide a snapshot against concurrent hostile writers.

The native SSH protocol is now version 2 (`TCOPY\0\0\x02`), adding a metadata
operation that validates expected file identity, rejects symlinks/special files,
and applies the same basic metadata routine used by ordinary native copies.
It stages hardlinked files on the destination agent, without sending their
contents over SSH. All participating binaries must support wire version 2;
version 1 fails at the handshake, with no silent fallback.

## Verification

Before-fix local and native regression tests failed on the incorrect mode.
The complete Rust suite passes: 56 unit, 38 copy integration and 5 extended
metadata integration tests. Coverage includes plain/checksum sync, nanosecond
mtime, read-only files, unchanged inode/content, repeated no-op sync, dry-run,
hardlink aliases, size-only different bytes, access errors with nonzero status,
xattr convergence/removal, default xattr behavior, ACL convergence, and stale
or symlinked native targets. Existing atomic-replacement, resume and failure
tests remain enabled. Xattr/ACL tests retain platform capability guards;
ownership tests requiring root retain their existing guard and were not
executed as root in this session.

All nine real loopback SSH tests passed, including upload, download and relay metadata-only updates,
dry-run, preserved inodes, nanosecond timestamps and an inaccessible-destination
failure. Formatting, strict all-target Clippy, and release build are checked.
No WAN, network filesystem or physical power-loss validation is claimed.
