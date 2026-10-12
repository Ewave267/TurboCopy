use crate::{
    cli::Options,
    fsutil::{self, Directory},
    progress::{Summary, Timer},
    scheduler::Concurrency,
    storage_detect::Detector,
};
use std::{
    ffi::OsString,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::{symlink, FileExt, MetadataExt, OpenOptionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
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

pub fn copy(
    task: &Task,
    options: Options,
    buffer: &mut [u8],
    budget: &Concurrency,
    detector: &Detector,
) -> io::Result<Outcome> {
    let summary = &budget.summary;
    let metadata_timer = Timer::new(&summary.metadata_ns);
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
    let chunk_size =
        if (options.chunk_size.is_some() || options.auto) && before.len() >= 64 * 1024 * 1024 {
            let source_path = task.source_parent.real_path()?.join(&task.source_name);
            let destination_path = task.parent.real_path()?;
            crate::tuner::chunk_size(
                options,
                &detector.detect(&source_path, before.dev()),
                &detector.detect(&destination_path, task.parent.metadata()?.dev()),
                before.len(),
                options.worker_count(),
            )
        } else {
            None
        };
    drop(metadata_timer);
    {
        let _timer = Timer::new(&summary.transfer_ns);
        let chunked = match chunk_size.filter(|_| options.worker_count() > 1) {
            Some(size) if before.len() > size => {
                match transfer_chunks(&input, &output, before.len(), size, buffer, budget) {
                    Err(error) if unsupported_positioned(&error) => {
                        summary.fallbacks.fetch_add(1, Ordering::Relaxed);
                        output.set_len(0)?;
                        input.seek(SeekFrom::Start(0))?;
                        output.seek(SeekFrom::Start(0))?;
                        false
                    }
                    result => result?,
                }
            }
            _ => false,
        };
        if !chunked {
            transfer(&mut input, &mut output, before.len(), buffer, |n| {
                summary.transferred.fetch_add(n, Ordering::Relaxed);
            })?;
        }
    }
    let _metadata_timer = Timer::new(&summary.metadata_ns);
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
    mut report: impl FnMut(u64),
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
        let mut written = 0;
        while written < count {
            fsutil::check_cancelled()?;
            let n = match output.write(&buffer[written..count]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            written += n;
            report(n as u64);
        }
        remaining -= count as u64;
    }
    Ok(())
}

fn unsupported_positioned(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(22 | 29 | 38 | 95))
}

fn transfer_chunks(
    input: &File,
    output: &File,
    length: u64,
    chunk_size: u64,
    buffer: &mut [u8],
    budget: &Concurrency,
) -> io::Result<bool> {
    let chunks = length / chunk_size + u64::from(length % chunk_size != 0);
    let mut permits = Vec::new();
    for _ in 1..chunks.min(budget.summary.options.worker_count() as u64) {
        match budget.try_acquire() {
            Some(permit) => permits.push(permit),
            None => break,
        }
    }
    if permits.is_empty() {
        return Ok(false);
    }
    output.set_len(length)?;
    budget.summary.chunked.fetch_add(1, Ordering::Relaxed);
    let chunks = Chunks {
        length,
        size: chunk_size,
        next: AtomicU64::new(0),
        stopped: AtomicBool::new(false),
    };
    let failure = Mutex::new(None);
    let run = |buffer: &mut [u8]| {
        let result = chunks.transfer(input, output, buffer, &budget.summary);
        if let Err(error) = result {
            let mut first = failure.lock().unwrap();
            // A real I/O failure must not be hidden by an unsupported-I/O fallback.
            if first.as_ref().map_or(true, |saved| {
                unsupported_positioned(saved) && !unsupported_positioned(&error)
            }) {
                *first = Some(error);
            }
            chunks.stopped.store(true, Ordering::Relaxed);
        }
    };
    std::thread::scope(|scope| {
        let mut helpers = Vec::new();
        for permit in permits {
            let run = &run;
            match std::thread::Builder::new()
                .name("tcopy-chunk".into())
                .spawn_scoped(scope, move || {
                    let _permit = permit;
                    run(&mut vec![0; 128 * 1024]);
                }) {
                Ok(helper) => helpers.push(helper),
                Err(error) => {
                    *failure.lock().unwrap() = Some(error);
                    chunks.stopped.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
        run(buffer);
        for helper in helpers {
            if helper.join().is_err() {
                *failure.lock().unwrap() = Some(io::Error::other("chunk worker panicked"));
                chunks.stopped.store(true, Ordering::Relaxed);
            }
        }
    });
    match failure.into_inner().unwrap() {
        Some(error) => Err(error),
        None => Ok(true),
    }
}

// Each claimed chunk owns its offsets; positioned I/O never changes shared file cursors.
struct Chunks {
    length: u64,
    size: u64,
    next: AtomicU64,
    stopped: AtomicBool,
}
impl Chunks {
    fn transfer(
        &self,
        input: &File,
        output: &File,
        buffer: &mut [u8],
        summary: &Summary,
    ) -> io::Result<()> {
        let length = self.length;
        let chunk_size = self.size;
        let chunks = length / chunk_size + u64::from(length % chunk_size != 0);
        loop {
            if self.stopped.load(Ordering::Relaxed) {
                return Ok(());
            }
            fsutil::check_cancelled()?;
            let index = self.next.fetch_add(1, Ordering::Relaxed);
            if index >= chunks {
                return Ok(());
            }
            let mut offset = index * chunk_size;
            let end = offset + chunk_size.min(length - offset);
            while offset < end {
                if self.stopped.load(Ordering::Relaxed) {
                    return Ok(());
                }
                fsutil::check_cancelled()?;
                let limit = (end - offset).min(buffer.len() as u64) as usize;
                let count = match input.read_at(&mut buffer[..limit], offset) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    result => result?,
                };
                if count == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "source shrank during chunk copying",
                    ));
                }
                let mut written = 0;
                while written < count {
                    if self.stopped.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    fsutil::check_cancelled()?;
                    let n = match output.write_at(&buffer[written..count], offset + written as u64)
                    {
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        result => result?,
                    };
                    if n == 0 {
                        return Err(io::ErrorKind::WriteZero.into());
                    }
                    summary.transferred.fetch_add(n as u64, Ordering::Relaxed);
                    written += n;
                }
                offset += count as u64;
            }
        }
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::*;
    use std::{sync::Arc, time::Instant};
    #[test]
    fn positioned_ranges_preserve_cursors_and_reject_short_sources() {
        let root = std::env::temp_dir().join(format!("tcopy-ranges-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let bytes: Vec<_> = (0..33).collect();
        fs::write(root.join("in"), &bytes).unwrap();
        let mut input = File::open(root.join("in")).unwrap();
        input.seek(SeekFrom::Start(7)).unwrap();
        let output = File::create(root.join("out")).unwrap();
        let summary = Arc::new(Summary::new(
            Options {
                workers: Some(3),
                ..Options::default()
            },
            vec![],
            Instant::now(),
        ));
        let budget = Concurrency::new(summary.clone());
        let _slot = budget.try_acquire().unwrap();
        assert!(transfer_chunks(&input, &output, 33, 8, &mut [0; 3], &budget).unwrap());
        assert_eq!(fs::read(root.join("out")).unwrap(), bytes);
        assert_eq!(input.stream_position().unwrap(), 7);
        assert_eq!(summary.transferred.load(Ordering::Relaxed), 33);
        assert_eq!(
            transfer_chunks(&input, &output, 34, 8, &mut [0; 3], &budget)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        let full = OpenOptions::new().write(true).open("/dev/full").unwrap();
        let ranges = Chunks {
            length: 33,
            size: 8,
            next: 0.into(),
            stopped: false.into(),
        };
        assert_eq!(
            ranges
                .transfer(&input, &full, &mut [0; 3], &summary)
                .unwrap_err()
                .raw_os_error(),
            Some(28)
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fallback_is_only_for_unsupported_positioned_io() {
        for code in [22, 29, 38, 95] {
            assert!(unsupported_positioned(&io::Error::from_raw_os_error(code)));
        }
        for code in [5, 13, 28, 4] {
            assert!(!unsupported_positioned(&io::Error::from_raw_os_error(code)));
        }
    }
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
        transfer(&mut reader, &mut output, 6, &mut [0; 4], |_| {}).unwrap();
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
        transfer(&mut &b"abcdef"[..], &mut writer, 6, &mut [0; 3], |_| {}).unwrap();
        assert_eq!(writer.bytes, b"abcdef");
        writer.bytes.clear();
        writer.full_after = Some(2);
        assert_eq!(
            transfer(&mut &b"abcdef"[..], &mut writer, 6, &mut [0; 3], |_| {})
                .unwrap_err()
                .raw_os_error(),
            Some(28)
        );
    }
    #[test]
    fn short_source_is_failure_and_growth_is_bounded() {
        assert_eq!(
            transfer(&mut &b"a"[..], &mut Vec::new(), 2, &mut [0; 4], |_| {})
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut output = Vec::new();
        transfer(&mut &b"longer"[..], &mut output, 2, &mut [0; 4], |_| {}).unwrap();
        assert_eq!(output, b"lo");
    }
}
