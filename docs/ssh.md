# Native SSH copying

SSH transfers use TurboCopy's Rust agents, bounded file workers, and positioned
chunk I/O. OpenSSH establishes authenticated tunnels only; rsync and scp are
never invoked. No file data is staged on the coordinating machine.

## Requirements and routing

Install a compatible `tcopy` on every remote host. `[user@]host:path` selects
a remote endpoint; bracket IPv6 addresses and use SSH configuration aliases for
ports, keys, and jump hosts. `--ssh-program PATH` selects the remote executable
(default `tcopy`). Paths are literal, relative to the remote login directory
unless absolute. Prefix local names containing colons with `./`.

Each remote agent binds a temporary loopback TCP listener. The coordinator
opens an SSH local forward to that listener and connects native worker streams
through it. A random session capability authenticates each stream. Source and
destination tunnels have independent local ports: using the same local port
would collide. Two remote hosts need access only from the coordinator, not to
each other. SSH host-key checks and SSH configuration remain effective; no
passwords are stored and no agent forwarding is requested.

## Protocol compatibility

The current wire version is 2 (`TCOPY\0\0\x02`). Coordinator and agents must
use exactly the same wire version; there is no negotiation or fallback.
The agent startup response and every worker connection check this version
before processing operations. An incompatible peer fails the transfer.
Use the same TurboCopy release on all hosts; package version alone does not
establish compatibility. Changes to framing, operation meanings or metadata
encoding require a new wire version. Compatible fixes keep the wire version.
Version 2 adds the validated metadata-only synchronization operation; version 1
peers are incompatible. Upgrade the coordinator and all participating agents together.
Unknown operations, oversized fields and invalid metadata terminate the
affected session after an error response where possible. Incomplete frames
fail on EOF or the active I/O timeout. Uncommitted stages are removed when
their owning session closes; already published files remain in place.

## Examples

```bash
tcopy ./source/ user@host:/data/destination/ --auto
tcopy user@host:/data/source/ ./destination/ --checksum --sync
tcopy host-a:/data/source/ host-b:/data/destination/ --workers 4
tcopy ./source/ host:/data/destination/ --ssh-program /opt/tcopy/bin/tcopy
```

Use `--dry-run` to check routing and destination mapping first. A failed copy
returns a nonzero exit status; retry normally to overwrite files, or use
`--sync --checksum` to skip contents whose bytes already match. Sync still
reconciles mode and mtime without transferring those contents; see the
[metadata synchronization contract](metadata-sync.md). Remote resume
checkpoints are unsupported.

## Preservation and failure behavior

- Local-to-remote, remote-to-local, remote-to-remote, and mixed multiple sources
  use the same root mapping as local copies, including trailing slashes and `/.`.
- Native streaming traversal, bounded job queues, and a shared concurrency
  budget limit memory and active file/range transfers. Large files use parallel
  chunks; each range streams through bounded buffers with no per-buffer RPC.
  Workers reuse connections, and directory metadata arrives in batches of at
  most 64 entries. Default remote concurrency is at most four; automatic chunks
  are 16 MiB and large-file parallelism starts at 64 MiB.
- Retained directory descriptors anchor destination operations. Refuse
  directory symlinks, special files, overlapping same-machine trees, and
  colliding destination roots.
- Stages are private exclusive temporary files. Validate exact chunk coverage,
  source identity/size/mtime/ctime, and source read completion before publishing.
  Apply modes/mtime and fsync before atomic rename, then fsync the parent.
  A failed or incomplete stage cannot replace an existing destination.
- Clean up stages and stop agents/tunnels on completion, failure, or cancellation.
  Abrupt machine failure may leave identifiable `.tcopy-*.tmp` files. A directory
  copy is not transactional, and source trees should remain stable.
- `--sync`, byte-comparison `--checksum`, `--dry-run`, `--workers`, `--chunk-size`,
  `--sparse`, `--progress`, `--json`, and `--no-fsync` work with SSH. Sparse mode
  omits writing all-zero buffers to a new stage. Unsupported advanced options
  fail explicitly: resume checkpoints, reflinks, extended owner/xattr/ACL
  preservation, active tuning, and the GPFS profile remain local-only.
- Reports identify the native SSH backend. Hardware information describes the
  coordinator; remote transfers use conservative fixed concurrency rather than
  pretending local storage detection describes remote disks.
- SSH authenticates/encrypts every remote data stream. The private protocol
  has a version handshake, bounded fields/ranges, capability checks, and explicit
  completion/error acknowledgements. Active I/O has a bounded timeout.

Active I/O and startup have 30-second timeouts. Idle metadata connections stay
open while file workers run. SSH keepalives detect lost peers; cancellation is
checked between buffer operations. Published files remain valid after a later
transfer fails. Resume, extended metadata and reflink support are explicit
future additions rather than silent fallbacks.
Each tunnel has its own SSH connection; configured persistent multiplexing and
agent forwarding are disabled so cleanup cannot leave shared forwards behind.

Run verification with `cargo test --locked`, `cargo fmt --check`,
`cargo clippy --locked --all-targets -- -D warnings`, then `cargo build --locked`
and `python3 -m unittest discover -s tests -p 'test_native_ssh.py'`. The SSH tests
require OpenSSH client/server and temporary loopback socket permissions.

## Acceptance checks

A local measurement on 2026-10-12 copied 500 files of 37 bytes between two
private loopback SSH servers through the coordinator, using the debug build,
four workers, default fsync, and warm caches. Three interleaved runs with byte
verification gave median times of 12.79 seconds before metadata buffering and
2.03 seconds after it; both versions reused worker connections and batched
directory entries. This measures small-file protocol overhead on this machine,
not WAN throughput or a comparison with other copy tools.

Exercise real SSH upload, download, and a two-host route through the coordinator;
check bytes, empty directories, links, mode/mtime, atomic replacement, parallel
chunks, sync/checksum, sparse files, dry-run and JSON. Test malicious path text,
malformed endpoints/frames, symlink destinations, overlapping trees, missing
agents, truncated streams, incomplete/duplicate chunks, source changes, and
cancellation. Keep the existing local test suite green.

```bash
tcopy --workers 8 ./data alice@server:/backup/data
tcopy alice@server:/data ./backup
tcopy --workers 8 --chunk-size 16M alice@source:/data bob@destination:/backup
tcopy --sync --checksum alice@server:/data/. ./backup/
```
