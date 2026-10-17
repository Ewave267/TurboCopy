use super::{
    agent,
    transport::{Client, Endpoint},
    wire::Info,
};
use crate::{cli::Options, copy_engine::Outcome, fsutil, progress::Timer, scheduler::Concurrency};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
};

#[derive(Clone)]
pub struct Job {
    pub source: Arc<Endpoint>,
    pub destination: Arc<Endpoint>,
    pub info: Info,
    pub parent: Option<u64>,
    pub name: OsString,
    pub path: PathBuf,
}

#[derive(Default)]
pub struct Worker {
    source: Option<(Arc<Endpoint>, Client)>,
    destination: Option<(Arc<Endpoint>, Client)>,
}
impl Worker {
    pub fn run(
        &mut self,
        job: &Job,
        options: Options,
        budget: &Concurrency,
        buffer: &mut [u8],
    ) -> io::Result<Outcome> {
        for (cached, endpoint) in [
            (&mut self.source, &job.source),
            (&mut self.destination, &job.destination),
        ] {
            if cached
                .as_ref()
                .map_or(true, |(previous, _)| !Arc::ptr_eq(previous, endpoint))
            {
                *cached = Some((endpoint.clone(), endpoint.connect()?));
            }
        }
        let result = run(
            job,
            options,
            budget,
            buffer,
            &mut self.source.as_mut().unwrap().1,
            &mut self.destination.as_mut().unwrap().1,
        );
        if result.is_err() {
            self.source = None;
            self.destination = None;
        }
        result
    }
}

fn run(
    job: &Job,
    options: Options,
    budget: &Concurrency,
    buffer: &mut [u8],
    source: &mut Client,
    destination: &mut Client,
) -> io::Result<Outcome> {
    let metadata_timer = Timer::new(&budget.summary.metadata_ns);
    fsutil::check_cancelled()?;
    if job.info.kind != 1 && job.info.kind != 3 {
        return Err(fsutil::invalid(
            "unsupported special file (FIFO, socket or device)",
        ));
    }
    let existing = match job.parent {
        Some(parent) => destination.stat_at(parent, &job.name)?,
        None => Info::default(),
    };
    agent::validate(&job.info, &existing)?;
    if job.source.same_machine(&job.destination)
        && existing.kind != 0
        && existing.dev == job.info.dev
        && existing.ino == job.info.ino
    {
        return Err(fsutil::invalid("source and destination are the same file"));
    }
    let size_matches =
        options.sync && job.info.kind == 1 && existing.kind == 1 && job.info.size == existing.size;
    let skip = size_matches
        && (!options.checksum || checksum(job, &existing, source, destination, buffer)?);
    agent::verify(&job.info, &source.stat(&job.info.path, false)?.2)?;
    if options.dry_run {
        options.preview(format_args!(
            "{} {:?} -> {:?}",
            if skip { "skip" } else { "copy" },
            job.info.path,
            job.path
        ));
    }
    if skip
        && !options.dry_run
        && (job.info.mode & 0o7777 != existing.mode & 0o7777
            || job.info.mtime != existing.mtime
            || job.info.mtime_ns != existing.mtime_ns)
    {
        let transferred = destination.sync_metadata(
            job.parent.unwrap(),
            &job.name,
            &existing,
            &job.info,
            !options.no_fsync,
        )?;
        budget
            .summary
            .transferred
            .fetch_add(transferred, Ordering::Relaxed);
        agent::verify(&job.info, &source.stat(&job.info.path, false)?.2)?;
    }
    if skip {
        return Ok(Outcome::Skipped);
    }
    if options.dry_run {
        return Ok(Outcome::Copied(if job.info.kind == 1 {
            job.info.size
        } else {
            0
        }));
    }
    let parent = job
        .parent
        .ok_or_else(|| fsutil::invalid("missing destination directory"))?;
    if job.info.kind == 3 {
        destination.link(parent, &job.name, &job.info, !options.no_fsync)?;
        return Ok(Outcome::Copied(0));
    }
    let chunk_size = options.chunk_size.unwrap_or(16 << 20);
    let stage = destination.begin(parent, &job.name, &job.info, chunk_size, !options.no_fsync)?;
    drop(metadata_timer);
    let result = (|| {
        let transfer_timer = Timer::new(&budget.summary.transfer_ns);
        let next = AtomicU64::new(0);
        let stopped = AtomicBool::new(false);
        let failure = Mutex::new(None);
        let count = job.info.size.div_ceil(chunk_size);
        let mut permits = Vec::new();
        if job.info.size >= 64 << 20 || options.manual_chunks {
            for _ in 1..count.min(options.worker_count() as u64) {
                match budget.try_acquire() {
                    Some(permit) => permits.push(permit),
                    None => break,
                }
            }
        }
        if !permits.is_empty() {
            budget.summary.chunked.fetch_add(1, Ordering::Relaxed);
        }
        let transfer =
            |source: &mut Client, destination: &mut Client, buffer: &mut [u8]| -> io::Result<()> {
                while !stopped.load(Ordering::Relaxed) {
                    fsutil::check_cancelled()?;
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= count {
                        break;
                    }
                    let offset = index * chunk_size;
                    let length = chunk_size.min(job.info.size - offset);
                    source.read_start(&job.info, None, offset, length)?;
                    destination.write_start(stage, index, options.sparse)?;
                    let mut remaining = length;
                    while remaining != 0 {
                        fsutil::check_cancelled()?;
                        if stopped.load(Ordering::Relaxed) {
                            return Err(io::Error::other("parallel SSH range failed"));
                        }
                        let take = remaining.min(buffer.len() as u64) as usize;
                        source.stream.read_exact(&mut buffer[..take])?;
                        destination.stream.write_all(&buffer[..take])?;
                        budget
                            .summary
                            .transferred
                            .fetch_add(take as u64, Ordering::Relaxed);
                        remaining -= take as u64;
                    }
                    source.read_finish()?;
                    destination.write_finish()?;
                }
                Ok(())
            };
        let record = |result: io::Result<()>| {
            if let Err(error) = result {
                let mut saved = failure.lock().unwrap();
                if saved.is_none() {
                    *saved = Some(error);
                }
                stopped.store(true, Ordering::Relaxed);
            }
        };
        thread::scope(|scope| {
            let mut helpers = Vec::new();
            for permit in permits {
                let transfer = &transfer;
                let record = &record;
                match thread::Builder::new()
                    .name("tcopy-ssh-chunk".into())
                    .spawn_scoped(scope, move || {
                        let _permit = permit;
                        record((|| {
                            transfer(
                                &mut job.source.connect()?,
                                &mut job.destination.connect()?,
                                &mut vec![0; options.buffer_size()],
                            )
                        })());
                    }) {
                    Ok(helper) => helpers.push(helper),
                    Err(error) => {
                        record(Err(error));
                        break;
                    }
                }
            }
            record(transfer(source, destination, buffer));
            for helper in helpers {
                if helper.join().is_err() {
                    record(Err(io::Error::other("SSH chunk worker panicked")));
                }
            }
        });
        if let Some(error) = failure.into_inner().unwrap() {
            return Err(error);
        }
        drop(transfer_timer);
        let _timer = Timer::new(&budget.summary.metadata_ns);
        agent::verify(&job.info, &source.stat(&job.info.path, false)?.2)?;
        fsutil::check_cancelled()?;
        destination.commit(stage)
    })();
    if result.is_err() {
        let _ = destination.abort(stage);
    }
    result?;
    Ok(Outcome::Copied(job.info.size))
}

fn checksum(
    job: &Job,
    existing: &Info,
    source: &mut Client,
    destination: &mut Client,
    left: &mut [u8],
) -> io::Result<bool> {
    let mut right = vec![0; left.len()];
    let mut offset = 0;
    let mut matches = true;
    while offset < job.info.size {
        let length = (job.info.size - offset).min(16 << 20);
        source.read_start(&job.info, None, offset, length)?;
        destination.read_start(
            existing,
            Some((job.parent.unwrap(), &job.name)),
            offset,
            length,
        )?;
        let mut remaining = length;
        while remaining != 0 {
            fsutil::check_cancelled()?;
            let take = remaining.min(left.len() as u64) as usize;
            source.stream.read_exact(&mut left[..take])?;
            destination.stream.read_exact(&mut right[..take])?;
            matches &= left[..take] == right[..take];
            remaining -= take as u64;
        }
        source.read_finish()?;
        destination.read_finish()?;
        offset += length;
    }
    let current = destination.stat_at(job.parent.unwrap(), &job.name)?;
    Ok(matches && existing.unchanged(&current))
}
