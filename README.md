# TurboCopy

TurboCopy is a Linux command-line tool for copying files and directories with
parallel I/O, automatic storage-based configuration, and atomic file replacement.

## Installation

Download the archive for Linux x86-64 or AArch64 from
[GitHub Releases](https://github.com/Ewave267/TurboCopy/releases), extract it,
and run `./tcopy --help`. Releases are development prereleases; `SHA256SUMS`
is provided for verification.

To build from source, install Rust 1.75 or newer:

```bash
cargo build --release
./target/release/tcopy --help
```

Linux and mounted `/proc/self/fd` are required. There are no third-party Rust
dependencies.

## Usage

```bash
# Copy a file or directory
./tcopy source.bin destination.bin
./tcopy source_dir destination_dir

# Copy only a directory's contents
./tcopy source_dir/. destination_dir/

# Preview changes
./tcopy source_dir destination_dir --dry-run

# Skip files with identical contents
./tcopy source_dir destination_dir --sync --checksum

# Show live progress
./tcopy source_dir destination_dir --progress
```

For a source build, use `./target/release/tcopy` in place of `./tcopy`.
Run `./tcopy --help` for all options, including worker controls, JSON output,
sparse files, reflinks, resumable copies, and extended metadata preservation.
Native GPFS mounts select the GPFS profile automatically; see the
[GPFS guide](docs/gpfs.md) for configuration and measurement.

## Copy behavior

- Files are overwritten by default. Existing destination directories receive
  the source basename; directories are merged and extra files are retained.
- Multiple sources require an existing destination directory. Destination
  parents must already exist.
- Permissions and modification times are preserved. Source symlinks are
  recreated; destination directory symlinks are refused.
- `--sync` alone skips files with matching sizes, which may have different
  contents. Add `--checksum` to compare contents before skipping.
- File replacement is atomic; a directory copy is not transactional. Keep
  source data stable while copying. `--no-fsync` disables durability barriers
  and removes the guarantee of crash durability.

## Development

```bash
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
python3 -m unittest discover -s tests -p 'test_publish_preview.py'
```

Chunking tests require a supported non-HDD filesystem, such as tmpfs.
See the [architecture](docs/architecture.md) and
[performance documentation](docs/performance.md) for implementation details.

## License

[MIT](LICENSE).
