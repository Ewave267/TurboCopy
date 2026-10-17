pub mod agent;
mod copy;
#[cfg(test)]
#[path = "tests.rs"]
mod native_tests;
mod transport;
mod wire;

use crate::{
    cli::{Command, Options},
    fsutil,
    progress::{Summary, Timer},
    scheduler::Concurrency,
};
use copy::Job;
use std::{
    collections::HashMap,
    ffi::OsString,
    io,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::{atomic::Ordering, mpsc, Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};
use transport::{Client, Endpoint};

struct Operand {
    host: Option<String>,
    path: PathBuf,
    contents: bool,
    slash: bool,
}
fn operand(path: &Path) -> io::Result<Operand> {
    let raw = path.as_os_str().as_bytes();
    let first_colon = raw.iter().position(|c| *c == b':').unwrap_or(raw.len());
    let candidate = first_colon < raw.len() && !raw[..first_colon].contains(&b'/');
    let colon = if candidate
        && (raw.starts_with(b"[")
            || raw
                .iter()
                .position(|c| *c == b'@')
                .is_some_and(|at| raw.get(at + 1) == Some(&b'[')))
    {
        raw.iter()
            .position(|c| *c == b']')
            .and_then(|end| (raw.get(end + 1) == Some(&b':')).then_some(end + 1))
            .ok_or_else(|| fsutil::invalid("IPv6 SSH endpoints require [address]:path"))?
    } else {
        first_colon
    };
    let remote = colon < raw.len() && !raw[..colon].contains(&b'/');
    let (host, bytes) = if remote {
        let host = std::str::from_utf8(&raw[..colon])
            .map_err(|_| fsutil::invalid("SSH host must be ASCII"))?;
        let (user, address) = host
            .rsplit_once('@')
            .map_or((None, host), |(user, host)| (Some(user), host));
        let valid_user = user.map_or(true, |user| {
            !user.is_empty()
                && !user.starts_with('-')
                && user
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        });
        let valid_host =
            if let Some(ip) = address.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                ip.parse::<std::net::Ipv6Addr>().is_ok()
            } else {
                !address.is_empty()
                    && !address.starts_with('-')
                    && address
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            };
        if !valid_user || !valid_host || raw.get(colon + 1) == Some(&b':') {
            return Err(fsutil::invalid(
                "invalid SSH endpoint; expected [user@]host:path (not rsync daemon syntax)",
            ));
        }
        (Some(host.to_owned()), &raw[colon + 1..])
    } else {
        (None, raw)
    };
    if bytes.is_empty() || bytes.contains(&0) {
        return Err(fsutil::invalid(
            "SSH paths must not be empty or contain NUL",
        ));
    }
    let mut bytes = bytes.to_vec();
    let slash = bytes.ends_with(b"/");
    while bytes.len() > 1 && bytes.ends_with(b"/") {
        bytes.pop();
    }
    let contents = bytes == b"." || bytes == b"/" || bytes.ends_with(b"/.");
    Ok(Operand {
        host,
        path: PathBuf::from(OsString::from_vec(bytes)),
        contents,
        slash,
    })
}
pub fn requested(command: &Command) -> io::Result<bool> {
    let mut remote = false;
    for path in command
        .sources
        .iter()
        .chain(std::iter::once(&command.destination))
    {
        remote |= operand(path)?.host.is_some();
    }
    Ok(remote)
}

struct Root {
    job: Job,
}
struct Pool {
    sender: mpsc::SyncSender<Job>,
    pending: Arc<(Mutex<usize>, Condvar)>,
    summary: Arc<Summary>,
}
impl Pool {
    fn submit(&self, job: Job) -> io::Result<()> {
        fsutil::check_cancelled()?;
        self.summary.discovered.fetch_add(1, Ordering::Relaxed);
        if job.info.kind == 1 {
            let counter = if job.info.size < 1 << 20 {
                &self.summary.small_files
            } else if job.info.size < 64 << 20 {
                &self.summary.medium_files
            } else {
                &self.summary.large_files
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
        *self.pending.0.lock().unwrap() += 1;
        if self.sender.send(job).is_err() {
            *self.pending.0.lock().unwrap() -= 1;
            return Err(io::Error::other("native SSH workers stopped"));
        }
        Ok(())
    }
    fn drain(&self) {
        let mut count = self.pending.0.lock().unwrap();
        while *count != 0 {
            count = self.pending.1.wait(count).unwrap();
        }
    }
}

fn plan(command: &Command, options: Options) -> io::Result<(Vec<Root>, Client)> {
    let destination = operand(&command.destination)?;
    let mut endpoints: HashMap<Option<String>, Arc<Endpoint>> = HashMap::new();
    let mut endpoint = |host: &Option<String>| -> io::Result<Arc<Endpoint>> {
        if let Some(endpoint) = endpoints.get(host) {
            return Ok(endpoint.clone());
        }
        let endpoint = Endpoint::start(
            host.as_deref(),
            &command.ssh_program,
            options.worker_count(),
        )?;
        endpoints.insert(host.clone(), endpoint.clone());
        Ok(endpoint)
    };
    let destination_endpoint = endpoint(&destination.host)?;
    let mut destination_client = destination_endpoint.connect()?;
    let (parent, name, info) = destination_client.stat(&destination.path, true)?;
    if command.sources.len() > 1 && info.kind != 2 {
        return Err(fsutil::invalid(
            "multiple sources require an existing destination directory",
        ));
    }
    if destination.slash && info.kind != 0 && info.kind != 2 {
        return Err(fsutil::invalid(
            "a destination ending in / must be a directory",
        ));
    }
    let container = if info.kind == 2 {
        Some(destination_client.directory(parent, &name, false, false)?)
    } else {
        None
    };
    let mut roots: Vec<Root> = Vec::new();
    for path in &command.sources {
        let source = operand(path)?;
        let source_endpoint = endpoint(&source.host)?;
        let mut source_client = source_endpoint.connect()?;
        let (_, source_name, source_info) = source_client.stat(&source.path, false)?;
        if source_info.kind == 0 {
            return Err(fsutil::invalid(format!("missing source: {path:?}")));
        }
        if source_info.kind == 4 {
            return Err(fsutil::invalid(format!(
                "unsupported special source: {path:?}"
            )));
        }
        if (source.slash && source_info.kind == 1)
            || (destination.slash && container.is_none() && source_info.kind != 2)
        {
            return Err(fsutil::invalid("a regular file path must not end in /"));
        }
        let (parent, name) = if let Some(container) = container {
            (
                container,
                if source.contents {
                    OsString::from(".")
                } else {
                    source_name
                },
            )
        } else {
            (parent, name.clone())
        };
        let existing = destination_client.stat_at(parent, &name)?;
        agent::validate(&source_info, &existing)?;
        if source_info.kind == 2
            && existing.kind == 2
            && source_endpoint.same_machine(&destination_endpoint)
            && source_client.source_ancestor(&source_info, &existing)?
        {
            return Err(fsutil::invalid(
                "destination directory contains source tree",
            ));
        }
        roots.push(Root {
            job: Job {
                source: source_endpoint,
                destination: destination_endpoint.clone(),
                info: source_info,
                parent: Some(parent),
                name,
                path: existing.path,
            },
        });
    }
    for (index, root) in roots.iter().enumerate() {
        for source in &roots {
            if root.job.destination.same_machine(&source.job.source) {
                let overlaps = if source.job.info.kind == 2 {
                    root.job.path.starts_with(&source.job.info.path)
                        || source.job.info.path.starts_with(&root.job.path)
                        || destination_client
                            .ancestor(root.job.parent.unwrap(), &source.job.info)?
                } else {
                    root.job.path == source.job.info.path
                };
                let existing =
                    destination_client.stat_at(root.job.parent.unwrap(), &root.job.name)?;
                if overlaps
                    || (existing.kind != 0
                        && existing.dev == source.job.info.dev
                        && existing.ino == source.job.info.ino)
                {
                    return Err(fsutil::invalid(
                        "source and destination trees overlap or identify the same file",
                    ));
                }
            }
        }
        for other in &roots[..index] {
            if root.job.path.starts_with(&other.job.path)
                || other.job.path.starts_with(&root.job.path)
            {
                return Err(fsutil::invalid(
                    "source roots map to colliding destinations",
                ));
            }
        }
    }
    Ok((roots, destination_client))
}

struct Frame {
    job: Job,
    list: u64,
    directory: Option<u64>,
}
fn enter(
    job: Job,
    source: &mut Client,
    destination: &mut Client,
    options: Options,
) -> io::Result<Frame> {
    let list = source.list(&job.info)?;
    let directory = if let Some(parent) = job.parent {
        let existing = destination.stat_at(parent, &job.name)?;
        agent::validate(&job.info, &existing)?;
        if job.source.same_machine(&job.destination) && destination.ancestor(parent, &job.info)? {
            return Err(fsutil::invalid(
                "destination directory is inside source tree",
            ));
        }
        if existing.kind == 2
            && job.source.same_machine(&job.destination)
            && source.source_ancestor(&job.info, &existing)?
        {
            return Err(fsutil::invalid(
                "destination directory contains source tree",
            ));
        }
        if options.dry_run && existing.kind == 0 {
            options.preview(format_args!("mkdir {:?}", job.path));
            None
        } else {
            Some(destination.directory(parent, &job.name, !options.dry_run, !options.no_fsync)?)
        }
    } else {
        options.preview(format_args!("mkdir {:?}", job.path));
        None
    };
    Ok(Frame {
        job,
        list,
        directory,
    })
}
fn scan(root: Root, destination: &mut Client, options: Options, pool: &Pool) -> io::Result<()> {
    if root.job.info.kind != 2 {
        return pool.submit(root.job);
    }
    let mut source = root.job.source.connect()?;
    let mut stack = vec![pool
        .summary
        .metadata(|| enter(root.job, &mut source, destination, options))?];
    while let Some(frame) = stack.last() {
        fsutil::check_cancelled()?;
        if let Some((name, info)) = pool.summary.metadata(|| source.next(frame.list))? {
            let job = Job {
                source: frame.job.source.clone(),
                destination: frame.job.destination.clone(),
                info,
                parent: frame.directory,
                path: frame.job.path.join(&name),
                name,
            };
            if job.info.kind == 2 {
                stack.push(
                    pool.summary
                        .metadata(|| enter(job, &mut source, destination, options))?,
                );
            } else {
                pool.submit(job)?;
            }
        } else {
            let frame = stack.pop().unwrap();
            pool.drain();
            let _timer = Timer::new(&pool.summary.metadata_ns);
            agent::verify(
                &frame.job.info,
                &source.stat(&frame.job.info.path, false)?.2,
            )?;
            source.close_list(frame.list)?;
            if let Some(directory) = frame.directory {
                if !options.dry_run {
                    destination.finish_dir(directory, &frame.job.info, !options.no_fsync)?;
                }
                destination.close_dir(directory)?;
            }
        }
    }
    Ok(())
}

pub fn run(command: &Command, start: Instant) -> Arc<Summary> {
    let mut options = command.options;
    let mut summary = Summary::new(options, Vec::new(), start);
    summary.backend = "native-ssh";
    summary.worker_policy = if options.manual_workers {
        "manual"
    } else {
        "ssh-fixed"
    };
    *summary.tuning_state.lock().unwrap() = "ssh-fixed";
    // Remote topology is unknown; keep a conservative, explicit budget.
    options.workers = Some(options.worker_count());
    summary.options = options;
    let summary = Arc::new(summary);
    if options.resume
        || options.reflink
        || options.preserve_extended
        || options.tune
        || options.gpfs
    {
        summary.fail(
            "SSH copies do not support --resume, --reflink, --preserve-extended, --tune or --gpfs",
        );
        return summary;
    }
    let (roots, mut destination) = match plan(command, options) {
        Ok(plan) => plan,
        Err(error) => {
            summary.fail(error);
            return summary;
        }
    };
    let _destination_endpoint = roots.first().map(|root| root.job.destination.clone());
    summary.metadata_ns.store(
        start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
    let budget = Arc::new(Concurrency::new(summary.clone()));
    thread::scope(|scope| {
        let (stop, stopped) = mpsc::channel::<()>();
        if options.progress {
            let summary = summary.clone();
            scope.spawn(move || loop {
                summary.progress();
                if stopped.recv_timeout(Duration::from_millis(250)).is_ok() {
                    break;
                }
            });
        }
        let (sender, receiver) = mpsc::sync_channel::<Job>(64);
        let receiver = Arc::new(Mutex::new(receiver));
        let pending = Arc::new((Mutex::new(0), Condvar::new()));
        let pool = Pool {
            sender,
            pending: pending.clone(),
            summary: summary.clone(),
        };
        for number in 0..options.worker_count() {
            let receiver = receiver.clone();
            let pending = pending.clone();
            let budget = budget.clone();
            let worker_summary = summary.clone();
            if let Err(error) = thread::Builder::new()
                .name(format!("tcopy-ssh-{number}"))
                .spawn_scoped(scope, move || {
                    let mut buffer = vec![0; options.buffer_size()];
                    let mut worker = copy::Worker::default();
                    loop {
                        let job = match receiver.lock().unwrap().recv() {
                            Ok(job) => job,
                            Err(_) => break,
                        };
                        let _permit = budget.acquire();
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            worker.run(&job, options, &budget, &mut buffer)
                        }));
                        match result {
                            Ok(Ok(outcome)) => worker_summary.observe(outcome),
                            Ok(Err(error)) => worker_summary
                                .fail(format!("{:?} -> {:?}: {error}", job.info.path, job.path)),
                            Err(_) => worker_summary.fail("native SSH worker panicked"),
                        }
                        let mut count = pending.0.lock().unwrap();
                        *count -= 1;
                        pending.1.notify_all();
                    }
                })
            {
                summary.fail(error);
                drop(pool);
                let _ = stop.send(());
                return;
            }
        }
        for root in roots {
            if let Err(error) = scan(root, &mut destination, options, &pool) {
                summary.fail(error);
                break;
            }
        }
        pool.drain();
        drop(pool);
        let _ = stop.send(());
    });
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_hosts_ipv6_and_literal_local_paths() {
        let remote = operand(Path::new("alice@server:/data/")).unwrap();
        assert_eq!(remote.host.as_deref(), Some("alice@server"));
        assert_eq!(remote.path, Path::new("/data"));
        assert!(remote.slash);
        assert_eq!(
            operand(Path::new("user@[::1]:relative"))
                .unwrap()
                .host
                .as_deref(),
            Some("user@[::1]")
        );
        assert!(operand(Path::new("./local:colon")).unwrap().host.is_none());
        assert!(operand(Path::new("[local].txt")).unwrap().host.is_none());
        assert!(operand(Path::new("host:dir/.")).unwrap().contents);
        for invalid in [
            "host:",
            "host::module",
            "-host:file",
            "user@-host:file",
            "bad;host:file",
            "bad user@host:file",
            "[::1]file",
        ] {
            assert!(operand(Path::new(invalid)).is_err(), "{invalid}");
        }
    }
}
