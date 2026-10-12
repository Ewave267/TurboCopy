use std::{ffi::OsString, path::PathBuf};

pub const HELP: &str = "TurboCopy 0.1.0
Usage: tcopy [OPTIONS] SOURCE... DESTINATION

Recursively copy files and directories; overwrite files atomically.
An existing destination directory receives each source basename.

  --sync       Skip regular files with matching path and size.
               Matching sizes do not guarantee matching contents.
  --checksum   With --sync, compare all bytes before skipping.
  --sparse     Preserve sparse holes where Linux reports them.
  --reflink    Try copy-on-write clone; fall back if unsupported.
  --resume     Keep and validate sequential partials for retry (no sparse/reflink).
  --preserve-extended  Preserve owner, group, xattrs and ACLs where supported.
  --dry-run    Preview without changing destination data or metadata.
  --workers N  Concurrent I/O slots (1..128; default up to 4).
  --chunk-size SIZE  Opt into chunks for local large files (1K..64M, binary units).
  --auto       Select workers/chunks from hardware; react to pressure.
  --tune       Calibrate concurrency on the real copy workload (implies --auto).
  --no-tune    Keep the selected configuration fixed.
  --gpfs       GPFS profile: up to 32 fixed slots, 16 MiB chunks, 1 MiB buffers.
               Overrides via --workers/--chunk-size; incompatible with --auto/--tune.
  --no-fsync   Skip durability barriers; retain atomic replacement.
               Completed copies are not guaranteed durable after a crash.
  --progress   Print live progress to stderr.
  --json       Print one final JSON summary to stdout.
  --help, -h   Show this help.
  --version    Show the version.
  --           Treat remaining arguments as paths.

Destination directory symlinks are refused; source links are recreated.";

#[derive(Clone, Copy, Default)]
pub struct Options {
    pub gpfs: bool,
    pub no_fsync: bool,
    pub sync: bool,
    pub checksum: bool,
    pub sparse: bool,
    pub reflink: bool,
    pub resume: bool,
    pub preserve_extended: bool,
    pub dry_run: bool,
    pub workers: Option<usize>,
    pub chunk_size: Option<u64>,
    pub progress: bool,
    pub json: bool,
    pub auto: bool,
    pub tune: bool,
    pub no_tune: bool,
    pub manual_workers: bool,
    pub manual_chunks: bool,
}

pub struct Command {
    pub sources: Vec<PathBuf>,
    pub destination: PathBuf,
    pub options: Options,
}

pub enum Action {
    Copy(Command),
    Help,
    Version,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Action, String> {
    let mut options = Options::default();
    let mut paths = Vec::new();
    let mut operands_only = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if !operands_only {
            match arg.to_str() {
                Some("--") => {
                    operands_only = true;
                    continue;
                }
                Some("--help" | "-h") => return Ok(Action::Help),
                Some("--version") => return Ok(Action::Version),
                Some("--sync") => {
                    options.sync = true;
                    continue;
                }
                Some("--gpfs") => {
                    options.gpfs = true;
                    continue;
                }
                Some("--no-fsync") => {
                    options.no_fsync = true;
                    continue;
                }
                Some("--checksum") => {
                    options.checksum = true;
                    continue;
                }
                Some("--sparse") => {
                    options.sparse = true;
                    continue;
                }
                Some("--reflink") => {
                    options.reflink = true;
                    continue;
                }
                Some("--resume") => {
                    options.resume = true;
                    continue;
                }
                Some("--preserve-extended") => {
                    options.preserve_extended = true;
                    continue;
                }
                Some("--dry-run") => {
                    options.dry_run = true;
                    continue;
                }
                Some("--progress") => {
                    options.progress = true;
                    continue;
                }
                Some("--json") => {
                    options.json = true;
                    continue;
                }
                Some("--auto") => {
                    options.auto = true;
                    continue;
                }
                Some("--tune") => {
                    options.auto = true;
                    options.tune = true;
                    continue;
                }
                Some("--no-tune") => {
                    options.no_tune = true;
                    continue;
                }
                Some(text)
                    if text == "--workers"
                        || text.starts_with("--workers=")
                        || text == "--chunk-size"
                        || text.starts_with("--chunk-size=") =>
                {
                    let (flag, value) = match text.split_once('=') {
                        Some((flag, value)) => (flag, value.to_owned()),
                        None => (
                            text,
                            args.next()
                                .and_then(|s| s.into_string().ok())
                                .ok_or_else(|| format!("{text} requires a numeric value"))?,
                        ),
                    };
                    if flag == "--workers" {
                        let count = value
                            .parse::<usize>()
                            .ok()
                            .filter(|n| (1..=128).contains(n))
                            .ok_or("--workers must be an integer from 1 to 128")?;
                        options.workers = Some(count);
                        options.manual_workers = true;
                    } else {
                        options.chunk_size = Some(parse_size(&value)?);
                        options.manual_chunks = true;
                    }
                    continue;
                }
                _ => {}
            }
            if arg.as_encoded_bytes().starts_with(b"-") && arg != "-" {
                return Err(format!("unknown option: {arg:?}"));
            }
        }
        if arg.is_empty() {
            return Err("paths must not be empty".into());
        }
        paths.push(PathBuf::from(arg));
    }
    if options.tune && options.no_tune {
        return Err("--tune and --no-tune cannot be combined".into());
    }
    if options.gpfs && options.auto {
        return Err("--gpfs cannot be combined with --auto or --tune".into());
    }
    if options.resume && options.no_fsync {
        return Err("--resume cannot be combined with --no-fsync".into());
    }
    if options.checksum && !options.sync {
        return Err("--checksum requires --sync".into());
    }
    if options.resume && (options.sparse || options.reflink) {
        return Err("--resume cannot be combined with --sparse or --reflink".into());
    }
    if paths.len() < 2 {
        return Err("expected SOURCE... DESTINATION (use --help for usage)".into());
    }
    let destination = paths.pop().unwrap();
    Ok(Action::Copy(Command {
        sources: paths,
        destination,
        options,
    }))
}

impl Options {
    pub fn buffer_size(self) -> usize {
        if self.gpfs {
            1 << 20
        } else {
            128 << 10
        }
    }
    pub fn worker_count(self) -> usize {
        self.workers
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(4)))
    }
    pub fn preview(self, message: impl std::fmt::Display) {
        if self.json {
            eprintln!("{message}");
        } else {
            println!("{message}");
        }
    }
}

fn parse_size(value: &str) -> Result<u64, String> {
    let digits = value.bytes().take_while(u8::is_ascii_digit).count();
    let multiplier = match &value[digits..] {
        "" => 1,
        "K" | "KiB" => 1024,
        "M" | "MiB" => 1024 * 1024,
        "G" | "GiB" => 1024 * 1024 * 1024,
        _ => return Err("chunk size uses bytes or binary K/M/G, KiB/MiB/GiB suffixes".into()),
    };
    value[..digits]
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .filter(|n| (1024..=64 * 1024 * 1024).contains(n))
        .ok_or_else(|| "--chunk-size must be from 1 KiB to 64 MiB".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Result<Options, String> {
        match parse(args.iter().map(OsString::from))? {
            Action::Copy(command) => Ok(command.options),
            _ => panic!("expected copy"),
        }
    }

    #[test]
    fn validates_worker_and_chunk_controls() {
        let parsed = options(&[
            "--workers=3",
            "--chunk-size",
            "8MiB",
            "--json",
            "--progress",
            "a",
            "b",
        ])
        .unwrap();
        assert_eq!(parsed.workers, Some(3));
        assert_eq!(parsed.chunk_size, Some(8 * 1024 * 1024));
        assert!(parsed.json && parsed.progress);
        for args in [
            vec!["--gpfs", "--auto", "a", "b"],
            vec!["--tune", "--gpfs", "a", "b"],
            vec!["--no-fsync", "--resume", "a", "b"],
            vec!["--workers", "0", "a", "b"],
            vec!["--workers=129", "a", "b"],
            vec!["--workers=-1", "a", "b"],
            vec!["--chunk-size=0", "a", "b"],
            vec!["--chunk-size=65M", "a", "b"],
            vec!["--chunk-size=18446744073709551615G", "a", "b"],
            vec!["--chunk-size=1MB", "a", "b"],
            vec!["--workers"],
        ] {
            assert!(options(&args).is_err(), "{args:?}");
        }
        assert_eq!(
            options(&["--chunk-size=1K", "a", "b"]).unwrap().chunk_size,
            Some(1024)
        );
    }
}
