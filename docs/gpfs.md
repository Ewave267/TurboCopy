# GPFS bulk copying and measurement

The GPFS profile removes three application-side obstacles to overlapping remote
I/O: a small CPU-bounded worker ceiling, local PSI-driven admission reductions,
and a global drain at every directory exit. It also enables positioned chunking
on native `gpfs` mounts and uses 1 MiB transfer buffers. These changes are locally
tested behavior, not measured GPFS speedups. Neither Storage Scale topology nor
cluster configuration can be inferred from this development filesystem.

## Copy commands

```bash
cargo build --release
./target/release/tcopy /gpfs/source/. /gpfs/destination/ --gpfs --no-fsync --workers=32 --progress --json
```

The destination directory must already exist when using `source/.` to copy its
contents. Use `source` without `/.` to retain ordinary basename mapping.
Keep sources stable. Existing destination files are atomically replaced;
extra destination entries are retained. Add `--preserve-extended` if owner/group,
xattrs and ACL preservation is required; preservation failures are copy errors.

For many small files, compare fixed worker counts first. Directory batching
allows work from up to 64 completed directories to overlap before metadata
finalization, and the file queue remains capped at 64 entries. Traversal still
uses one scanner; metadata discovery can become the next bottleneck. The tool
never builds a million-entry manifest in memory during normal copying.

For a few large files, the profile uses 16 MiB ranges on files at least 64 MiB;
`--chunk-size=64M` is available for comparison. Chunks stream through 1 MiB
buffers, so chunk size does not determine allocation size. Helpers use idle
slots rather than oversubscribing file workers. Parallel ranges are a testable
starting point: their benefit depends on the cluster and workload, and should
not be assumed for same-filesystem copies. NFS mounts remain excluded from
chunking even when their server uses GPFS.

## Atomicity and durability are separate

Default copying retains both atomic replacement and explicit per-file/directory
durability barriers. `--no-fsync` skips the barriers, including mkdir-parent and
directory-metadata synchronization, but retains temporary-file staging and
atomic rename. It does not issue an end-of-run flush. Detected transfer failures
before publication preserve the previous destination. Delayed writeback failures
may surface after publication; successful completion does not promise recovery
after a node crash or power loss. Resume requires the default durability mode.

In-place overwrites are deliberately not introduced: they would expose partial
files and destroy old destinations on failure. If GPFS measurements show rename
or temporary creation dominates after removing fsync waits, an explicit in-place
mode would need its own failure contract and tests.

## Compare on the cluster

The existing benchmark now accepts `--gpfs`. It compares default tcopy, the
durable GPFS profile at 32 workers, GPFS without fsync at 8/16/32/64 workers,
GNU cp and rsync when installed. It creates and removes its own temporary
dataset under the supplied parent and verifies contents, modes and mtimes.

```bash
# Start small before scaling the number of files.
python3 benches/compare.py --directory /gpfs/benchmark-parent --gpfs \
  --runs 3 --small-files 10000 --medium-files 0 --large-mib 1 \
  --output /tmp/gpfs-small.json

# One large file exercises positioned chunking.
python3 benches/compare.py --directory /gpfs/benchmark-parent --gpfs \
  --runs 3 --small-files 0 --medium-files 0 --large-mib 10240 \
  --output /tmp/gpfs-large.json
```

This harness is a warm-cache, same-filesystem comparison with a common final
file/directory fsync timed for every tool. That makes the durability endpoint
comparable even when a tool skips intermediate barriers. Its large files repeat
a seeded random block, and it keeps verification manifests in memory; it is a
small comparison harness, not a memory-bounded million-file benchmark. Use the
normal copying CLI on representative production-sized trees, with separate
external verification, to measure scale. Do not infer cold-storage or
cross-filesystem performance from this harness.

Record client CPU/memory, native mount type, competing jobs, file-size/tree shape,
worker/chunk counts and durability mode. IBM exposes client create/open/rename,
read/write and fsync counts/latencies through
[Storage Scale performance metrics](https://www.ibm.com/docs/en/storage-scale/6.0.0?topic=gui-performance-metrics-available-in).
Those metrics help distinguish discovery, metadata, data-transfer and durability
waits. Local PSI alone cannot establish whether remote cluster capacity is full.

Choose worker counts from actual throughput and cluster impact. Fixed manual
workers bypass the profile's automatic memory ceiling; at 32 workers, transfer
buffers occupy at most 63 MiB, plus thread stacks, retained directory handles,
filesystem caches and optional metadata. The profile does not impose byte-rate
limits, spread work across clients, or alter GPFS settings.
