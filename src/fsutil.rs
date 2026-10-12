use std::{
    ffi::{c_char, c_int, c_long, CString, OsStr},
    fs::{self, File, Metadata, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicI32, Ordering},
        Arc,
    },
};

#[cfg(target_arch = "x86_64")]
pub const NOFOLLOW: c_int = 0o400000;
#[cfg(target_arch = "aarch64")]
pub const NOFOLLOW: c_int = 1 << 15;
#[cfg(target_arch = "x86_64")]
const DIRECTORY: c_int = 0o200000;
#[cfg(target_arch = "aarch64")]
const DIRECTORY: c_int = 1 << 14;
pub const NONBLOCK: c_int = 0o4000;

static SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" {
    fn signal(number: c_int, handler: usize) -> usize;
    fn utimensat(fd: c_int, path: *const c_char, times: *const Timespec, flags: c_int) -> c_int;
    fn fstatvfs(fd: c_int, stat: *mut Statvfs) -> c_int;
}

#[repr(C)]
#[derive(Default)]
struct Statvfs {
    fields: [u64; 11],
    spare: [c_int; 6],
}

#[repr(C)]
struct Timespec {
    seconds: c_long,
    nanos: c_long,
}

extern "C" fn on_signal(number: c_int) {
    SIGNAL.store(number, Ordering::Relaxed);
}

pub fn install_signals() -> io::Result<()> {
    for number in [2, 15] {
        // SAFETY: handler performs only a lock-free atomic store and has the C ABI.
        if unsafe { signal(number, on_signal as *const () as usize) } == usize::MAX {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

pub fn received_signal() -> i32 {
    SIGNAL.load(Ordering::Relaxed)
}

pub fn check_cancelled() -> io::Result<()> {
    if received_signal() == 0 {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Interrupted, "copy cancelled"))
    }
}

pub fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

pub fn optional_metadata(path: &Path) -> io::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

pub fn validate_destination(source: &Metadata, destination: Option<&Metadata>) -> io::Result<()> {
    if let Some(destination) = destination {
        if same_file(source, destination) {
            return Err(invalid("source and destination are the same file"));
        }
        if !destination.is_file() && !destination.is_dir() && !destination.file_type().is_symlink()
        {
            return Err(invalid("refusing to replace a special destination file"));
        }
        if source.is_dir() != destination.is_dir() {
            return Err(invalid(
                "cannot replace a directory with a file or link, or vice versa",
            ));
        }
    }
    Ok(())
}

pub fn unchanged(a: &Metadata, b: &Metadata) -> bool {
    same_file(a, b)
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

#[derive(Clone)]
pub struct Directory(Arc<File>);

impl Directory {
    pub fn as_file(&self) -> &File {
        &self.0
    }

    pub fn available_bytes(&self) -> Option<u64> {
        let mut stat = Statvfs::default();
        // SAFETY: live fd and Linux 64-bit statvfs layout with writable storage.
        if unsafe { fstatvfs(self.0.as_raw_fd(), &mut stat) } != 0 {
            return None;
        }
        stat.fields[4].checked_mul(stat.fields[1])
    }
    pub fn open(path: &Path) -> io::Result<Self> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut directory = Self(Arc::new(File::open("/")?));
        for component in absolute.components() {
            match component {
                Component::Normal(name) => directory = directory.child(name)?,
                Component::ParentDir => directory = directory.child(OsStr::new(".."))?,
                Component::RootDir | Component::CurDir => {}
                Component::Prefix(_) => return Err(invalid("unsupported path prefix")),
            }
        }
        Ok(directory)
    }

    pub fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.0.as_raw_fd()))
    }

    pub fn real_path(&self) -> io::Result<PathBuf> {
        fs::read_link(self.path())
    }

    pub fn child(&self, name: &OsStr) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(DIRECTORY | NOFOLLOW)
            .open(self.path().join(name))?;
        Ok(Self(Arc::new(file)))
    }

    pub fn ensure_child(&self, name: &OsStr) -> io::Result<Self> {
        let path = self.path().join(name);
        match self.child(name) {
            Ok(directory) => Ok(directory),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                use std::os::unix::fs::DirBuilderExt;
                match fs::DirBuilder::new().mode(0o700).create(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                self.sync()?;
                self.child(name)
            }
            Err(error) => Err(error),
        }
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        self.0.metadata()
    }
    pub fn has_ancestor(&self, metadata: &Metadata) -> io::Result<bool> {
        let mut directory = self.clone();
        loop {
            let current = directory.metadata()?;
            if same_file(&current, metadata) {
                return Ok(true);
            }
            let parent = directory.child(OsStr::new(".."))?;
            if same_file(&current, &parent.metadata()?) {
                return Ok(false);
            }
            directory = parent;
        }
    }
    pub fn sync(&self) -> io::Result<()> {
        self.0.sync_all()
    }
    pub fn link_mtime(&self, name: &OsStr, metadata: &Metadata) -> io::Result<()> {
        let name = CString::new(name.as_bytes()).map_err(|_| invalid("NUL in filename"))?;
        let times = [
            Timespec {
                seconds: 0,
                nanos: (1 << 30) - 2,
            }, // UTIME_OMIT: retain atime.
            Timespec {
                seconds: metadata.mtime(),
                nanos: metadata.mtime_nsec(),
            },
        ];
        // SAFETY: live fd, terminated filename, two initialized Linux timespecs.
        let result = unsafe { utimensat(self.0.as_raw_fd(), name.as_ptr(), times.as_ptr(), 0x100) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_ancestry_uses_identity_and_includes_self() {
        let root = Directory::open(Path::new("/")).unwrap();
        let proc = Directory::open(Path::new("/proc")).unwrap();
        assert!(proc.has_ancestor(&proc.metadata().unwrap()).unwrap());
        assert!(proc.has_ancestor(&root.metadata().unwrap()).unwrap());
        assert!(!root.has_ancestor(&proc.metadata().unwrap()).unwrap());
    }
}
