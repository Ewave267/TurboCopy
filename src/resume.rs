use crate::fsutil::{self, Directory};
use std::{
    ffi::OsStr,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{FileExt, MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
};

extern "C" {
    fn geteuid() -> u32;
    fn flock(fd: i32, operation: i32) -> i32;
}

fn remove_if_same(path: &Path, file: &File) {
    if fs::symlink_metadata(path).as_ref().is_ok_and(|current| {
        file.metadata()
            .as_ref()
            .is_ok_and(|opened| fsutil::same_file(current, opened))
    }) {
        let _ = fs::remove_file(path);
    }
}

pub struct Partial {
    pub file: File,
    path: PathBuf,
    state_path: PathBuf,
    state: File,
    directory: Directory,
    parent: Directory,
    discard: bool,
    published: bool,
}

fn checked(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    // SAFETY: geteuid takes no arguments and has no side effects.
    let uid = unsafe { geteuid() };
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != uid
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(fsutil::invalid("unsafe resume checkpoint file"));
    }
    Ok(())
}

fn state_bytes(name: &OsStr, source: &Metadata) -> Vec<u8> {
    let mut bytes = b"TCOPY-RESUME-1\0".to_vec();
    let name = name.as_bytes();
    bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
    bytes.extend_from_slice(name);
    for value in [
        source.dev(),
        source.ino(),
        source.len(),
        source.mtime() as u64,
        source.mtime_nsec() as u64,
        source.ctime() as u64,
        source.ctime_nsec() as u64,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn key(name: &OsStr) -> String {
    let mut hash = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d_u128;
    for byte in name.as_bytes() {
        hash ^= *byte as u128;
        hash = hash.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013b);
    }
    format!("{hash:032x}")
}

fn compare_prefix(input: &File, partial: &File, length: u64) -> io::Result<()> {
    let mut source = [0; 128 * 1024];
    let mut retained = [0; 128 * 1024];
    let mut offset = 0;
    while offset < length {
        fsutil::check_cancelled()?;
        let size = (length - offset).min(source.len() as u64) as usize;
        input.read_exact_at(&mut source[..size], offset)?;
        partial.read_exact_at(&mut retained[..size], offset)?;
        if source[..size] != retained[..size] {
            return Err(fsutil::invalid(
                "retained partial differs from source; remove checkpoint to restart",
            ));
        }
        offset += size as u64;
    }
    Ok(())
}

impl Partial {
    pub fn open(
        parent: &Directory,
        name: &OsStr,
        input: &File,
        source: &Metadata,
    ) -> io::Result<(Self, u64)> {
        if name == OsStr::new(".tcopy-resume") {
            return Err(fsutil::invalid(
                "destination name is reserved for resume checkpoints",
            ));
        }
        let directory = parent.ensure_child(OsStr::new(".tcopy-resume"))?;
        let info = directory.metadata()?;
        // SAFETY: geteuid takes no arguments and has no side effects.
        let uid = unsafe { geteuid() };
        if !info.is_dir() || info.uid() != uid || info.mode() & 0o777 != 0o700 {
            return Err(fsutil::invalid(
                "resume directory must be private and owned by current user",
            ));
        }
        let key = key(name);
        let path = directory.path().join(format!("{key}.part"));
        let state_path = directory.path().join(format!("{key}.state"));
        let fresh = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path);
        let (file, created) = match fresh {
            Ok(file) => (file, true),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(fsutil::NOFOLLOW | fsutil::NONBLOCK)
                    .open(&path)?,
                false,
            ),
            Err(error) => return Err(error),
        };
        checked(&file)?;
        // SAFETY: file is a live fd; LOCK_EX | LOCK_NB has no pointer arguments.
        if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let expected = state_bytes(name, source);
        let state = if created {
            let attempt = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&state_path);
            let mut state = match attempt {
                Ok(state) => state,
                Err(error) => {
                    remove_if_same(&path, &file);
                    return Err(error);
                }
            };
            if let Err(error) = state
                .write_all(&expected)
                .and_then(|_| state.sync_all())
                .and_then(|_| directory.sync())
            {
                remove_if_same(&state_path, &state);
                remove_if_same(&path, &file);
                let _ = directory.sync();
                return Err(error);
            }
            state
        } else {
            let mut state = OpenOptions::new()
                .read(true)
                .custom_flags(fsutil::NOFOLLOW | fsutil::NONBLOCK)
                .open(&state_path)?;
            checked(&state)?;
            if state.metadata()?.len() != expected.len() as u64 {
                return Err(fsutil::invalid("invalid or stale resume checkpoint"));
            }
            let mut found = vec![0; expected.len()];
            state.read_exact(&mut found)?;
            if found != expected {
                return Err(fsutil::invalid(
                    "source or destination changed since resume checkpoint",
                ));
            }
            state
        };
        checked(&state)?;
        let length = file.metadata()?.len();
        if length > source.len() {
            return Err(fsutil::invalid("resume checkpoint exceeds source length"));
        }
        compare_prefix(input, &file, length)?;
        if !fsutil::unchanged(source, &input.metadata()?) {
            return Err(fsutil::invalid("source changed during resume validation"));
        }
        Ok((
            Self {
                file,
                path,
                state_path,
                state,
                directory,
                parent: parent.clone(),
                discard: false,
                published: false,
            },
            length,
        ))
    }

    pub fn discard_on_drop(&mut self) {
        self.discard = true;
    }

    pub fn publish(mut self, destination: &Path) -> io::Result<()> {
        let actual = fs::symlink_metadata(&self.path)?;
        let state = fs::symlink_metadata(&self.state_path)?;
        if !fsutil::same_file(&actual, &self.file.metadata()?)
            || !fsutil::same_file(&state, &self.state.metadata()?)
        {
            return Err(fsutil::invalid("resume checkpoint was replaced"));
        }
        fs::rename(&self.path, destination)?;
        self.published = true;
        self.parent.sync()?;
        self.directory.sync()?;
        fs::remove_file(&self.state_path)?;
        self.directory.sync()
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.discard || self.published {
            return;
        }
        remove_if_same(&self.path, &self.file);
        remove_if_same(&self.state_path, &self.state);
        let _ = self.directory.sync();
    }
}
