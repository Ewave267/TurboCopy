# 004: Explicit fast paths and validated resume checkpoints

Phase 4 leaves the ordinary buffered/positioned copy paths as the default.
`--reflink` tries Linux `FICLONE` and falls back only on unsupported clone
errors. `--sparse` uses `SEEK_DATA`/`SEEK_HOLE` when available. These are opt-ins
because clone storage semantics and hole layout differ from physical copying;
neither syscall is assumed faster on every filesystem.

`--resume` copies sequentially into a 0600 partial under a private 0700
`.tcopy-resume` directory on the destination filesystem. A sidecar records the
exact destination basename and source device, inode, length, mtime and ctime.
The sidecar and partial are locked while used. Restart checks owner, mode, link count, state
and every retained byte before seeking to its end. A bad or stale checkpoint
fails without replacing the destination; the operator can remove the checkpoint
to restart. The partial and sidecar have deterministic names, while the sidecar
checks the full basename to turn hash collisions into explicit failures.

The data file is synced before atomic rename. After rename, the destination
parent and checkpoint directory are synced before the sidecar is removed, then
the checkpoint directory is synced again. Failed transfer bytes may survive a
crash but are not promised durable. Restart can finish publication cleanup after validating the source identity,
basic destination metadata and every destination byte. Torn initial sidecar
writes are repairable only before partial data exists. See
[recovery guarantees and regression coverage](../resume-recovery.md). The reserved `.tcopy-resume` entry
and incompatibility with `--sparse`/`--reflink` are deliberate limits that keep
checkpoint offsets meaningful and the publication path reviewable.

`--preserve-extended` requests ownership and xattrs, including POSIX ACL
xattrs, with failures reported. Ownership is set before mode and xattrs because
chown can clear set-id bits and security capabilities. `--checksum` compares
all bytes for sync decisions without adding a hash dependency; it is an exact
read comparison, not a stored digest. See [Phase 4 contract](../phase4.md).
