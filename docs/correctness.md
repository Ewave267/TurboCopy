# Filesystem correctness validation

Validated locally on 2026-10-12. Local integration tests use `TMPDIR` for their
owned temporary trees, so the same checks can be repeated on a writable mount.

| Check | Btrfs | tmpfs | Contract |
| --- | --- | --- | --- |
| Source hard links | passed | passed | Contents copied independently; hard-link topology is not preserved. |
| Destination hard-link aliases | passed | passed | Atomic replacement leaves other aliases intact. |
| File/directory ACLs and user xattrs | passed | passed | Use `--preserve-extended`; verify access/default directory ACLs and inherited child ACLs. |
| Read-only source and destination directories | passed | passed | Restore source directory mode after children; failure to create a stage preserves existing data. |
| Raw Unix names, newlines, tabs, colons, quotes and leading dashes | passed | passed | Copy literal bytes without shell interpretation. |
| Destination parent replacement races | passed | passed | Retained directory descriptors prevent a symlink swap from redirecting writes. |
| Write-limit failures | passed | passed | Existing destination survives; temporary stages are removed. |
| Actual ENOSPC | passed on disposable image | passed on isolated 1 MiB mount | Existing destination survives and temporary stages are removed; tmpfs covers buffered, manual chunks and GPFS profile. |

The Rust transfer unit test also injects ENOSPC and short writes. File-size
limits produce EFBIG and are separate from a real full-filesystem test.
Symlink xattrs are checked only where the filesystem permits them; the
existing test returns early on unsupported setups. ACL tools must be present.
The completed [filesystem-image results](filesystem-validation-current.json)
record all 35 copy and four phase4 integration tests passing on each of ext4,
XFS, Btrfs and F2FS. Each filesystem also passed a real ENOSPC check with the
default copy settings: the existing destination survived and temporary stages
were removed. The user ran these checks on disposable 2 GiB images in a private
mount namespace; these are correctness checks, not performance measurements.
The initial 512 MiB image was too small for the chunk test's retained copies;
its [failed-run output](filesystem-validation-512m-failed.json) is preserved.
GPFS and NFS correctness testing remains deferred.

```bash
cargo build --locked
cargo test --locked -- --test-threads=1
TMPDIR=/path/to/writable/mount cargo test --locked --test copy --test phase4 -- --test-threads=1
python3 -m unittest discover -s tests -p 'test_low_space.py'
python3 -m unittest discover -s tests -p 'test_native_ssh.py'
```

The low-space test needs Linux user/mount namespaces and `mount`/`umount`.
It mounts size-limited tmpfs inside a private namespace, unmounts it in a
`finally` block, and removes only its own temporary directory. It fails if the
environment cannot provide the namespace; it does not silently skip ENOSPC.
The SSH suite requires OpenSSH and permission to bind temporary loopback
sockets. It checks upload/download tunnel disconnection, cancellation,
byte integrity and cleanup. Native agent tests also reject incompatible wire
versions, oversized requests, unknown operations and truncated transfers.
