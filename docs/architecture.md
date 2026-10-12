# Architecture

`main → cli / scanner::plan → scheduler::run → scanner::scan → copy_engine → fsutil`

The scanner plans source/destination roots before any mutation. An iterative
depth-first walk holds one directory iterator per depth and sends file/link
tasks through a bounded standard-library channel. Workers share the receiver
only while taking a task; data copying happens concurrently. Counters are
atomic, and errors are printed as they happen instead of accumulating in RAM.

Directory exits insert a scheduler barrier: all queued descendants finish
before directory permissions and mtimes are restored. This keeps memory
bounded by depth plus queue capacity, without storing all directory metadata.
The barrier trades some parallelism between sibling directories for simple,
correct metadata handling. Existing destination directory permissions are
retained during copying and replaced on successful directory finalization.

Destination directories are opened one component at a time with
`O_DIRECTORY | O_NOFOLLOW`. Tasks retain shared directory handles. Operations
through `/proc/self/fd/<descriptor>/<name>` remain anchored to those handles,
including if a path component is replaced by a symlink after opening. A final
destination symlink is replaced by rename, never opened for writing.
Directory ancestry also compares device/inode identity, catching overlap through
existing bind-mount aliases that ordinary path-prefix checks would miss.

Regular source files are opened without following their final symlink and
with nonblocking mode, then validated as regular files. Copying uses bounded
buffered reads and `write_all`, retries interrupted reads, and checks cancellation
between buffers. No reflinks, copy-offload syscalls or chunk scheduling are used.
The source's observed length limits the read so continuous growth cannot make
the copy unbounded. File identity, size, mtime and ctime are checked before
publication. Atomic rename happens only after metadata and file fsync succeed;
parent fsync completes durability where supported.

See [Phase 1 contract](phase1.md), [decisions](decisions/001-correctness-first.md)
and [benchmark methodology](performance.md) for acceptance criteria and limits.

The implementation uses stable [Rust FileTimes](https://doc.rust-lang.org/std/fs/struct.FileTimes.html)
and [bounded channels](https://doc.rust-lang.org/std/sync/mpsc/fn.sync_channel.html).
Linux open flags differ on [AArch64](https://github.com/torvalds/linux/blob/master/arch/arm64/include/uapi/asm/fcntl.h);
`fsutil` keeps those ABI differences explicit.
