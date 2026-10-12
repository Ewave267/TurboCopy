# Phase 2 specification

Implement the Performance phase in `project.md`, retaining Phase 1's atomic
replacement, metadata, cancellation and destination-symlink protections.
Rust 1.75+, Linux x86-64/AArch64 and the standard library remain the stack.

## Acceptance criteria

| Capability | Observable result |
| --- | --- |
| CLI | `--workers N` accepts 1–128; `--chunk-size SIZE` accepts 1 KiB–64 MiB, with K/M/G or KiB/MiB/GiB binary suffixes. Missing, overflowing and invalid values fail before mutation. |
| Storage | Report filesystem, mount device, SSD/NVMe/HDD/memory/unknown classification and underlying block identities using mountinfo/sysfs. Decode escaped mount paths, prefer longest matching mount, traverse mapper/partition topology, and report unknown when detection is unavailable. Distinguish shared filesystem from detectable shared physical storage. |
| Chunk copying | Explicit `--chunk-size` enables range copying for files at least 64 MiB with at least two chunks and more than one worker. Size the temporary destination first; copy disjoint ranges with positioned reads/writes. Join all helpers and revalidate source before metadata, fsync and atomic publication. |
| Conservative paths | Keep default buffered copying and at most four default file workers. Use streaming for small files, HDDs, network/unsupported filesystems, or unavailable helper slots. Unsupported positioned I/O falls back only after helpers stop; ENOSPC/EIO/permission failures remain errors. |
| Bounds | Retain the 64-entry file queue and 128 KiB buffers. A shared permit budget limits simultaneous file/chunk work to `--workers`, including the parent file task. Helper allocation does not scale with file length or chunk size. |
| Metrics | Report committed files/bytes, skips, failures, attempted transfer bytes, elapsed seconds, effective committed-byte throughput, busy worker-slot utilization, peak active slots, and accumulated metadata/transfer durations. Distinguish attempted bytes from published bytes. |
| Progress | `--progress` prints bounded-frequency live updates to stderr, including bytes within large files. Streaming traversal means totals/ETA are unknown during discovery; do not invent them. |
| JSON | `--json` emits one valid final JSON object to stdout, including configuration, storage and metrics on success, failure and cancellation. Dry-run action lines go to stderr in this mode. Arbitrary filenames cannot inject JSON. |
| Benchmarks | Compare buffered, pread/pwrite, sendfile and copy_file_range on owned temporary files, verify contents/metadata, include fsync and warm-cache methodology, and identify possible copy_file_range cloning. Extend the existing CLI benchmark with an explicit chunk configuration. Do not automatically select an experimental syscall. |
| Regression | Test numeric flags, storage parsing/topology, byte-exact partial-last-chunk copies, shared concurrency bounds, rollback/cancellation/source changes in the chunk path, progress and JSON. Retain all Phase 1 tests. |

## Implementation order and scope

1. Validated CLI controls and storage detection with focused tests.
2. Shared concurrency budget and safe positioned chunk engine with regressions.
3. Progress/JSON reporting and operation timings.
4. Release build, benchmarks, documentation, review and refreshed Graphify index.

Changes reach the CLI/main, task/scanner/copy-engine/scheduler, tests,
benchmark harness and README/design documents. New modules are
`storage_detect.rs` and `progress.rs`. No new Rust dependencies or changes to
existing copy destination mapping are needed.

## Assumptions and tradeoffs

Chunking is a manual opt-in here; automatic worker/chunk choice and active tuning
belong to Phase 3. CPU/RAM/pressure tuning is likewise deferred. Forcing additional
workers can reduce performance. Idle file-pool threads may coexist with short-lived
chunk helper threads, but active permits and allocated buffers stay bounded.
Directory finalization barriers remain for metadata correctness.

Storage detection is advisory and fails conservatively. Mapper/partition backing
information may be hidden; unknown does not mean independent storage. Physical
classification is not a benchmark. Network/HDD guards override manual chunk size;
the file-worker override remains effective. Local filesystem detection allows
explicit chunk requests on otherwise unknown devices.

`copy_file_range` may perform storage offload or cloning. It is benchmarked as an
experiment, never used implicitly in production copying. Syscall benchmarks include
Python overhead and are not direct measurements of the Rust engine.

Timers accumulate operation wall time across workers and traversal, not CPU time, and can exceed elapsed
time under concurrency. Utilization measures occupied permit time, including
waiting for a file's chunk helpers and durability operations. Failed attempts may
increase transferred bytes without increasing committed bytes.

## Verification commands

`cargo test`, `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo build --release`,
`python3 benches/compare.py --directory . --output docs/benchmark-phase2.json`,
`python3 benches/io_methods.py --directory . --output docs/benchmark-io-phase2.json`.
Retain honest limitations for untested hardware, NFS and real device exhaustion.

## Completed verification

Phase 2 retains all 21 Phase 1 integration cases, extends cancellation,
source-change, parent-swap and write-limit cases to chunk configurations, and
adds two integration cases plus focused unit checks (34 tests total).
Formatting, strict Clippy and release build pass on Rust 1.95.0/x86-64.

The mixed-tree and single-large-file comparisons each verify 36 samples; the
syscall experiment verifies 12. Results and limits are in [performance.md](performance.md).
A separate 1 GiB smoke copy observed live transfer updates before publication.
Unknown backing devices in this sandbox and multi-device Btrfs remain advisory
limitations. No automatic tuning or implicit reflinks were introduced.
