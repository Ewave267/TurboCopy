use super::wire::{self, Info};
use crate::{
    copy_engine::{self, Temporary},
    fsutil::{self, Directory},
};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::{self, File, FileTimes, OpenOptions, ReadDir},
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const TIMEOUT: Duration = Duration::from_secs(30);

struct Stage {
    parent: Directory,
    name: OsString,
    temporary: Mutex<Temporary>,
    file: File,
    info: Info,
    size: u64,
    chunks: Mutex<Vec<u8>>,
    fsync: bool,
}
impl Stage {
    fn range(&self, index: u64) -> io::Result<(u64, usize)> {
        let offset = index
            .checked_mul(self.size)
            .ok_or_else(|| fsutil::invalid("invalid chunk offset"))?;
        if offset >= self.info.size {
            return Err(fsutil::invalid("chunk outside file"));
        }
        Ok((offset, self.size.min(self.info.size - offset) as usize))
    }
}

#[derive(Default)]
struct State {
    next: AtomicU64,
    directories: Mutex<HashMap<u64, Directory>>,
    stages: Mutex<HashMap<u64, Arc<Stage>>>,
    streams: Mutex<HashMap<u64, TcpStream>>,
}
impl State {
    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }
    fn directory(&self, id: u64) -> io::Result<Directory> {
        self.directories
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| fsutil::invalid("unknown directory handle"))
    }
    fn stage(&self, id: u64) -> io::Result<Arc<Stage>> {
        self.stages
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| fsutil::invalid("unknown file stage"))
    }
}

pub struct Server {
    pub address: SocketAddr,
    pub token: [u8; 32],
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    pub fn start(token: [u8; 32], limit: usize) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let state = Arc::new(State::default());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = thread::Builder::new()
            .name("tcopy-agent".into())
            .spawn(move || {
                let mut threads: Vec<JoinHandle<()>> = Vec::new();
                while !stopped.load(Ordering::Relaxed) {
                    threads.retain_mut(|thread| !thread.is_finished());
                    match listener.accept() {
                        Ok((stream, _)) if threads.len() < limit => {
                            let state = state.clone();
                            match thread::Builder::new()
                                .name("tcopy-agent-stream".into())
                                .spawn(move || {
                                    let _ = Session::new(stream, state)
                                        .and_then(|mut session| session.serve(&token));
                                }) {
                                Ok(thread) => threads.push(thread),
                                Err(error) => eprintln!("tcopy: agent worker: {error}"),
                            }
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(error) => {
                            eprintln!("tcopy: agent accept: {error}");
                            break;
                        }
                    }
                }
                for stream in state.streams.lock().unwrap().values() {
                    let _ = stream.shutdown(Shutdown::Both);
                }
                for thread in threads {
                    let _ = thread.join();
                }
            })?;
        Ok(Self {
            address,
            token,
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn run() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut token = [0; 32];
    input.read_exact(&mut token)?;
    let workers = wire::number(&mut input)?;
    if !(1..=128).contains(&workers) {
        return Err(fsutil::invalid("invalid SSH worker budget"));
    }
    let server = Server::start(token, 4 * workers as usize + 8)?;
    let mut output = io::stdout().lock();
    output.write_all(wire::MAGIC)?;
    wire::put_number(&mut output, server.address.port() as u64)?;
    output.flush()?;
    // The authenticated SSH stdin is the agent's lease. EOF stops all native streams.
    let mut byte = [0];
    while input.read(&mut byte)? != 0 {}
    drop(server);
    Ok(())
}

struct Session {
    stream: wire::Stream,
    state: Arc<State>,
    id: u64,
    directories: Vec<u64>,
    stages: Vec<u64>,
    lists: HashMap<u64, (Directory, ReadDir)>,
    buffer: Vec<u8>,
}
impl Session {
    fn new(stream: TcpStream, state: Arc<State>) -> io::Result<Self> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let id = state.id();
        state
            .streams
            .lock()
            .unwrap()
            .insert(id, stream.try_clone()?);
        Ok(Self {
            stream: wire::Stream::new(stream),
            state,
            id,
            directories: Vec::new(),
            stages: Vec::new(),
            lists: HashMap::new(),
            buffer: vec![0; 128 << 10],
        })
    }
    fn retain(&mut self, directory: Directory) -> u64 {
        let id = self.state.id();
        self.state.directories.lock().unwrap().insert(id, directory);
        self.directories.push(id);
        id
    }
    fn serve(&mut self, token: &[u8; 32]) -> io::Result<()> {
        let mut magic = [0; 8];
        self.stream.read_exact(&mut magic)?;
        let mut supplied = [0; 32];
        self.stream.read_exact(&mut supplied)?;
        let mismatch = supplied
            .iter()
            .zip(token)
            .fold(0, |sum, (a, b)| sum | (a ^ b));
        if &magic != wire::MAGIC || mismatch != 0 {
            return Err(fsutil::invalid("SSH agent authentication/version mismatch"));
        }
        wire::ok(&mut self.stream)?;
        wire::put_bytes(
            &mut self.stream,
            &fs::read("/proc/sys/kernel/random/boot_id")?,
        )?;
        self.stream.flush()?;
        self.stream.set_read_timeout(None)?;
        while let Ok(op) = wire::byte(&mut self.stream) {
            self.stream.set_read_timeout(Some(TIMEOUT))?;
            if let Err(error) = self.dispatch(op) {
                let _ = wire::error(&mut self.stream, &error);
                let _ = self.stream.flush();
                return Err(error);
            }
            self.stream.flush()?;
            self.stream.set_read_timeout(None)?;
        }
        Ok(())
    }
    fn dispatch(&mut self, op: u8) -> io::Result<()> {
        match op {
            wire::STAT => {
                let path = wire::path(&mut self.stream)?;
                let destination = wire::byte(&mut self.stream)? != 0;
                use std::os::unix::ffi::OsStrExt;
                let contents = path == Path::new(".")
                    || path == Path::new("/")
                    || path.as_os_str().as_bytes().ends_with(b"/.");
                let (parent, name) = if contents {
                    (
                        Directory::open(&if destination {
                            path.clone()
                        } else {
                            fs::canonicalize(&path)?
                        })?,
                        OsString::from("."),
                    )
                } else {
                    let parent_path = path
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(Path::new("."));
                    let parent = Directory::open(&if destination {
                        parent_path.to_owned()
                    } else {
                        fs::canonicalize(parent_path)?
                    })?;
                    (
                        parent,
                        path.file_name()
                            .ok_or_else(|| fsutil::invalid("path has no filename"))?
                            .to_owned(),
                    )
                };
                let info = stat(&parent, &name)?;
                let id = if destination { self.retain(parent) } else { 0 };
                wire::ok(&mut self.stream)?;
                wire::put_number(&mut self.stream, id)?;
                wire::put_path(&mut self.stream, &name)?;
                info.write(&mut self.stream)
            }
            wire::STAT_AT => {
                let parent = self.state.directory(wire::number(&mut self.stream)?)?;
                let name = wire::name(&mut self.stream)?;
                let info = stat(&parent, &name)?;
                wire::ok(&mut self.stream)?;
                info.write(&mut self.stream)
            }
            wire::LIST => {
                let expected = Info::read(&mut self.stream)?;
                let directory = Directory::open(&expected.path)?;
                verify(
                    &expected,
                    &Info::new(&directory.metadata()?, expected.path.clone()),
                )?;
                let entries = fs::read_dir(directory.path())?;
                let id = self.state.id();
                self.lists.insert(id, (directory, entries));
                wire::ok(&mut self.stream)?;
                wire::put_number(&mut self.stream, id)
            }
            wire::NEXT => {
                let id = wire::number(&mut self.stream)?;
                let count = wire::number(&mut self.stream)?;
                if !(1..=64).contains(&count) {
                    return Err(fsutil::invalid("invalid directory batch size"));
                }
                let (directory, entries) = self
                    .lists
                    .get_mut(&id)
                    .ok_or_else(|| fsutil::invalid("unknown directory iterator"))?;
                let mut batch = Vec::new();
                for _ in 0..count {
                    let Some(entry) = entries.next().transpose()? else {
                        break;
                    };
                    let name = entry.file_name();
                    batch.push((name.clone(), stat(directory, &name)?));
                }
                let mut response = Vec::new();
                wire::ok(&mut response)?;
                wire::put_number(&mut response, batch.len() as u64)?;
                for (name, info) in batch {
                    wire::put_path(&mut response, &name)?;
                    info.write(&mut response)?;
                }
                self.stream.write_all(&response)
            }
            wire::CLOSE_LIST => {
                self.lists.remove(&wire::number(&mut self.stream)?);
                wire::ok(&mut self.stream)
            }
            wire::DIRECTORY => {
                let parent = self.state.directory(wire::number(&mut self.stream)?)?;
                let name = wire::name(&mut self.stream)?;
                let create = wire::byte(&mut self.stream)? != 0;
                let fsync = wire::byte(&mut self.stream)? != 0;
                let directory = if create {
                    parent.ensure_child(&name, fsync)?
                } else {
                    parent.child(&name)?
                };
                let id = self.retain(directory);
                wire::ok(&mut self.stream)?;
                wire::put_number(&mut self.stream, id)
            }
            wire::CLOSE_DIR => {
                let id = wire::number(&mut self.stream)?;
                if !self.directories.contains(&id) {
                    return Err(fsutil::invalid("directory handle owned by another stream"));
                }
                self.state.directories.lock().unwrap().remove(&id);
                self.directories.retain(|saved| *saved != id);
                wire::ok(&mut self.stream)
            }
            wire::FINISH_DIR => {
                let directory = self.state.directory(wire::number(&mut self.stream)?)?;
                let info = Info::read(&mut self.stream)?;
                let fsync = wire::byte(&mut self.stream)? != 0;
                apply(directory.as_file(), &info, fsync)?;
                wire::ok(&mut self.stream)
            }
            wire::BEGIN => {
                let parent = self.state.directory(wire::number(&mut self.stream)?)?;
                let name = wire::name(&mut self.stream)?;
                let info = Info::read(&mut self.stream)?;
                let size = wire::number(&mut self.stream)?;
                let fsync = wire::byte(&mut self.stream)? != 0;
                if info.kind != 1 || !(1024..=wire::MAX_RANGE).contains(&size) {
                    return Err(fsutil::invalid("invalid native transfer stage"));
                }
                let count = info.size.div_ceil(size);
                if count > 1 << 24 {
                    return Err(fsutil::invalid(
                        "too many chunks; use a larger --chunk-size",
                    ));
                }
                validate(&info, &stat(&parent, &name)?)?;
                let (temporary, file) = copy_engine::temporary(&parent, None)?;
                let file = file.unwrap();
                file.set_len(info.size)?;
                let stage = Arc::new(Stage {
                    parent,
                    name,
                    temporary: Mutex::new(temporary),
                    file,
                    info,
                    size,
                    chunks: Mutex::new(vec![0; count as usize]),
                    fsync,
                });
                let id = self.state.id();
                self.state.stages.lock().unwrap().insert(id, stage);
                self.stages.push(id);
                wire::ok(&mut self.stream)?;
                wire::put_number(&mut self.stream, id)
            }
            wire::WRITE_RANGE => {
                let stage = self.state.stage(wire::number(&mut self.stream)?)?;
                let index = wire::number(&mut self.stream)?;
                let sparse = wire::byte(&mut self.stream)? != 0;
                let (mut offset, mut remaining) = stage.range(index)?;
                {
                    let mut chunks = stage.chunks.lock().unwrap();
                    if chunks[index as usize] != 0 {
                        return Err(fsutil::invalid("duplicate or overlapping chunk"));
                    }
                    chunks[index as usize] = 1;
                }
                wire::ok(&mut self.stream)?;
                self.stream.flush()?;
                while remaining != 0 {
                    let take = remaining.min(self.buffer.len());
                    self.stream.read_exact(&mut self.buffer[..take])?;
                    if !sparse || self.buffer[..take].iter().any(|byte| *byte != 0) {
                        stage.file.write_all_at(&self.buffer[..take], offset)?;
                    }
                    offset += take as u64;
                    remaining -= take;
                }
                // The coordinator sends completion only after the source's post-read validation.
                wire::status(&mut self.stream)?;
                stage.chunks.lock().unwrap()[index as usize] = 2;
                wire::ok(&mut self.stream)
            }
            wire::COMMIT => {
                let id = wire::number(&mut self.stream)?;
                if !self.stages.contains(&id) {
                    return Err(fsutil::invalid("stage owned by another stream"));
                }
                let stage = self.state.stage(id)?;
                if stage.chunks.lock().unwrap().iter().any(|state| *state != 2) {
                    return Err(fsutil::invalid("incomplete native transfer"));
                }
                validate(&stage.info, &stat(&stage.parent, &stage.name)?)?;
                apply(&stage.file, &stage.info, stage.fsync)?;
                stage
                    .temporary
                    .lock()
                    .unwrap()
                    .publish(&stage.parent.path().join(&stage.name))?;
                if stage.fsync {
                    stage.parent.sync()?;
                }
                self.state.stages.lock().unwrap().remove(&id);
                self.stages.retain(|saved| *saved != id);
                wire::ok(&mut self.stream)
            }
            wire::ABORT => {
                let id = wire::number(&mut self.stream)?;
                if !self.stages.contains(&id) {
                    return Err(fsutil::invalid("stage owned by another stream"));
                }
                self.state.stages.lock().unwrap().remove(&id);
                self.stages.retain(|saved| *saved != id);
                wire::ok(&mut self.stream)
            }
            wire::READ_RANGE | wire::READ_AT => {
                let (parent, name) = if op == wire::READ_AT {
                    (
                        self.state.directory(wire::number(&mut self.stream)?)?,
                        wire::name(&mut self.stream)?,
                    )
                } else {
                    let path = wire::path(&mut self.stream)?;
                    (
                        Directory::open(path.parent().unwrap_or(Path::new(".")))?,
                        path.file_name()
                            .ok_or_else(|| fsutil::invalid("invalid source file"))?
                            .to_owned(),
                    )
                };
                let expected = Info::read(&mut self.stream)?;
                let mut offset = wire::number(&mut self.stream)?;
                let mut remaining = wire::number(&mut self.stream)?;
                if expected.kind != 1
                    || remaining > wire::MAX_RANGE
                    || offset
                        .checked_add(remaining)
                        .map_or(true, |end| end > expected.size)
                {
                    return Err(fsutil::invalid("invalid source range"));
                }
                let path = parent.path().join(&name);
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(fsutil::NOFOLLOW | fsutil::NONBLOCK)
                    .open(&path)?;
                verify(
                    &expected,
                    &Info::new(&file.metadata()?, expected.path.clone()),
                )?;
                wire::ok(&mut self.stream)?;
                while remaining != 0 {
                    let take = remaining.min(self.buffer.len() as u64) as usize;
                    file.read_exact_at(&mut self.buffer[..take], offset)?;
                    self.stream.write_all(&self.buffer[..take])?;
                    offset += take as u64;
                    remaining -= take as u64;
                }
                verify(
                    &expected,
                    &Info::new(&file.metadata()?, expected.path.clone()),
                )?;
                verify(&expected, &stat(&parent, &name)?)?;
                wire::ok(&mut self.stream)
            }
            wire::LINK => {
                let parent = self.state.directory(wire::number(&mut self.stream)?)?;
                let name = wire::name(&mut self.stream)?;
                let info = Info::read(&mut self.stream)?;
                let fsync = wire::byte(&mut self.stream)? != 0;
                if info.kind != 3 {
                    return Err(fsutil::invalid("invalid link metadata"));
                }
                validate(&info, &stat(&parent, &name)?)?;
                let (mut temporary, _) = copy_engine::temporary(&parent, Some(&info.link))?;
                parent.link_mtime_parts(
                    temporary.path.file_name().unwrap(),
                    info.mtime,
                    info.mtime_ns,
                )?;
                temporary.publish(&parent.path().join(name))?;
                if fsync {
                    parent.sync()?;
                }
                wire::ok(&mut self.stream)
            }
            wire::ANCESTOR => {
                let directory = self.state.directory(wire::number(&mut self.stream)?)?;
                let info = Info::read(&mut self.stream)?;
                let found = has_ancestor(directory, &info)?;
                wire::ok(&mut self.stream)?;
                self.stream.write_all(&[u8::from(found)])
            }
            wire::PATH_ANCESTOR => {
                let source = Info::read(&mut self.stream)?;
                let info = Info::read(&mut self.stream)?;
                let directory = Directory::open(&source.path)?;
                verify(
                    &source,
                    &Info::new(&directory.metadata()?, source.path.clone()),
                )?;
                let found = has_ancestor(directory, &info)?;
                wire::ok(&mut self.stream)?;
                self.stream.write_all(&[u8::from(found)])
            }
            _ => Err(fsutil::invalid("unknown SSH protocol operation")),
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.state.streams.lock().unwrap().remove(&self.id);
        let mut stages = self.state.stages.lock().unwrap();
        for id in &self.stages {
            stages.remove(id);
        }
        let mut directories = self.state.directories.lock().unwrap();
        for id in &self.directories {
            directories.remove(id);
        }
    }
}

use std::os::unix::fs::MetadataExt;
fn has_ancestor(mut current: Directory, info: &Info) -> io::Result<bool> {
    loop {
        let metadata = current.metadata()?;
        if metadata.dev() == info.dev && metadata.ino() == info.ino {
            return Ok(true);
        }
        let parent = current.child(OsStr::new(".."))?;
        if fsutil::same_file(&metadata, &parent.metadata()?) {
            return Ok(false);
        }
        current = parent;
    }
}
pub fn verify(expected: &Info, current: &Info) -> io::Result<()> {
    if expected.unchanged(current) {
        Ok(())
    } else {
        Err(fsutil::invalid(
            "source changed during copying; no consistent snapshot",
        ))
    }
}
pub fn validate(source: &Info, destination: &Info) -> io::Result<()> {
    if destination.kind == 4 {
        return Err(fsutil::invalid(
            "refusing to replace a special destination file",
        ));
    }
    if destination.kind != 0 && (source.kind == 2) != (destination.kind == 2) {
        return Err(fsutil::invalid(
            "cannot replace a directory with a file or link, or vice versa",
        ));
    }
    Ok(())
}
fn stat(parent: &Directory, name: &OsStr) -> io::Result<Info> {
    let path = parent.path().join(name);
    let canonical = parent.real_path()?.join(name);
    let Some(metadata) = fsutil::optional_metadata(&path)? else {
        return Ok(Info {
            path: canonical,
            ..Info::default()
        });
    };
    let mut info = Info::new(&metadata, canonical);
    if info.kind == 3 {
        info.link = fs::read_link(&path)?;
        verify(
            &info,
            &Info::new(&fs::symlink_metadata(path)?, info.path.clone()),
        )?;
    }
    Ok(info)
}
fn modified(info: &Info) -> io::Result<SystemTime> {
    let seconds = info.mtime;
    let nanos = Duration::from_nanos(info.mtime_ns as u64);
    (if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))
    })
    .and_then(|time| time.checked_add(nanos))
    .ok_or_else(|| fsutil::invalid("unsupported file timestamp"))
}
fn apply(file: &File, info: &Info, fsync: bool) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(info.mode & 0o7777))?;
    file.set_times(FileTimes::new().set_modified(modified(info)?))?;
    if fsync {
        file.sync_all()?;
    }
    Ok(())
}
