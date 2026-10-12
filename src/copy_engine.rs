use crate::{
    cli::Options,
    fsutil::{self, Directory},
};
use std::{
    ffi::OsString,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{symlink, OpenOptionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub struct Task {
    pub source_parent: Directory,
    pub source_name: OsString,
    pub source_path: PathBuf,
    pub parent: Directory,
    pub name: OsString,
    pub destination: PathBuf,
    pub metadata: Metadata,
}

pub enum Outcome {
    Copied(u64),
    Skipped,
}

struct Temporary {
    path: PathBuf,
    published: bool,
}
impl Temporary {
    fn publish(&mut self, destination: &std::path::Path) -> io::Result<()> {
        fs::rename(&self.path, destination)?;
        self.published = true;
        Ok(())
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        if let Err(error) = fs::remove_file(&self.path) {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!("tcopy: cannot clean up temporary {:?}: {error}", self.path);
            }
        }
    }
}

fn temporary(
    parent: &Directory,
    link: Option<&std::path::Path>,
) -> io::Result<(Temporary, Option<File>)> {
    for _ in 0..100 {
        let name = format!(
            ".tcopy-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        );
        let path = parent.path().join(name);
        let created = match link {
            Some(target) => symlink(target, &path).map(|_| None),
            None => OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map(Some),
        };
        match created {
            Ok(file) => {
                return Ok((
                    Temporary {
                        path,
                        published: false,
                    },
                    file,
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "temporary name collisions",
    ))
}

pub fn copy(task: &Task, options: Options, buffer: &mut [u8]) -> io::Result<Outcome> {
    fsutil::check_cancelled()?;
    let source = task.source_parent.path().join(&task.source_name);
    let destination = task.parent.path().join(&task.name);
    let existing = fsutil::optional_metadata(&destination)?;
    fsutil::validate_destination(&task.metadata, existing.as_ref())?;
    if task.metadata.file_type().is_symlink() {
        let target = fs::read_link(&source)?;
        let (mut temporary, _) = temporary(&task.parent, Some(&target))?;
        task.parent
            .link_mtime(temporary.path.file_name().unwrap(), &task.metadata)?;
        if !fsutil::unchanged(&task.metadata, &fs::symlink_metadata(&source)?) {
            return Err(fsutil::invalid("source link changed during copying"));
        }
        fsutil::check_cancelled()?;
        temporary.publish(&destination)?;
        task.parent.sync()?;
        return Ok(Outcome::Copied(0));
    }
    if !task.metadata.is_file() {
        return Err(fsutil::invalid(
            "unsupported special file (FIFO, socket or device)",
        ));
    }
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(fsutil::NOFOLLOW | fsutil::NONBLOCK)
        .open(&source)?;
    let before = input.metadata()?;
    if !before.is_file() || !fsutil::unchanged(&task.metadata, &before) {
        return Err(fsutil::invalid("source changed before copying"));
    }
    if options.sync
        && existing
            .as_ref()
            .is_some_and(|m| m.is_file() && m.len() == before.len())
    {
        return Ok(Outcome::Skipped);
    }
    let (mut temporary, output) = temporary(&task.parent, None)?;
    let mut output = output.unwrap();
    transfer(&mut input, &mut output, before.len(), buffer)?;
    if !fsutil::unchanged(&before, &input.metadata()?)
        || !fsutil::unchanged(&before, &fs::symlink_metadata(&source)?)
    {
        return Err(fsutil::invalid(
            "source changed during copying; no consistent snapshot",
        ));
    }
    fsutil::preserve_metadata(&output, &before)?;
    fsutil::check_cancelled()?;
    temporary.publish(&destination)?;
    task.parent.sync()?;
    Ok(Outcome::Copied(before.len()))
}

fn transfer(
    input: &mut impl Read,
    output: &mut impl Write,
    mut remaining: u64,
    buffer: &mut [u8],
) -> io::Result<()> {
    while remaining > 0 {
        fsutil::check_cancelled()?;
        let limit = remaining.min(buffer.len() as u64) as usize;
        let count = match input.read(&mut buffer[..limit]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "source shrank during copying",
            ));
        }
        output.write_all(&buffer[..count])?;
        remaining -= count as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_interrupted_reads_and_accepts_partial_reads() {
        struct ShortReader<'a> {
            bytes: &'a [u8],
            interrupted: bool,
        }
        impl Read for ShortReader<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(io::Error::from(io::ErrorKind::Interrupted));
                }
                let count = buffer.len().min(self.bytes.len()).min(2);
                buffer[..count].copy_from_slice(&self.bytes[..count]);
                self.bytes = &self.bytes[count..];
                Ok(count)
            }
        }
        let mut reader = ShortReader {
            bytes: b"abcdef",
            interrupted: false,
        };
        let mut output = Vec::new();
        transfer(&mut reader, &mut output, 6, &mut [0; 4]).unwrap();
        assert_eq!(output, b"abcdef");
    }
    struct ShortWriter {
        bytes: Vec<u8>,
        interrupted: bool,
        full_after: Option<usize>,
    }
    impl Write for ShortWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            if self
                .full_after
                .is_some_and(|limit| self.bytes.len() >= limit)
            {
                return Err(io::Error::from_raw_os_error(28));
            }
            let count = bytes.len().min(2);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn handles_short_writes_interrupts_and_disk_full() {
        let mut writer = ShortWriter {
            bytes: Vec::new(),
            interrupted: false,
            full_after: None,
        };
        transfer(&mut &b"abcdef"[..], &mut writer, 6, &mut [0; 3]).unwrap();
        assert_eq!(writer.bytes, b"abcdef");
        writer.bytes.clear();
        writer.full_after = Some(2);
        assert_eq!(
            transfer(&mut &b"abcdef"[..], &mut writer, 6, &mut [0; 3])
                .unwrap_err()
                .raw_os_error(),
            Some(28)
        );
    }
    #[test]
    fn short_source_is_failure_and_growth_is_bounded() {
        assert_eq!(
            transfer(&mut &b"a"[..], &mut Vec::new(), 2, &mut [0; 4])
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut output = Vec::new();
        transfer(&mut &b"longer"[..], &mut output, 2, &mut [0; 4]).unwrap();
        assert_eq!(output, b"lo");
    }
}
