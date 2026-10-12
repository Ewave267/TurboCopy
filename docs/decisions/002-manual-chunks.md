# ADR-002: Manual positioned chunks with a shared permit budget

Status: Accepted for Phase 2. Date: 2026-10-11.

## Context

Phase 2 needs configurable workers/chunks and measured copy mechanisms while
retaining atomic replacement, bounded memory and source-change checks. Physical
storage details can be hidden by mounts, mapper layers and limited permissions.

## Decision

Retain default buffered workers. Explicit chunk size opts large files into
positioned I/O on recognized local filesystems, excluding detected HDDs.
Reserve available shared permits without waiting, spawn scoped range helpers,
and have the parent participate. Join all helpers before publication or fallback.
Reserve neither a full-file task list nor chunk-sized buffers. Keep all metadata,
fsync and descriptor-anchored rename logic common to both paths.

Report storage observations and unknowns without automatic tuning. Benchmark
read/write, pread/pwrite, sendfile and copy_file_range separately. Do not silently
select copy_file_range: it may clone/offload instead of copying physical data.
Expose live transfer and final committed-byte counts separately.

## Alternatives and consequences

A permanent queue of range tasks could reduce thread startup costs but introduces
nested completion tracking and deadlock risks. Short-lived helpers keep ownership
and error handling local; idle file threads can coexist with helpers, with at most
`2 * workers - 1` data buffers/threads and `workers` active slots. Helpers are
reserved once per file; this is not continuous feedback-driven adaptation.

Pre-sizing ensures independent ranges can write their offsets; it creates sparse
length before data is complete, but the file remains private until validation,
metadata and fsync succeed. A shared unsupported-I/O error falls back only after
joining helpers. ENOSPC/EIO/permission/cancellation errors preserve the old file.
Directory completion barriers and per-file syncing retain their Phase 1 costs.
Automatic selection/tuning and syscall production integration remain later work.
