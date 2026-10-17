use super::{
    agent::{Server, TIMEOUT},
    wire::{self, Info},
};
use crate::fsutil;
use std::{
    collections::{HashMap, VecDeque},
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        process::CommandExt,
    },
    path::Path,
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};

pub struct Endpoint {
    pub address: SocketAddr,
    pub token: [u8; 32],
    pub identity: Vec<u8>,
    _resource: Resource,
}
enum Resource {
    Local { _server: Server },
    Remote { agent: Process, tunnel: Process },
}
impl Drop for Resource {
    fn drop(&mut self) {
        if let Self::Remote { agent, tunnel } = self {
            agent.stop();
            tunnel.stop();
        }
    }
}
struct Process(Option<Child>);
extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}
impl Process {
    fn stop(&mut self) {
        if let Some(mut child) = self.0.take() {
            drop(child.stdin.take());
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            // SAFETY: each owned SSH child starts a private process group; reap it below.
            unsafe {
                kill(-(child.id() as i32), 9);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

fn ssh_host(host: &str) -> String {
    match host.rsplit_once('@') {
        Some((user, address)) => format!("{user}@{}", address.trim_matches(['[', ']'])),
        None => host.trim_matches(['[', ']']).to_owned(),
    }
}
fn ssh() -> Command {
    let mut command = Command::new("ssh");
    command.args([
        "-T",
        "-oBatchMode=yes",
        "-oConnectTimeout=15",
        "-oServerAliveInterval=10",
        "-oServerAliveCountMax=3",
        "-oControlMaster=no",
        "-oControlPath=none",
        "-oControlPersist=no",
        "-oForwardAgent=no",
    ]);
    command.process_group(0);
    command
}
pub fn quote(value: &OsStr) -> OsString {
    let mut result = vec![b'\''];
    for byte in value.as_bytes() {
        if *byte == b'\'' {
            result.extend_from_slice(b"'\\''");
        } else {
            result.push(*byte);
        }
    }
    result.push(b'\'');
    OsString::from_vec(result)
}

impl Endpoint {
    pub fn start(host: Option<&str>, program: &OsStr, workers: usize) -> io::Result<Arc<Self>> {
        let mut token = [0; 32];
        File::open("/dev/urandom")?.read_exact(&mut token)?;
        let endpoint = if let Some(host) = host {
            let mut command = ssh();
            command
                .arg("-oClearAllForwardings=yes")
                .arg("--")
                .arg(ssh_host(host));
            let mut remote = OsString::from("exec ");
            remote.push(quote(program));
            remote.push(" --ssh-agent");
            let child = command
                .arg(remote)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!("cannot start OpenSSH for {host}: {error}"),
                    )
                })?;
            let mut agent = Process(Some(child));
            let child = agent.0.as_mut().unwrap();
            child.stdin.as_mut().unwrap().write_all(&token)?;
            wire::put_number(child.stdin.as_mut().unwrap(), workers as u64)?;
            let mut stdout = child.stdout.take().unwrap();
            let (sender, receiver) = mpsc::sync_channel(1);
            let reader = thread::spawn(move || {
                let result = (|| {
                    let mut magic = [0; 8];
                    stdout.read_exact(&mut magic)?;
                    if &magic != wire::MAGIC {
                        return Err(fsutil::invalid(
                            "incompatible remote tcopy agent or unexpected SSH stdout",
                        ));
                    }
                    let port = wire::number(&mut stdout)?;
                    u16::try_from(port)
                        .ok()
                        .filter(|port| *port != 0)
                        .ok_or_else(|| fsutil::invalid("invalid remote agent port"))
                })();
                let _ = sender.send(result);
            });
            let deadline = Instant::now() + TIMEOUT;
            let port = loop {
                match receiver.recv_timeout(Duration::from_millis(50)) {
                    Ok(result) => break result,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break Err(io::Error::other("remote agent startup reader stopped"))
                    }
                    Err(_) => {
                        if let Err(error) = fsutil::check_cancelled() {
                            break Err(error);
                        }
                        if Instant::now() >= deadline {
                            break Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "remote tcopy agent startup timed out",
                            ));
                        }
                    }
                }
            };
            if port.is_err() {
                agent.stop();
            }
            let _ = reader.join();
            let port = port.map_err(|error| io::Error::new(error.kind(), format!("{host}: cannot start remote tcopy (install it or use --ssh-program): {error}")))?;
            let reserved = TcpListener::bind(("127.0.0.1", 0))?;
            let address = reserved.local_addr()?;
            drop(reserved);
            let mut tunnel_command = ssh();
            // Two authenticated SSH connections avoid relying on remote-to-remote reachability.
            tunnel_command
                .args(["-N", "-oExitOnForwardFailure=yes", "-L"])
                .arg(format!("127.0.0.1:{}:127.0.0.1:{port}", address.port()));
            tunnel_command.arg("--").arg(ssh_host(host));
            let mut tunnel = Process(Some(
                tunnel_command
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()?,
            ));
            let deadline = Instant::now() + TIMEOUT;
            loop {
                fsutil::check_cancelled()?;
                if let Some(status) = tunnel.0.as_mut().unwrap().try_wait()? {
                    return Err(io::Error::other(format!(
                        "SSH forwarding to {host} failed: {status}"
                    )));
                }
                if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "SSH forward did not become ready",
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Self {
                address,
                token,
                identity: Vec::new(),
                _resource: Resource::Remote { agent, tunnel },
            }
        } else {
            let server = Server::start(token, 4 * workers + 8)?;
            Self {
                address: server.address,
                token: server.token,
                identity: Vec::new(),
                _resource: Resource::Local { _server: server },
            }
        };
        let mut endpoint = endpoint;
        let (_, identity) = Client::connect_raw(&endpoint)?;
        endpoint.identity = identity;
        Ok(Arc::new(endpoint))
    }
    pub fn connect(&self) -> io::Result<Client> {
        Client::connect_raw(self).map(|pair| pair.0)
    }
    pub fn same_machine(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

type DirectoryBatch = (VecDeque<(OsString, Info)>, bool);
pub struct Client {
    pub stream: wire::Stream,
    lists: HashMap<u64, DirectoryBatch>,
}
impl Client {
    fn connect_raw(endpoint: &Endpoint) -> io::Result<(Self, Vec<u8>)> {
        let stream = TcpStream::connect_timeout(&endpoint.address, TIMEOUT)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let mut stream = wire::Stream::new(stream);
        stream.write_all(wire::MAGIC)?;
        stream.write_all(&endpoint.token)?;
        wire::status(&mut stream)?;
        let identity = wire::bytes(&mut stream)?;
        Ok((
            Self {
                stream,
                lists: HashMap::new(),
            },
            identity,
        ))
    }
    fn op(&mut self, op: u8) -> io::Result<()> {
        fsutil::check_cancelled()?;
        self.stream.write_all(&[op])
    }
    pub fn stat(&mut self, path: &Path, destination: bool) -> io::Result<(u64, OsString, Info)> {
        self.op(wire::STAT)?;
        wire::put_path(&mut self.stream, path.as_os_str())?;
        self.stream.write_all(&[u8::from(destination)])?;
        wire::status(&mut self.stream)?;
        Ok((
            wire::number(&mut self.stream)?,
            wire::name(&mut self.stream)?,
            Info::read(&mut self.stream)?,
        ))
    }
    pub fn stat_at(&mut self, parent: u64, name: &OsStr) -> io::Result<Info> {
        self.op(wire::STAT_AT)?;
        wire::put_number(&mut self.stream, parent)?;
        wire::put_path(&mut self.stream, name)?;
        wire::status(&mut self.stream)?;
        Info::read(&mut self.stream)
    }
    pub fn sync_metadata(
        &mut self,
        parent: u64,
        name: &OsStr,
        expected: &Info,
        desired: &Info,
        fsync: bool,
    ) -> io::Result<u64> {
        self.op(wire::SYNC_METADATA)?;
        wire::put_number(&mut self.stream, parent)?;
        wire::put_path(&mut self.stream, name)?;
        expected.write(&mut self.stream)?;
        desired.write(&mut self.stream)?;
        self.stream.write_all(&[u8::from(fsync)])?;
        wire::status(&mut self.stream)?;
        wire::number(&mut self.stream)
    }
    pub fn list(&mut self, info: &Info) -> io::Result<u64> {
        self.op(wire::LIST)?;
        info.write(&mut self.stream)?;
        wire::status(&mut self.stream)?;
        wire::number(&mut self.stream)
    }
    pub fn next(&mut self, id: u64) -> io::Result<Option<(OsString, Info)>> {
        if let Some((entries, finished)) = self.lists.get_mut(&id) {
            if let Some(entry) = entries.pop_front() {
                return Ok(Some(entry));
            }
            if *finished {
                return Ok(None);
            }
        }
        self.op(wire::NEXT)?;
        wire::put_number(&mut self.stream, id)?;
        wire::put_number(&mut self.stream, 64)?;
        wire::status(&mut self.stream)?;
        let count = wire::number(&mut self.stream)?;
        if count > 64 {
            return Err(fsutil::invalid("oversized directory response"));
        }
        let mut entries = VecDeque::new();
        for _ in 0..count {
            let name = wire::name(&mut self.stream)?;
            if name == "." {
                return Err(fsutil::invalid("invalid directory entry"));
            }
            entries.push_back((name, Info::read(&mut self.stream)?));
        }
        let entry = entries.pop_front();
        self.lists.insert(id, (entries, count < 64));
        Ok(entry)
    }
    fn handle_op(&mut self, op: u8, id: u64) -> io::Result<()> {
        self.op(op)?;
        wire::put_number(&mut self.stream, id)?;
        wire::status(&mut self.stream)
    }
    pub fn close_list(&mut self, id: u64) -> io::Result<()> {
        self.lists.remove(&id);
        self.handle_op(wire::CLOSE_LIST, id)
    }
    pub fn close_dir(&mut self, id: u64) -> io::Result<()> {
        self.handle_op(wire::CLOSE_DIR, id)
    }
    pub fn commit(&mut self, id: u64) -> io::Result<()> {
        self.handle_op(wire::COMMIT, id)
    }
    pub fn abort(&mut self, id: u64) -> io::Result<()> {
        self.handle_op(wire::ABORT, id)
    }
    pub fn directory(
        &mut self,
        parent: u64,
        name: &OsStr,
        create: bool,
        fsync: bool,
    ) -> io::Result<u64> {
        self.op(wire::DIRECTORY)?;
        wire::put_number(&mut self.stream, parent)?;
        wire::put_path(&mut self.stream, name)?;
        self.stream
            .write_all(&[u8::from(create), u8::from(fsync)])?;
        wire::status(&mut self.stream)?;
        wire::number(&mut self.stream)
    }
    pub fn finish_dir(&mut self, id: u64, info: &Info, fsync: bool) -> io::Result<()> {
        self.op(wire::FINISH_DIR)?;
        wire::put_number(&mut self.stream, id)?;
        info.write(&mut self.stream)?;
        self.stream.write_all(&[u8::from(fsync)])?;
        wire::status(&mut self.stream)
    }
    pub fn begin(
        &mut self,
        parent: u64,
        name: &OsStr,
        info: &Info,
        size: u64,
        fsync: bool,
    ) -> io::Result<u64> {
        self.op(wire::BEGIN)?;
        wire::put_number(&mut self.stream, parent)?;
        wire::put_path(&mut self.stream, name)?;
        info.write(&mut self.stream)?;
        wire::put_number(&mut self.stream, size)?;
        self.stream.write_all(&[u8::from(fsync)])?;
        wire::status(&mut self.stream)?;
        wire::number(&mut self.stream)
    }
    pub fn write_start(&mut self, id: u64, index: u64, sparse: bool) -> io::Result<()> {
        self.op(wire::WRITE_RANGE)?;
        wire::put_number(&mut self.stream, id)?;
        wire::put_number(&mut self.stream, index)?;
        self.stream.write_all(&[u8::from(sparse)])?;
        wire::status(&mut self.stream)
    }
    pub fn write_finish(&mut self) -> io::Result<()> {
        wire::ok(&mut self.stream)?;
        wire::status(&mut self.stream)
    }
    pub fn read_start(
        &mut self,
        info: &Info,
        at: Option<(u64, &OsStr)>,
        offset: u64,
        length: u64,
    ) -> io::Result<()> {
        if let Some((parent, name)) = at {
            self.op(wire::READ_AT)?;
            wire::put_number(&mut self.stream, parent)?;
            wire::put_path(&mut self.stream, name)?;
        } else {
            self.op(wire::READ_RANGE)?;
            wire::put_path(&mut self.stream, info.path.as_os_str())?;
        }
        info.write(&mut self.stream)?;
        wire::put_number(&mut self.stream, offset)?;
        wire::put_number(&mut self.stream, length)?;
        wire::status(&mut self.stream)
    }
    pub fn read_finish(&mut self) -> io::Result<()> {
        wire::status(&mut self.stream)
    }
    pub fn link(&mut self, parent: u64, name: &OsStr, info: &Info, fsync: bool) -> io::Result<()> {
        self.op(wire::LINK)?;
        wire::put_number(&mut self.stream, parent)?;
        wire::put_path(&mut self.stream, name)?;
        info.write(&mut self.stream)?;
        self.stream.write_all(&[u8::from(fsync)])?;
        wire::status(&mut self.stream)
    }
    pub fn ancestor(&mut self, directory: u64, info: &Info) -> io::Result<bool> {
        self.op(wire::ANCESTOR)?;
        wire::put_number(&mut self.stream, directory)?;
        info.write(&mut self.stream)?;
        wire::status(&mut self.stream)?;
        Ok(wire::byte(&mut self.stream)? != 0)
    }
    pub fn source_ancestor(&mut self, source: &Info, info: &Info) -> io::Result<bool> {
        self.op(wire::PATH_ANCESTOR)?;
        source.write(&mut self.stream)?;
        info.write(&mut self.stream)?;
        wire::status(&mut self.stream)?;
        Ok(wire::byte(&mut self.stream)? != 0)
    }
}
