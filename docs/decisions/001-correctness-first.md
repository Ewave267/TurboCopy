# ADR-001: Bounded buffered copy and descriptor-anchored replacement

Status: Accepted for Phase 1. Date: 2026-10-11.

## Context

The first phase must preserve data on errors, avoid destination symlink attacks,
copy large trees with bounded memory, and run without additional Rust packages.

## Decision

Use the Rust standard library, a small Linux FFI boundary for signals and
symlink timestamps, buffered file-level workers, a 64-entry queue, and
descriptor-anchored temporary-file rename. Use conservative automatic worker
counts up to four. Preserve directory metadata after draining descendant work.
Keep CPU/storage tuning and alternate copy mechanisms for later phases.

## Alternatives

`std::fs::copy` is concise but does not expose the cancellation checkpoints and
source-change validation needed here. Path checks followed by ordinary writes
leave a symlink race. Loading all file tasks enables simple parallel iteration
but scales memory with file count. A third-party capability filesystem crate
would reduce platform-specific code but adds dependencies; revisit when adding
non-Linux support. Per-directory completion graphs could remove barriers but
add scheduling state unnecessary for the MVP.

## Consequences

Existing destination inode/hard-link relationships change on atomic replacement.
Sparse holes can expand without changing logical contents. Fsync costs throughput.
This version requires Linux x86-64/AArch64 with procfs. Security and publication
logic can be tested independently of the scheduler; later engines must retain
these guarantees.
