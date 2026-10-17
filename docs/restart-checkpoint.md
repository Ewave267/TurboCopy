# Current work — 2026-10-17

| Task | Status | Scope and completion criteria |
| --- | --- | --- |
| 1. Crash-recoverable resume | Completed | Reproduce the publication crash before changing code; review initialization, validation, metadata, publication and cleanup; make retries idempotent while protecting unrelated data and preserving atomic rename/fsync; test deterministic and repeated interruptions; document guarantees and commit independently. See [recovery details](resume-recovery.md). |
| 2. Metadata synchronization | Queued | Reproduce identical-content skips locally and over SSH; define the sync/checksum/preservation contract; reconcile requested mode, mtime and supported ownership/ACL/xattrs without rewriting contents; propagate failures; test and document local/remote behavior; commit independently. |
| 3. Automatic worker selection | Queued | Benchmark current HEAD before changes; identify the actual auto/manual bottleneck; implement a small bounded policy change only if measurements justify it, preserving manual settings and short-transfer behavior. Compare large, multiple-large, small and mixed workloads against manual TurboCopy, cp and rsync, using existing infrastructure and available same/separate devices; record medians, variability, workers, CPU/RAM and correctness/durability conditions. GPFS/NFS requires authorized accessible mounts. Test, document before/after results and limitations, and commit independently. |

Only Task 1 was authorized for implementation in this work session. Tasks 2 and
3 remain queued; earlier benchmark results below are historical, not Task 3's
required current-HEAD baseline.

# Restart checkpoint — 2026-10-12

User asked to stop and save progress, then offered to run the benchmark for later
analysis. No commits were created. All changes remain in the working tree.

Update: the user supplied completed run `run-20261013T015836Z-8Kq6h4`.
All 192 copies verified. Its exact JSON, report and four graphs are now retained
as docs/benchmark-phase5-20261013T015836Z.*, and docs/performance.md compares it
with both historical runs. Automatic workers stayed 4 → 4; classification was
correct. Benchmark analysis and filesystem-image validation are finished.
The original partial-run and pending-validation notes below remain historical;
only GPFS/NFS remains deferred.

## Historical benchmark handoff (completed)

```bash
cd /home/ewave/projects/turboCopy
bash tools/benchmark.sh
```

The script builds the release binary and runs all original Phase 5 workloads,
three iterations, cp/rsync/automatic/manual worker comparisons and empty-file
companions. Results go to a unique `target/performance/run-*` directory, including
`benchmark.json`, `benchmark.md`, SVGs, `run.log` and `exit-status`. Run without
other storage-heavy work. Later provide that directory to the assistant and ask
it to compare with both historical Phase 5 reports and update docs/performance.md.
Require JSON status `completed` before claiming a full rebenchmark.

## 1. Automatic-mode rebenchmark: partially completed

Saved docs/benchmark-phase5-current.json and .md plus three SVGs. Cleanly stopped
with SIGTERM at the user's request: status `stopped`, 157 samples, 156 verified
(84 dense, 72 metadata-only), one aborted. Owned temporary trees were cleaned up.
The harness has no resume support; keep these artifacts and run a fresh full run.

All three iterations completed for the first three workloads. Automatic medians:

| Workload | Durable MiB/s |
| --- | ---: |
| 1 × 10 GiB | 670.2 |
| 10 × 1 GiB | 692.9 |
| 100 × 100 MiB | 648.3 |

10,000 × 1 MiB has only two automatic samples (230.2 and 222.0 MiB/s).
Do not present it as a complete three-run group. Every measured automatic dense
copy selected and retained four workers. Classification correctly reported
Btrfs/NVMe, complete topology, shared storage, backing device `259:4`; independently
checked against mount inventory and `/sys/class/block/nvme0n1/{dev,queue/rotational}`.
Other validation ran concurrently during parts of this partial benchmark;
interpret differences cautiously. The measured release binary was built before
the SSH-only cleanup fix, which does not change local automatic copying.

benches/phase5.py now reports per-sample automatic workers, storage kinds,
topology completeness and filesystem-type agreement with independent inventory.
Removed obsolete report wording about pressure throttling. Added a report test
covering both correct classification and a mismatch. The partial report was
regenerated with this updated reporting code after stopping.

Historical comparisons: docs/benchmark-phase5.json and
docs/benchmark-phase5-20261012T213109Z.json. Existing docs/performance.md has both
historical comparisons; it has not been updated with incomplete current results.

## 2. GPFS/NFS: explicitly deferred

User said to defer these and continue the rest. No GPFS/NFS data mounts are
available. Recorded in docs/gpfs.md. Resume with actual writable mounts or SSH
hosts. The Phase 5 harness currently rejects network mounts; adapt its device
accounting/cache controls before using it there. Classifier fixtures do not
establish network performance or filesystem correctness.

## 3. SSH hardening: completed and tested

Added live version-mismatch, unknown-operation and oversized-frame tests, plus
metadata truncation/out-of-range tests and completed-stage owner-disconnect
coverage. Added real SSH upload/download tunnel-kill tests. Those reproduced a
temporary-file cleanup bug: Stage dropped its retained directory descriptor
before its Temporary could unlink through `/proc/self/fd`. Fixed by moving
temporary ahead of parent in src/remote/agent.rs, with a lifetime comment.
The tunnel-kill test failed before this fix and passed afterward.

docs/ssh.md now has exact wire-version compatibility rules and upload/download/
relay examples, retry behavior and failure semantics. Current wire version is 1;
no negotiation or fallback. Compatible cleanup fix does not require a bump.

Validation: complete Rust suite passed after the fix (45 unit, 35 copy, 4 phase4
tests). One additional wire test was then added and all three wire tests passed.
Eight real SSH integration tests passed, including the new disconnect test.
Native socket tests require execution outside the sandbox. Formatting, clippy
with warnings denied and diff checks passed.

## 4. Filesystem correctness: available filesystems validated

Added source-hardlink/raw-filename/read-only-tree and read-only-destination
checks. Extended ACL checks to directory access/default ACLs and inherited child
ACLs. The local integration suite (35 copy + 4 phase4) passed on tmpfs and Btrfs;
extended ACL tests were rerun successfully on both after adding directory ACLs.
Existing destination alias, xattr and destination-parent race checks also passed.
Source hard-link topology is not preserved: contents are copied independently.

tests/test_low_space.py uses a private user/mount namespace and a 1 MiB tmpfs;
real ENOSPC tests passed for buffered, manual chunks and GPFS-profile copying.
Existing data survived and stages were removed. Six benchmark harness tests
passed. docs/correctness.md records coverage, limits and reproduction commands;
README links it. Existing SSH CI invocation includes the new disconnect test.

Actual ext4, XFS and F2FS tests and Btrfs ENOSPC remain pending. Tools for all
four filesystem image formats are installed. A disposable-image runner was
prepared at /tmp/tcopy-fs-matrix.py; not executed because `sudo -n` requires a
password. If it still exists, review then run interactively:

```bash
sudo unshare --mount --propagation private python3 /tmp/tcopy-fs-matrix.py
```

It formats only owned temporary 2 GiB image files, mounts them privately,
runs local Rust integration tests as ewave, fills each test filesystem and checks
ENOSPC preserves the existing destination and removes stages. It unmounts and
removes each image, recording docs/filesystem-validation-current.json. It is a
temporary, unvalidated runner; recreate/review if absent. Run separately from
the benchmark. No existing disk was formatted and no filesystem-matrix result
is claimed. Update docs/correctness.md only after actual successful runs.

## Finish in the next session

1. Completed: analyze the user's fresh benchmark and update performance docs
   with medians, ranges, selected/final workers and classifier correctness.
2. If admin access is available, finish the disposable filesystem matrix and
   record actual coverage. GPFS/NFS stays deferred until user supplies access.
3. Review the complete diff; build release to include the SSH fix. Commit only
   if requested. Preserve historical and partial benchmark artifacts.

## Continuation validation

The final diff review found no required code changes. The release build,
formatting, clippy with warnings denied, all 85 Rust tests (46 unit, 35 copy,
4 phase4), all six benchmark harness tests, all eight real SSH tests and the
real tmpfs ENOSPC test passed again. No commit was requested or created.

The disposable filesystem runner was reviewed, but an elevated invocation
failed with `sudo: a password is required`. The user was asked to run the
private-mount command above interactively. ext4, XFS, F2FS and Btrfs ENOSPC
remain unvalidated until that run produces successful records.

The first interactive matrix run failed on ext4 because the original 512 MiB
image could not hold the chunk test's eight 64 MiB files plus filesystem
overhead. Its raw output is retained in filesystem-validation-512m-failed.json.
The runner now uses 2 GiB images and records exceptions as failed runs.

## Completion

The user reran the corrected runner successfully. The saved
filesystem-validation-current.json records all 39 local integration tests
passing on each of ext4, XFS, Btrfs and F2FS, plus real ENOSPC preservation and
stage cleanup on each. The records were independently checked for successful
test exits, expected ENOSPC exits and passed status. docs/correctness.md now
records this coverage. Benchmark analysis, SSH hardening, available local
filesystem validation, final review and release build are complete. GPFS/NFS
remains explicitly deferred. Changes remain uncommitted as requested.
