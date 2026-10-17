# Resume recovery

Task 1 (2026-10-17) confirmed the reported failure before implementation. A
regression wrote and synced a partial, renamed it to the destination, and stopped
before sidecar deletion. Restart failed with `EEXIST`: it created a new partial
and then attempted exclusive creation of the still-existing state file.

The sidecar now provides a lock across the entire lifecycle, including the
publication interval where no partial name exists. The partial lock remains,
including compatibility with existing checkpoint locking tests. The on-disk
source identity format and CLI are unchanged. No dependencies were added.

## Recovery contract

- State records the full destination basename and source device, inode, size,
  mtime and ctime. Validation rejects changed sources, symlinks, unsafe owners,
  hardlinks and conflicting checkpoints. The destination directory remains the
  trust boundary against hostile concurrent writers.
- State is initialized before creating a partial and synced before retaining
  transfer data. An interrupted initial state write can be repaired only if its
  bytes are a prefix of the expected state and the partial is absent or empty.
  This also handles legacy empty partials without a sidecar.
- Every retained partial byte is compared with the source before resuming at
  its length. Completed partials remain usable after metadata has changed their
  permissions, including read-only modes. Root can resume a completed partial
  whose owner was changed to the recorded source owner by extended preservation.
  Recovery from mode `000` opens an anchored Linux `O_PATH` handle directly;
  this avoids Rust `OpenOptions` masking that flag on musl builds. Ownership,
  hardlink and full-content validation still apply before publication.
- When valid state exists without a partial, recovery opens the destination
  without following symlinks. It requires a regular file with matching size,
  mode and mtime, compares every byte, and rechecks file and source stability.
  Only then does it sync the destination and both directories, remove the
  matching sidecar and sync the checkpoint directory. It keeps the destination
  inode and writes no file contents. Repeated interruptions before cleanup are
  safe. This path completes a previously published copy; metadata convergence
  for arbitrary existing destinations is tracked separately as Task 2.
- A missing or nonmatching destination is copied through a new partial while
  the current destination stays intact until atomic rename. An unrelated
  symlink's target is never modified. Read and synchronization failures are
  reported rather than treated as successful publication.
- Publication retains the existing ordering: requested metadata/file sync,
  atomic rename, destination-parent sync, checkpoint-directory sync, sidecar
  deletion, checkpoint-directory sync. `--no-fsync` retains its existing
  weaker durability contract. An error after rename can leave a completed
  destination; a later retry validates it before finishing cleanup.

Nonempty partials with missing, malformed or mismatched source state, and
partials whose retained data differs from the source, still fail closed. They
cannot safely be attributed to the requested transfer. TurboCopy leaves them
for operator inspection/removal rather than deleting potentially unrelated
bytes. Once cleanup is finished, another ordinary `--resume` invocation is a
new copy request; avoiding that copy requires the existing `--sync --checksum`.

## Verification

Test-only child processes exit immediately at 24 deterministic boundaries:
initial state write/sync and directory sync, partial creation, before/during/
after data writes, metadata application, file sync, rename, publication directory
syncs and checkpoint cleanup/sync. Exiting bypasses Rust destructors. The tests
also repeatedly interrupt recovery sync/cleanup, verify unchanged destination
inodes for completed publication, validate the retained offset after a partial
write, repair truncated initial sidecars, resume read-only partials, and protect
wrong same-size destinations and unrelated symlink targets. The existing tests
cover corrupt retained bytes, changed sources, locks, hardlinks, symlinked state,
interruption, failed writes and destination preservation.

`cargo test`: 53 unit tests, 35 copy integration tests and 4 extended-metadata
integration tests passed. Existing native transport tests require localhost
socket access outside the restricted sandbox. Formatting and strict all-target
Clippy passed.

These are process-crash tests on the available local filesystem, not physical
power-cut or filesystem-journal replay tests. Retained bytes are best effort and
always verified again; filesystem/hardware fsync behavior remains a prerequisite
for power-loss durability. No new GPFS/NFS or filesystem-image validation is
claimed for this change.
