# Architecture

`main → cli / scanner::plan → scheduler::run → scanner::scan → copy_engine → fsutil / metadata / sparse / resume`

The scanner plans source/destination roots before any mutation. An iterative
depth-first walk holds one directory iterator per depth and sends file/link
tasks through a bounded standard-library channel. Workers share the receiver
only while taking a task; data copying happens concurrently. Counters are
atomic, and errors are printed as they happen instead of accumulating in RAM.

By default directory exits insert a scheduler barrier: all queued descendants finish
before directory permissions and mtimes are restored. This keeps memory
bounded by depth plus queue capacity, without storing all directory metadata.
The barrier trades some parallelism between sibling directories for simple,
correct metadata handling. Existing destination directory permissions are
retained during copying and replaced on successful directory finalization.
The explicit GPFS profile accumulates up to 64 completed frames before draining
the pool and restoring metadata in post-order. This bounds retained handles while
allowing file work from sibling directories to overlap.

Destination directories are opened one component at a time with
`O_DIRECTORY | O_NOFOLLOW`. Tasks retain shared directory handles. Operations
through `/proc/self/fd/<descriptor>/<name>` remain anchored to those handles,
including if a path component is replaced by a symlink after opening. A final
destination symlink is replaced by rename, never opened for writing.
Directory ancestry also compares device/inode identity, catching overlap through
existing bind-mount aliases that ordinary path-prefix checks would miss.

Regular source files are opened without following their final symlink and
with nonblocking mode, then validated as regular files. Copying uses bounded
buffered reads and partial-write loops, retries interrupted I/O, and checks
cancellation between operations. `--reflink` explicitly tries `FICLONE` and
falls back on unsupported clone errors; `--sparse` uses extent seeking where
available. The ordinary default uses neither.
The source's observed length limits the read so continuous growth cannot make
the copy unbounded. File identity, size, mtime and ctime are checked before
publication. Atomic rename happens only after metadata and file fsync succeed;
parent fsync completes durability where supported. Explicit `--no-fsync` skips
these barriers and directory creation/finalization barriers while retaining
atomic publication. It provides no crash durability promise and forbids resume.

`--resume` stages sequential regular-file data under a private destination
checkpoint directory. Its source identity sidecar and full retained-prefix
comparison prevent stale or corrupt partials from being published. The
checkpoint file is locked during use and publication still uses atomic rename.
`metadata` applies basic mode/mtime or, with `--preserve-extended`, owner/group
and xattrs including ACLs. Directory metadata is finalized after descendants.

Explicit chunk controls add positioned [Rust FileExt I/O](https://doc.rust-lang.org/std/os/unix/fs/trait.FileExt.html)
for eligible large local files. The parent worker reserves idle permits, sizes
the temporary file and runs ranges alongside scoped helpers. An atomic chunk
index assigns non-overlapping offsets; each range streams through 128 KiB
(1 MiB with the GPFS profile). Native GPFS mounts also permit explicit chunks.
All helper threads stop and join before validation/publication or an unsupported
positioned-I/O fallback. Data, space, permission and cancellation errors fail
the copy. A shared permit budget bounds active file and chunk work; helpers
never wait for additional permits, avoiding nested scheduling deadlocks.

`storage_detect` reads the process mount namespace and resolves partition/mapper
backing identities through sysfs. Detection is read-only and conservative;
unknown topology stays unknown. `progress` owns atomic counters, operation
timers, JSON escaping and the final report. A scoped reporter reads live counters
at 250 ms intervals and exits when traversal/queued work finish. JSON dry-run
previews are sent to stderr so stdout contains one object.

`hardware` reads CPU topology, available RAM, visible cgroup v2 ancestor limits,
and CPU/I/O pressure. `tuner::select` combines these with the root storage pairs
before the pool starts. Automatic workers have a conservative ceiling of eight,
also bounded by CPU and buffer budgets. `copy_engine` selects automatic range
sizes from each eligible file's length and rechecks its storage pair, including
mounted subdirectories. Explicit worker/chunk overrides retain precedence.

The same scoped reporter performs opt-in calibration and pressure feedback.
It samples attempted bytes, committed files and slot occupancy once per second;
eight windows bound calibration. `Concurrency::resize` changes admission under
the permit mutex, wakes waiters on growth and lets existing work drain on shrink.
The thread count, queue capacity and buffer ceiling stay fixed. Helpers never
wait for additional permits, so reducing admission does not add nested deadlocks.
Workload counters classify regular files during streaming discovery, including
sync skips and dry-run previews. Reports distinguish the configured worker
ceiling from the final admission limit and expose insufficient calibration data.
The root chunk-size hint can differ from automatically selected per-file sizes.

`--gpfs` is a separate fixed profile: memory-bounded 32 slots, 16 MiB chunks,
1 MiB buffers and no local CPU/PSI admission policy. Manual worker/chunk controls
take precedence. It forbids auto/tune rather than mixing the two policies.

See [Phase 1 contract](phase1.md), [Phase 2 contract](phase2.md), [Phase 3 contract](phase3.md), [Phase 4 contract](phase4.md), [adaptation decision](decisions/003-workload-adaptation.md), [Phase 4 decision](decisions/004-phase4-opt-ins.md), [decisions](decisions/001-correctness-first.md)
and [benchmark methodology](performance.md) for acceptance criteria and limits.

The implementation uses stable [Rust FileTimes](https://doc.rust-lang.org/std/fs/struct.FileTimes.html)
and [bounded channels](https://doc.rust-lang.org/std/sync/mpsc/fn.sync_channel.html).
Linux open flags differ on [AArch64](https://github.com/torvalds/linux/blob/master/arch/arm64/include/uapi/asm/fcntl.h);
`fsutil` keeps those ABI differences explicit.

Storage sources: [mountinfo format](https://www.man7.org/linux/man-pages/man5/proc_pid_mountinfo.5.html)
and [kernel block queue attributes](https://www.kernel.org/doc/html/v5.9/block/queue-sysfs.html).
The separate syscall benchmark accounts for [copy_file_range acceleration](https://www.man7.org/linux/man-pages/man2/copy_file_range.2.html),
which can use reflinks and is unsuitable as an implicit ordinary-copy fallback.
Btrfs can return a subvolume-specific [anonymous device ID from getattr](https://github.com/torvalds/linux/blob/master/fs/btrfs/inode.c).
Detection verifies the mount-point stat device before accepting this mountinfo
mismatch. Unmounted nested subvolumes may remain unknown and use streaming.
