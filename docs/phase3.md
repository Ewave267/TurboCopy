# Phase 3 specification

Current policy update: PSI is diagnostic and no longer reduces admission. CLI
storage selection is enabled by default; detected native GPFS selects its fixed
profile. Only explicit `--tune` calibrates non-GPFS workload concurrency. The
pressure-feedback design below records the original Phase 3 behavior. The
[Task 3 policy](automatic-workers.md) adds a bounded small-file probe for shared
NVMe, with no new active calibration.

Implement hardware adaptation from `project.md` on the Phase 2 engine, with no
new Rust dependencies and no changes to atomic replacement or destination mapping.

| Capability | Acceptance criterion |
| --- | --- |
| CLI | `--auto` selects workers/chunks; `--tune` implies auto and enables bounded calibration; `--no-tune` disables runtime changes. Contradictory tuning flags fail before mutation. Manual workers/chunks take precedence. Ordinary defaults remain unchanged. |
| Hardware profile | Read logical/physical CPUs, available memory, visible cgroup memory/CPU limits and Linux CPU/I/O pressure. Reuse storage topology; missing information yields conservative choices. Report destination free space where available. |
| Automatic selection | Use conservative HDD/network/shared/unknown profiles, cap automatic workers by CPUs and memory, enable automatic chunks only on known fast local storage, and select chunk size from each large file's length. Nested mounts are checked per file. |
| Workload profile | Count small (<1 MiB), medium, and large (>=64 MiB) regular files as they are discovered; retain streaming traversal and bounded memory. |
| Active tuning | Observe actual attempted bytes and committed files in one-second windows. Try increasing concurrency limits starting at one slot for at most eight windows (about eight seconds, subject to scheduling), require saturation and meaningful work, stop on <5% improvement, retain the best observed setting. Short/idle/sync workloads do not invent measurements. Score = (attempted bytes + 4 KiB per committed file) / window seconds, keeping units consistent across workload changes. No extra writes or source modifications. |
| Feedback | Reduce admission under CPU/I/O pressure and recover conservatively after it subsides. Change admission between file/helper tasks; existing tasks finish safely. Never exceed configured slot/memory bounds or resize explicit worker overrides. |
| Reporting | JSON and readable summaries include selected configuration, final admission limit, calibration state, adjustment count and hardware/workload profiles. Keep stdout machine-readable on failures/cancellation. |
| Verification | Test parsing, hardware input handling, selection/override policies, tuning/pressure decisions, shrinking/growing a live permit budget, byte-exact adaptive copying and dry-run/sync behavior. Retain Phase 1/2 regressions. Compare default, manual chunks, auto and tune using the existing durability/content checks. |

Implementation slices: (1) CLI and hardware selection, (2) adjustable permits and
workload feedback, (3) reporting/regressions, (4) release benchmarks and docs.

Calibration measures the real workload, including its changing mix and durability
stalls; it is a heuristic, not a guarantee of optimal throughput. In-flight chunk
helpers finish with their original permits, so shrinking is gradual and a single
file cannot be repartitioned mid-copy. Persistent tuning caches are optional in
the project brief and deferred: workload comparability is not established yet.
Free-space reporting is advisory; ENOSPC remains an ordinary rollback-safe error.
Visible cgroup v2 ancestor limits are supported; v1 limits and hidden ancestors
cannot be inspected here. Automatic selection remains bounded, but a one-slot
floor is not a guarantee against OOM in an already exhausted memory cgroup.
No reflinks, checksum, resume, or other Phase 4 features are included.

## Completed verification

`cargo test` passes all 42 tests (18 unit, 24 integration), including tuned-mode
rollback/cancellation/source-change/parent-swap cases and the retained Phase 1/2
suite. `cargo fmt --check`, strict all-target Clippy and the release build pass
on Rust 1.95.0/x86-64. Inverting the new storage eligibility guard failed the
selection regression, then the original guard was restored. A Python statvfs
cross-check verified advisory destination free-space reporting and dry-run
workload classification.

The mixed and single-large-file benchmark comparisons each verified 60 samples
against content, mode, nanosecond mtime, durability and JSON/concurrency checks.
[Performance results](performance.md) retain individual samples, medians and
observed calibration states. Automatic/tuned copying was slower for the mixed
workload here; no general performance advantage is claimed.

A longer 3,000-file smoke copy completed calibration with two measurements and
one adjustment. Its unchanged-sync pass skipped every file without measurements.
Both passed content/metadata checks; [calibration telemetry](calibration-phase3.json)
retains the observations. The local Graphify code index is refreshed with no API
cost; documentation semantic extraction remains outside that code-only index.
