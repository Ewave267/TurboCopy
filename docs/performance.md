# Benchmark methodology and Phase 1 results

Run a release binary with Python 3.11+:

```bash
cargo build --release
python3 benches/compare.py --binary target/release/tcopy --directory . \
  --runs 3 --output /tmp/tcopy-benchmark.json
```

The harness owns a temporary directory under the selected filesystem and deletes
only that dataset afterward. Its default dataset has 1,000 random 4 KiB files,
16 files of 1 MiB, one 64 MiB file, and one empty directory: 1,017 files totaling
83.90625 MiB. Larger files repeat a seeded random block; note this when testing
compressed/deduplicated storage. Override `--small-files` and `--large-mib` to
test other mixes. Select source/destination storage pairs in future benchmark
work; this harness measures one filesystem per run.

Every tool receives the same source basename and destination layout. Commands:

- `cp -rp --reflink=never --sparse=never SOURCE DESTINATION`
- `rsync -rltp SOURCE DESTINATION`
- `tcopy SOURCE DESTINATION`
- `tcopy --sync SOURCE DESTINATION`

All paths preserve permissions and mtimes. Ownership is identical for this
single-user dataset; ACLs, extended attributes and hard links are not included.
Reflinks and sparse expansion shortcuts are disabled for `cp`. There is no
`--auto` comparison because hardware adaptation is not implemented in Phase 1.

Read the source before each sample to measure **warm-cache** runs. The harness
never drops global caches or issues destructive device writes. Randomize tool
order with a fixed seed, collect three samples per scenario/tool, and report
medians. Separate first-copy from unchanged-incremental runs. Default `tcopy`
and `cp` recopy; `tcopy --sync` skips by size, while `rsync` uses its ordinary
size/time quick check. Changed incremental data, same-size modifications and
cold-cache runs require separate experiments.

The clock covers tool execution plus a final file/directory fsync of the entire
destination for every tool, including skipped files. This supplies comparable
end-of-run durability but also charges incremental copies for flushing the
whole tree. TurboCopy additionally fsyncs every file before publication and each
parent after rename; its per-file atomicity and intermediate durability are
stronger than these baseline commands. This difference is deliberate and must
remain visible in any comparison. No instantaneous or copied-byte throughput is
inferred from an unchanged-incremental run.

Outside the timed interval, verify SHA-256 contents, permission bits and
nanosecond mtimes for every file and directory. Output retains individual
samples, commands, tool/compiler versions, filesystem and host information.

## Phase 1 measurement

Results are recorded in [benchmark-phase1.json](benchmark-phase1.json).
On 2026-10-11, Linux x86-64 with eight logical CPUs, Rust 1.95.0, and the
project's Btrfs filesystem, the final release binary produced these medians:

| Tool | First copy (seconds) | Unchanged incremental (seconds) |
| --- | ---: | ---: |
| `tcopy` | 1.886 | 1.926 |
| `tcopy --sync` | 1.875 | 0.968 |
| `cp` | 1.324 | 1.119 |
| `rsync` | 1.447 | 0.969 |

All 24 benchmark samples passed the content and metadata checks. TurboCopy
was slower on the initial copy in this workload. Size-based sync skipped all
1,017 files on unchanged incremental runs; default copying transferred all
87,982,080 logical bytes again. Timings include the common durability flush
described above and are not isolated transfer throughput measurements.

The measurement is a small local regression baseline, not evidence of optimal
throughput or a general speed advantage. HDD, separate NVMe devices, NFS,
large-scale metadata workloads and cold-cache performance remain unmeasured.

Per-file syncing and post-order directory barriers are the main intentional
tradeoffs. They make failures and metadata ordering straightforward, at a cost
to throughput and parallelism. Phase 2 can investigate alternative copy syscalls
and scheduling with this correctness suite as its regression gate.
