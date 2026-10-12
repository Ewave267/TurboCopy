# Project: TurboCopy — Hardware-Adaptive Parallel File Copier

You are an experienced Rust systems engineer specializing in Linux filesystems, high-performance I/O, multithreading, and storage optimization.

Build **TurboCopy**, an open-source command-line tool combining the simplicity of `cp`, the useful synchronization behavior of `rsync`, and a hardware-adaptive parallel copy engine.

The main objective is to achieve high throughput while remaining reliable, predictable, and easy to use.

## 1. Core behavior

The CLI should work as follows:

```bash
# Copy a file
tcopy source.bin destination.bin

# Copy a directory recursively
tcopy source_dir/ destination_dir/

# Parallel copy with automatic hardware optimization
tcopy source_dir/ destination_dir/ --auto

# Skip existing destination files with matching name and size
tcopy source_dir/ destination_dir/ --sync

# Manually override parallelism and chunk size
tcopy source_dir/ destination_dir/ --workers 16 --chunk-size 8M

# Preview without changing anything
tcopy source_dir/ destination_dir/ --dry-run
```

**Default behavior:**

- Copy files and directories recursively.
- Overwrite existing destination files.
- Do not compare source and destination contents before copying.
- Do not delete unrelated destination files.
- Support copying multiple source files into one destination directory.
- Preserve the intuitive source/destination behavior of GNU `cp -r`, including consistent treatment of trailing slashes.
- Support large files, deeply nested directories, empty directories, and filenames containing spaces or unusual characters.
- Preserve file permissions and modification timestamps where supported.
- Avoid following symbolic links by default; recreate the links instead.
- Never recursively copy a destination into itself.

**`--sync` behavior:**

- Compare files using relative path and file size.
- If the destination file exists at the corresponding relative path and its size matches, skip it.
- Otherwise copy it.
- Do not calculate hashes or compare timestamps by default.
- Provide an optional `--checksum` mode for content verification.
- Explain that matching file sizes do not guarantee matching contents.
- Never delete extra destination files unless a separately designed explicit deletion feature is added.

## 2. Parallel copy engine

Implement two levels of parallelism:

**File-level parallelism**

- Copy multiple independent files concurrently.
- Use a bounded worker pool.
- Use a bounded task queue to avoid excessive memory consumption.
- Dynamically schedule work so that workers do not remain idle.
- Handle directories containing millions of files without loading the entire directory tree into memory.

**Chunk-level parallelism**

- For sufficiently large regular files, support copying independent file ranges concurrently.
- Use safe offset-based I/O such as `pread`/`pwrite` equivalents where appropriate.
- Preallocate or size the destination appropriately before parallel writes.
- Prevent overlapping writes and race conditions.
- Avoid chunk-level parallelism when it is expected to reduce performance.
- Use a streaming copy path for small files and unsupported filesystems.

## 3. Hardware-aware optimization

Implement an automatic tuning system that examines:

- Logical and physical CPU cores.
- Available RAM and memory limits.
- Source and destination filesystem types.
- Whether source and destination share the same underlying storage.
- SSD, NVMe, HDD, or unknown device classification where detectable.
- CPU and I/O pressure.
- Available storage space.
- Network filesystem detection, including NFS, where possible.
- Linux storage topology through `/sys`, mount information, and appropriate system APIs.

Determine:

1. Number of concurrent file-copy workers.
2. Number of concurrent chunks per large file.
3. Chunk size.
4. Total in-flight I/O and memory budget.
5. When to prioritize sequential I/O versus parallel I/O.

**Do not assume that more workers always means better performance.**

Use conservative defaults when hardware characteristics cannot be determined.

### Adaptive benchmarking

Implement an optional `--tune` mode that performs a short, bounded workload calibration.

The tuner should:

- Prefer representative reads and actual copy workload observations.
- Avoid destructive writes or modifications to existing data.
- Measure throughput across a small set of candidate configurations.
- Avoid flooding shared or production storage systems.
- Stop testing when additional parallelism provides no meaningful improvement.
- Optionally adjust concurrency during a long-running transfer.
- Cache tuning results where the storage identity and workload remain comparable.
- Provide `--no-tune` and manual overrides.

Do not promise guaranteed optimal performance. Aim for a practical configuration that approaches the highest sustainable throughput.

## 4. Fast copying strategies

Investigate and benchmark Linux I/O strategies:

- `copy_file_range()`
- `sendfile()`, where applicable
- Buffered `read()` / `write()`
- `pread()` / `pwrite()`
- `io_uring` as an optional later optimization
- Reflink/clone support on compatible filesystems

Implement a reliable fallback strategy.

A reflink should only be used when explicitly enabled because instantaneous copy-on-write cloning has different storage semantics from a full physical copy.

Do not assume zero-copy syscalls are always faster.

Optimize for:

- Millions of small files.
- Medium-sized files.
- Multi-gigabyte files.
- NVMe-to-NVMe copies.
- HDD-to-SSD copies.
- Copies between directories on the same filesystem.
- NFS and other shared filesystems.

## 5. Reliability and filesystem correctness

Reliability is mandatory, even when it reduces peak performance.

- Handle partial reads and writes.
- Handle disk-full errors and permission failures.
- Handle interrupted system calls.
- Handle cancellation with SIGINT and SIGTERM.
- Do not leave a truncated destination file presented as a successful copy.
- Use temporary files and atomic renames where supported.
- Clean up or clearly identify incomplete temporary files.
- Never follow malicious destination symlinks into unintended locations.
- Do not silently corrupt sparse files.
- Define behavior for hard links, symlinks, special files, and metadata.
- Never silently skip failed files.
- Provide a clear summary and nonzero exit status when failures occur.
- Document crash-consistency guarantees and limitations.

If a source file changes during copying, detect the condition when practical and report that a consistent snapshot was not guaranteed.

## 6. Performance reporting

Provide optional progress output:

```text
TurboCopy 0.1.0

Source:      /data/source
Destination: /backup/source

Storage:     NVMe -> NVMe
Workers:     12
Chunk size:  4 MiB
Mode:        Parallel / Adaptive

Files:       12,450 / 20,000
Copied:      85.3 GiB
Skipped:     4,100
Throughput:  2.1 GiB/s
Elapsed:     00:41
ETA:         00:28
```

Report useful metrics:

- Total bytes copied.
- Effective transfer throughput.
- File count and skipped files.
- Failures.
- Worker utilization.
- Time spent on metadata operations versus transferring data.

Add `--json` for machine-readable output.

## 7. Testing and benchmarking

Create comprehensive automated tests for:

- Basic copy operations.
- Recursive directory copying.
- `--sync` skip behavior.
- Same-size files with different contents.
- Empty files and directories.
- Symbolic links.
- File permission errors.
- Disk-full conditions.
- Sparse files.
- Concurrent writes to large files.
- Interrupted copies.
- Destination collision handling.
- Very large directory trees.
- Unicode and unusual filenames.
- Files changing during transfer.

Create a benchmark suite comparing TurboCopy against:

```bash
cp -r source/ destination/
rsync -a source/ destination/
tcopy source/ destination/
tcopy source/ destination/ --auto
```

Ensure benchmarks use equivalent metadata and durability semantics, identical dataset layouts, warm/cold cache distinctions where possible, and separate first-copy and incremental-copy scenarios.

Never claim TurboCopy is faster without reproducible measurements.

## 8. Architecture

Use Rust with a modular design.

Suggested modules:

```text
src/
├── main.rs
├── cli.rs
├── scanner.rs
├── copy_engine.rs
├── scheduler.rs
├── storage_detect.rs
├── tuner.rs
├── metadata.rs
├── progress.rs
└── error.rs

tests/
benches/
docs/
  architecture.md
  decisions/
  performance.md
```

Avoid unnecessary dependencies and abstractions.

Design the copy engine so that storage detection, scheduling, and copy mechanisms can be independently tested.

## 9. Development methodology

Use the available Ponytail, Agent Skills, and Graphify capabilities when they are installed.

- **Agent Skills:** Specification-first planning, architecture, test planning, implementation and review.
- **Ponytail:** Keep implementation straightforward, avoid needless abstractions, and review complexity.
- **Graphify:** Track architecture, dependencies, and the effects of changes as the project evolves.
- **OmniRoute:** Optional model routing if configured; do not require it.

Before implementation, create:

- A clear requirements specification.
- Acceptance criteria for each feature.
- A short architecture document.
- Architectural decision records explaining important choices.
- A benchmarking methodology.
- A list of assumptions and unresolved risks.

Implement the project in phases:

**Phase 1 — Correctness-first MVP**

- CLI and recursive copying.
- Default overwrite behavior.
- `--sync` behavior.
- Atomic destination replacement.
- Basic metadata preservation.
- File-level parallelism.
- Automated tests.

**Phase 2 — Performance**

- Storage detection.
- Adjustable worker count and chunk size.
- Chunk-level parallel copying.
- Copy syscall benchmarking.
- Progress and throughput metrics.

**Phase 3 — Hardware adaptation**

- Automatic worker and chunk selection.
- Optional active tuning.
- Feedback-driven concurrency adjustment.
- Hardware and workload profiles.

**Phase 4 — Advanced functionality**

- Sparse-file optimization.
- Optional reflinks.
- Advanced Linux I/O experiments.
- Resume support.
- Extended metadata support.
- Extensive filesystem benchmarking.

At the end of each phase, run tests, report benchmark results, and explain architectural tradeoffs.

**Start with Phase 1. Do not attempt to implement every optimization immediately.**

