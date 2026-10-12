use std::{ffi::OsString, path::PathBuf};

pub const HELP: &str = "TurboCopy 0.1.0
Usage: tcopy [--sync] [--dry-run] SOURCE... DESTINATION

Recursively copy files and directories; overwrite files atomically.
An existing destination directory receives each source basename.

  --sync       Skip regular files with matching path and size.
               Matching sizes do not guarantee matching contents.
  --dry-run    Preview without changing destination data or metadata.
  --help, -h   Show this help.
  --version    Show the version.
  --           Treat remaining arguments as paths.

Destination directory symlinks are refused; source links are recreated.
Hardware tuning, checksum verification and chunk controls are later phases.";

#[derive(Clone, Copy, Default)]
pub struct Options {
    pub sync: bool,
    pub dry_run: bool,
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
    for arg in args {
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
                Some("--dry-run") => {
                    options.dry_run = true;
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
