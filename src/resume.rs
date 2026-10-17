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

fn checked_partial(file: &File, source: &Metadata) -> io::Result<()> {
    let metadata = file.metadata()?;
    // SAFETY: geteuid takes no arguments and has no side effects.
    let uid = unsafe { geteuid() };
    let preserved_owner =
        uid == 0 && metadata.uid() == source.uid() && metadata.len() == source.len();
    if !metadata.is_file() || metadata.nlink() != 1 || (metadata.uid() != uid && !preserved_owner) {
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

#[inline]
fn fault(_point: &str) {
    #[cfg(test)]
    if std::env::var("TCOPY_TEST_CRASH").as_deref() == Ok(_point) {
        std::process::exit(77);
    }
}

impl Partial {
    pub fn open(
        parent: &Directory,
        name: &OsStr,
        input: &File,
        source: &Metadata,
    ) -> io::Result<(Option<Self>, u64)> {
        if name == OsStr::new(".tcopy-resume") {
            return Err(fsutil::invalid(
                "destination name is reserved for resume checkpoints",
            ));
        }
        let directory = parent.ensure_child(OsStr::new(".tcopy-resume"), true)?;
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
        // Lock the sidecar before creating or recovering the partial, including
        // the interval where publication has removed the partial's name.
        let mut state = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(fsutil::NOFOLLOW | fsutil::NONBLOCK)
            .open(&state_path)?;
        checked(&state)?;
        // SAFETY: live fd; LOCK_EX | LOCK_NB has no pointer arguments.
        if unsafe { flock(state.as_raw_fd(), 2 | 4) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let expected = state_bytes(name, source);
        let mut found = Vec::new();
        Read::by_ref(&mut state)
            .take(expected.len() as u64 + 1)
            .read_to_end(&mut found)?;
        let existing = fsutil::optional_metadata(&path)?;
        if found != expected {
            // Only an empty partial (or no partial) can precede state initialization.
            if !expected.starts_with(&found) || existing.as_ref().is_some_and(|m| m.len() != 0) {
                return Err(fsutil::invalid(
                    "source or destination changed since resume checkpoint",
                ));
            }
            use std::io::{Seek, SeekFrom};
            state.seek(SeekFrom::Start(0))?;
            fault("before_state_write");
            state.write_all(&expected)?;
            fault("after_state_write");
            fault("before_state_sync");
            state.sync_all()?;
            fault("after_state_sync");
            fault("before_initial_directory_sync");
            directory.sync()?;
            fault("after_initial_directory_sync");
        } else if existing.is_none() {
            let destination = parent.path().join(name);
            match OpenOptions::new()
                .read(true)
                .custom_flags(fsutil::NOFOLLOW | fsutil::NONBLOCK)
                .open(&destination)
            {
                Ok(file) => {
                    let before = file.metadata()?;
                    if before.is_file()
                        && before.len() == source.len()
                        && before.mode() & 0o7777 == source.mode() & 0o7777
                        && before.mtime() == source.mtime()
                        && before.mtime_nsec() == source.mtime_nsec()
                        && match compare_prefix(input, &file, source.len()) {
                            Ok(()) => true,
                            Err(error) if error.kind() == io::ErrorKind::InvalidInput => false,
                            Err(error) => return Err(error),
                        }
                    {
                        if !fsutil::unchanged(source, &input.metadata()?)
                            || !fsutil::unchanged(&before, &file.metadata()?)
                            || !fsutil::same_file(&before, &fs::symlink_metadata(&destination)?)
                        {
                            return Err(fsutil::invalid(
                                "file changed during publication recovery",
                            ));
                        }
                        fault("before_recovery_sync");
                        file.sync_all()?;
                        parent.sync()?;
                        directory.sync()?;
                        if !fsutil::same_file(
                            &state.metadata()?,
                            &fs::symlink_metadata(&state_path)?,
                        ) {
                            return Err(fsutil::invalid("resume checkpoint was replaced"));
                        }
                        fault("before_recovery_cleanup");
                        fs::remove_file(&state_path)?;
                        fault("after_recovery_cleanup");
                        directory.sync()?;
                        return Ok((None, source.len()));
                    }
                }
                Err(error)
                    if matches!(error.kind(), io::ErrorKind::NotFound)
                        || error.raw_os_error() == Some(40) => {}
                Err(error) => return Err(error),
            }
        }
        // Re-sync even when a prior process died after write but before fsync.
        state.sync_all()?;
        directory.sync()?;
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let file = match OpenOptions::new()
                    .read(true)
                    .custom_flags(fsutil::NOFOLLOW | fsutil::NONBLOCK)
                    .open(&path)
                {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                        // O_PATH lets the owner restore access after metadata set mode 000.
                        let handle = OpenOptions::new()
                            .read(true)
                            .custom_flags(0o10000000 | fsutil::NOFOLLOW)
                            .open(&path)?;
                        let metadata = handle.metadata()?;
                        if !metadata.is_file()
                            || metadata.nlink() != 1
                            || metadata.uid() != uid
                            || metadata.len() != source.len()
                        {
                            return Err(fsutil::invalid("unsafe resume checkpoint file"));
                        }
                        use std::os::unix::fs::PermissionsExt;
                        let anchored =
                            PathBuf::from(format!("/proc/self/fd/{}", handle.as_raw_fd()));
                        fs::set_permissions(&anchored, fs::Permissions::from_mode(0o600))?;
                        File::open(anchored)?
                    }
                    Err(error) => return Err(error),
                };
                let metadata = file.metadata()?;
                if metadata.mode() & 0o777 != 0o600 {
                    // Metadata preservation can precede a crash before rename.
                    if !metadata.is_file()
                        || metadata.nlink() != 1
                        || (metadata.uid() != uid && !(uid == 0 && metadata.uid() == source.uid()))
                        || metadata.len() != source.len()
                    {
                        return Err(fsutil::invalid("unsafe resume checkpoint file"));
                    }
                    compare_prefix(input, &file, source.len())?;
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(fs::Permissions::from_mode(0o600))?;
                }
                checked_partial(&file, source)?;
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(fsutil::NOFOLLOW | fsutil::NONBLOCK)
                    .open(&path)?
            }
            Err(error) => return Err(error),
        };
        fault("after_part_create");
        checked_partial(&file, source)?;
        // SAFETY: live fd; LOCK_EX | LOCK_NB has no pointer arguments.
        if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
            return Err(io::Error::last_os_error());
        }
        directory.sync()?;
        let length = file.metadata()?.len();
        if length > source.len() {
            return Err(fsutil::invalid("resume checkpoint exceeds source length"));
        }
        compare_prefix(input, &file, length)?;
        if !fsutil::unchanged(source, &input.metadata()?) {
            return Err(fsutil::invalid("source changed during resume validation"));
        }
        Ok((
            Some(Self {
                file,
                path,
                state_path,
                state,
                directory,
                parent: parent.clone(),
                discard: false,
                published: false,
            }),
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
        fault("before_rename");
        fs::rename(&self.path, destination)?;
        fault("after_rename");
        self.published = true;
        fault("before_parent_sync");
        self.parent.sync()?;
        fault("after_parent_sync");
        fault("before_publication_directory_sync");
        self.directory.sync()?;
        fault("after_publication_directory_sync");
        fault("before_cleanup");
        fs::remove_file(&self.state_path)?;
        fault("after_cleanup");
        fault("before_cleanup_sync");
        self.directory.sync()?;
        fault("after_cleanup_sync");
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_after_publication_before_cleanup() {
        let root =
            std::env::temp_dir().join(format!("tcopy-resume-publication-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("source"), b"verified data").unwrap();
        let parent = Directory::open(&root).unwrap();
        let input = File::open(root.join("source")).unwrap();
        let source = input.metadata().unwrap();
        let (partial, _) = Partial::open(&parent, OsStr::new("dest"), &input, &source).unwrap();
        let mut partial = partial.unwrap();
        partial.file.write_all(b"verified data").unwrap();
        crate::metadata::preserve_file(&input, &partial.file, &source, false, true).unwrap();
        // Deterministically stop between rename and checkpoint cleanup.
        fs::rename(&partial.path, root.join("dest")).unwrap();
        drop(partial);
        let result = Partial::open(&parent, OsStr::new("dest"), &input, &source);
        assert!(result.is_ok(), "restart blocked: {:?}", result.err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn crash_child() {
        let Ok(root) = std::env::var("TCOPY_TEST_ROOT") else {
            return;
        };
        let root = Path::new(&root);
        let parent = Directory::open(root).unwrap();
        let input = File::open(root.join("source")).unwrap();
        let source = input.metadata().unwrap();
        let (partial, offset) =
            Partial::open(&parent, OsStr::new("dest"), &input, &source).unwrap();
        if let Some(mut partial) = partial {
            use std::io::{Seek, SeekFrom};
            partial.file.seek(SeekFrom::Start(offset)).unwrap();
            fault("before_data_write");
            let mut bytes = vec![0; (source.len() - offset) as usize];
            input.read_exact_at(&mut bytes, offset).unwrap();
            let first = bytes.len().min(128 * 1024);
            partial.file.write_all(&bytes[..first]).unwrap();
            fault("during_data_write");
            partial.file.write_all(&bytes[first..]).unwrap();
            fault("after_data_write");
            fault("before_metadata");
            crate::metadata::preserve_file(&input, &partial.file, &source, false, false).unwrap();
            fault("after_metadata");
            fault("before_data_sync");
            partial.file.sync_all().unwrap();
            fault("after_data_sync");
            partial.publish(&root.join("dest")).unwrap();
        }
    }

    fn child(root: &Path, point: &str) -> std::process::ExitStatus {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "resume::tests::crash_child"])
            .env("TCOPY_TEST_ROOT", root)
            .env("TCOPY_TEST_CRASH", point)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
    }

    #[test]
    fn crashes_at_lifecycle_boundaries_recover_without_recopying_verified_bytes() {
        let points = [
            "before_state_write",
            "after_state_write",
            "before_state_sync",
            "after_state_sync",
            "before_initial_directory_sync",
            "after_initial_directory_sync",
            "after_part_create",
            "before_data_write",
            "after_data_write",
            "during_data_write",
            "before_metadata",
            "after_metadata",
            "before_data_sync",
            "after_data_sync",
            "before_rename",
            "after_rename",
            "before_parent_sync",
            "after_parent_sync",
            "before_cleanup",
            "before_publication_directory_sync",
            "after_publication_directory_sync",
            "after_cleanup",
            "before_cleanup_sync",
            "after_cleanup_sync",
        ];
        for point in points {
            let root =
                std::env::temp_dir().join(format!("tcopy-crash-{}-{point}", std::process::id()));
            fs::create_dir(&root).unwrap();
            let bytes = vec![42; 512 * 1024 + 7];
            fs::write(root.join("source"), &bytes).unwrap();
            fs::write(root.join("dest"), b"unrelated old destination").unwrap();
            assert_eq!(child(&root, point).code(), Some(77), "{point}");
            if point == "during_data_write" {
                let parent = Directory::open(&root).unwrap();
                let input = File::open(root.join("source")).unwrap();
                let (partial, offset) = Partial::open(
                    &parent,
                    OsStr::new("dest"),
                    &input,
                    &input.metadata().unwrap(),
                )
                .unwrap();
                assert_eq!(offset, 128 * 1024);
                drop(partial);
                assert_eq!(
                    fs::read(root.join("dest")).unwrap(),
                    b"unrelated old destination"
                );
            }
            // Repeated recovery interruptions must retain both checkpoint and data.
            if matches!(
                point,
                "after_rename" | "before_parent_sync" | "after_parent_sync" | "before_cleanup"
            ) {
                let inode = fs::metadata(root.join("dest")).unwrap().ino();
                for recovery in [
                    "before_recovery_sync",
                    "before_recovery_cleanup",
                    "before_recovery_cleanup",
                ] {
                    assert_eq!(child(&root, recovery).code(), Some(77));
                    assert_eq!(fs::metadata(root.join("dest")).unwrap().ino(), inode);
                }
                assert!(child(&root, "").success());
                assert_eq!(fs::metadata(root.join("dest")).unwrap().ino(), inode);
            } else {
                let retained = fs::read_dir(root.join(".tcopy-resume"))
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .find(|p| p.extension() == Some(OsStr::new("part")))
                    .map(|p| fs::metadata(p).unwrap().ino());
                assert!(child(&root, "").success(), "{point}");
                if let Some(inode) = retained {
                    assert_eq!(fs::metadata(root.join("dest")).unwrap().ino(), inode);
                }
            }
            assert_eq!(fs::read(root.join("dest")).unwrap(), bytes, "{point}");
            assert_eq!(fs::read_dir(root.join(".tcopy-resume")).unwrap().count(), 0);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn orphan_state_does_not_accept_wrong_same_size_destination_or_follow_symlinks() {
        for link in [false, true] {
            let root =
                std::env::temp_dir().join(format!("tcopy-orphan-{}-{link}", std::process::id()));
            fs::create_dir(&root).unwrap();
            let bytes = vec![42; 1024];
            fs::write(root.join("source"), &bytes).unwrap();
            assert_eq!(child(&root, "after_rename").code(), Some(77));
            fs::remove_file(root.join("dest")).unwrap();
            fs::write(root.join("unrelated"), vec![99; 1024]).unwrap();
            if link {
                std::os::unix::fs::symlink(root.join("unrelated"), root.join("dest")).unwrap();
            } else {
                fs::write(root.join("dest"), vec![99; 1024]).unwrap();
            }
            assert_eq!(child(&root, "before_data_write").code(), Some(77));
            assert_eq!(fs::read(root.join("dest")).unwrap(), vec![99; 1024]);
            assert!(child(&root, "").success());
            assert_eq!(fs::read(root.join("dest")).unwrap(), bytes);
            assert_eq!(fs::read(root.join("unrelated")).unwrap(), vec![99; 1024]);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn torn_initial_state_and_legacy_empty_orphan_can_restart() {
        for cut in [0, 1, 17, 40] {
            let root =
                std::env::temp_dir().join(format!("tcopy-torn-{}-{cut}", std::process::id()));
            fs::create_dir(&root).unwrap();
            fs::write(root.join("source"), b"data").unwrap();
            fs::write(root.join("dest"), b"keep").unwrap();
            assert_eq!(child(&root, "after_part_create").code(), Some(77));
            let state = fs::read_dir(root.join(".tcopy-resume"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .find(|p| p.extension() == Some(OsStr::new("state")))
                .unwrap();
            OpenOptions::new()
                .write(true)
                .open(&state)
                .unwrap()
                .set_len(cut)
                .unwrap();
            assert_eq!(child(&root, "before_data_write").code(), Some(77));
            assert_eq!(fs::read(root.join("dest")).unwrap(), b"keep");
            assert!(child(&root, "").success());
            assert_eq!(fs::read(root.join("dest")).unwrap(), b"data");
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn verified_read_only_partial_is_reused_after_metadata_crash() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("tcopy-readonly-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("source"), b"data").unwrap();
        fs::set_permissions(root.join("source"), fs::Permissions::from_mode(0o444)).unwrap();
        assert_eq!(child(&root, "after_metadata").code(), Some(77));
        let part = fs::read_dir(root.join(".tcopy-resume"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension() == Some(OsStr::new("part")))
            .unwrap();
        // Simulate interrupted ACL/mode application removing read permission.
        fs::set_permissions(&part, fs::Permissions::from_mode(0o0)).unwrap();
        let inode = fs::metadata(&part).unwrap().ino();
        assert!(child(&root, "").success());
        assert_eq!(fs::metadata(root.join("dest")).unwrap().ino(), inode);
        assert_eq!(
            fs::metadata(root.join("dest")).unwrap().mode() & 0o777,
            0o444
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nonempty_orphan_partial_without_state_is_left_untouched() {
        let root = std::env::temp_dir().join(format!("tcopy-orphan-part-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("source"), b"new source").unwrap();
        fs::write(root.join("dest"), b"keep destination").unwrap();
        assert_eq!(child(&root, "after_part_create").code(), Some(77));
        let directory = root.join(".tcopy-resume");
        let part = directory.join(format!("{}.part", key(OsStr::new("dest"))));
        let state = directory.join(format!("{}.state", key(OsStr::new("dest"))));
        fs::write(&part, b"unrelated retained bytes").unwrap();
        fs::remove_file(state).unwrap();
        assert!(!child(&root, "").success());
        assert_eq!(fs::read(part).unwrap(), b"unrelated retained bytes");
        assert_eq!(fs::read(root.join("dest")).unwrap(), b"keep destination");
        fs::remove_dir_all(root).unwrap();
    }
}
