use crate::fsutil::{self, Directory};
use std::{
    ffi::{c_char, c_int, c_void, CString, OsStr},
    fs::{File, FileTimes, Metadata},
    io,
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
};

extern "C" {
    fn fchown(fd: c_int, uid: u32, gid: u32) -> c_int;
    fn fchownat(fd: c_int, path: *const c_char, uid: u32, gid: u32, flags: c_int) -> c_int;
    fn flistxattr(fd: c_int, list: *mut c_char, size: usize) -> isize;
    fn fgetxattr(fd: c_int, name: *const c_char, value: *mut c_void, size: usize) -> isize;
    fn fsetxattr(
        fd: c_int,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        flags: c_int,
    ) -> c_int;
    fn fremovexattr(fd: c_int, name: *const c_char) -> c_int;
    fn llistxattr(path: *const c_char, list: *mut c_char, size: usize) -> isize;
    fn lgetxattr(
        path: *const c_char,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
    ) -> isize;
    fn lsetxattr(
        path: *const c_char,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        flags: c_int,
    ) -> c_int;
    fn lremovexattr(path: *const c_char, name: *const c_char) -> c_int;
}

fn syscall(result: c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn names(mut list: impl FnMut(*mut c_char, usize) -> isize) -> io::Result<Vec<CString>> {
    for _ in 0..4 {
        fsutil::check_cancelled()?;
        let size = list(std::ptr::null_mut(), 0);
        if size < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0; size as usize];
        let count = list(buffer.as_mut_ptr().cast(), buffer.len());
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(34) {
                continue;
            } // ERANGE: attributes changed during listing.
            return Err(error);
        }
        if count as usize > buffer.len() {
            continue;
        }
        buffer.truncate(count as usize);
        return buffer
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
            .map(|name| CString::new(name).map_err(|_| fsutil::invalid("invalid xattr name")))
            .collect();
    }
    Err(io::Error::other("xattrs changed repeatedly"))
}

fn value(mut get: impl FnMut(*mut c_void, usize) -> isize) -> io::Result<Vec<u8>> {
    for _ in 0..4 {
        fsutil::check_cancelled()?;
        let size = get(std::ptr::null_mut(), 0);
        if size < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0; size as usize];
        let count = get(buffer.as_mut_ptr().cast(), buffer.len());
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(34) {
                continue;
            }
            return Err(error);
        }
        if count as usize > buffer.len() {
            continue;
        }
        buffer.truncate(count as usize);
        return Ok(buffer);
    }
    Err(io::Error::other("xattr changed repeatedly"))
}

fn copy_fd_xattrs(source: &File, dest: &File) -> io::Result<()> {
    let source_fd = source.as_raw_fd();
    let dest_fd = dest.as_raw_fd();
    // SAFETY: both descriptors stay open; buffers have the sizes supplied to the calls.
    let source_names = names(|ptr, len| unsafe { flistxattr(source_fd, ptr, len) })?;
    for name in names(|ptr, len| unsafe { flistxattr(dest_fd, ptr, len) })? {
        fsutil::check_cancelled()?;
        if !source_names.contains(&name) {
            syscall(unsafe { fremovexattr(dest_fd, name.as_ptr()) })?;
        }
    }
    for name in source_names {
        fsutil::check_cancelled()?;
        let data = value(|ptr, len| unsafe { fgetxattr(source_fd, name.as_ptr(), ptr, len) })?;
        syscall(unsafe { fsetxattr(dest_fd, name.as_ptr(), data.as_ptr().cast(), data.len(), 0) })?;
    }
    Ok(())
}

pub fn basic_matches(source: &Metadata, dest: &Metadata) -> bool {
    source.mode() & 0o7777 == dest.mode() & 0o7777
        && source.mtime() == dest.mtime()
        && source.mtime_nsec() == dest.mtime_nsec()
}

pub fn file_matches(
    source: &File,
    dest: &File,
    metadata: &Metadata,
    extended: bool,
) -> io::Result<bool> {
    let current = dest.metadata()?;
    if !basic_matches(metadata, &current) {
        return Ok(false);
    }
    if !extended {
        return Ok(true);
    }
    if (metadata.uid(), metadata.gid()) != (current.uid(), current.gid()) {
        return Ok(false);
    }
    // SAFETY: live descriptors and buffers sized by the checked xattr helpers.
    let source_names = names(|ptr, len| unsafe { flistxattr(source.as_raw_fd(), ptr, len) })?;
    let dest_names = names(|ptr, len| unsafe { flistxattr(dest.as_raw_fd(), ptr, len) })?;
    if source_names.len() != dest_names.len() {
        return Ok(false);
    }
    for name in source_names {
        fsutil::check_cancelled()?;
        if !dest_names.contains(&name) {
            return Ok(false);
        }
        let left =
            value(|ptr, len| unsafe { fgetxattr(source.as_raw_fd(), name.as_ptr(), ptr, len) })?;
        let right =
            value(|ptr, len| unsafe { fgetxattr(dest.as_raw_fd(), name.as_ptr(), ptr, len) })?;
        if left != right {
            return Ok(false);
        }
    }
    Ok(true)
}

fn preserve_fd(
    source: &File,
    dest: &File,
    metadata: &Metadata,
    extended: bool,
    fsync: bool,
) -> io::Result<()> {
    if extended {
        let current = dest.metadata()?;
        if (metadata.uid(), metadata.gid()) != (current.uid(), current.gid()) {
            // SAFETY: live destination descriptor and ownership from source metadata.
            syscall(unsafe { fchown(dest.as_raw_fd(), metadata.uid(), metadata.gid()) })?;
        }
    }
    dest.set_permissions(metadata.permissions())?;
    if extended {
        copy_fd_xattrs(source, dest)?;
    }
    dest.set_times(FileTimes::new().set_modified(metadata.modified()?))?;
    if fsync {
        dest.sync_all()?;
    }
    Ok(())
}

pub fn preserve_file(
    source: &File,
    dest: &File,
    metadata: &Metadata,
    extended: bool,
    fsync: bool,
) -> io::Result<()> {
    preserve_fd(source, dest, metadata, extended, fsync)
}

pub fn preserve_directory(
    source: &Directory,
    dest: &Directory,
    metadata: &Metadata,
    extended: bool,
    fsync: bool,
) -> io::Result<()> {
    preserve_fd(source.as_file(), dest.as_file(), metadata, extended, fsync)
}

pub fn preserve_link(
    source_parent: &Directory,
    source_name: &OsStr,
    dest_parent: &Directory,
    dest_name: &OsStr,
    metadata: &Metadata,
    extended: bool,
) -> io::Result<()> {
    if extended {
        let name =
            CString::new(dest_name.as_bytes()).map_err(|_| fsutil::invalid("NUL in filename"))?;
        let dest_metadata = std::fs::symlink_metadata(dest_parent.path().join(dest_name))?;
        if (metadata.uid(), metadata.gid()) != (dest_metadata.uid(), dest_metadata.gid()) {
            // SAFETY: live parent descriptor, terminated child name, and AT_SYMLINK_NOFOLLOW.
            syscall(unsafe {
                fchownat(
                    dest_parent.as_file().as_raw_fd(),
                    name.as_ptr(),
                    metadata.uid(),
                    metadata.gid(),
                    0x100,
                )
            })?;
        }
        let source = CString::new(
            source_parent
                .path()
                .join(source_name)
                .as_os_str()
                .as_bytes(),
        )
        .map_err(|_| fsutil::invalid("NUL in filename"))?;
        let dest = CString::new(dest_parent.path().join(dest_name).as_os_str().as_bytes())
            .map_err(|_| fsutil::invalid("NUL in filename"))?;
        // SAFETY: both paths and names are terminated, and all buffers are sized by the calls.
        let source_names = names(|ptr, len| unsafe { llistxattr(source.as_ptr(), ptr, len) })?;
        for name in names(|ptr, len| unsafe { llistxattr(dest.as_ptr(), ptr, len) })? {
            fsutil::check_cancelled()?;
            if !source_names.contains(&name) {
                syscall(unsafe { lremovexattr(dest.as_ptr(), name.as_ptr()) })?;
            }
        }
        for name in source_names {
            fsutil::check_cancelled()?;
            let data =
                value(|ptr, len| unsafe { lgetxattr(source.as_ptr(), name.as_ptr(), ptr, len) })?;
            syscall(unsafe {
                lsetxattr(
                    dest.as_ptr(),
                    name.as_ptr(),
                    data.as_ptr().cast(),
                    data.len(),
                    0,
                )
            })?;
        }
    }
    dest_parent.link_mtime(dest_name, metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, OpenOptions},
        os::unix::fs::PermissionsExt,
    };

    #[test]
    fn xattr_queries_retry_when_data_appears_after_empty_size_probe() {
        let mut calls = 0;
        let listed = names(|ptr, len| {
            calls += 1;
            match calls {
                1 => 0,
                2 => 2,
                3 => 2,
                _ => {
                    assert_eq!(len, 2);
                    unsafe { std::ptr::copy_nonoverlapping(b"x\0".as_ptr().cast(), ptr, 2) };
                    2
                }
            }
        })
        .unwrap();
        assert_eq!(listed, vec![CString::new("x").unwrap()]);
        let mut calls = 0;
        let bytes = value(|ptr, len| {
            calls += 1;
            match calls {
                1 => 0,
                2 => 2,
                3 => 2,
                _ => {
                    assert_eq!(len, 2);
                    unsafe { std::ptr::copy_nonoverlapping(b"xy".as_ptr().cast(), ptr, 2) };
                    2
                }
            }
        })
        .unwrap();
        assert_eq!(bytes, b"xy");
    }

    #[test]
    fn preserves_file_mode_mtime_and_xattrs() {
        let root = std::env::temp_dir().join(format!(
            "tcopy-metadata-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&root).unwrap();
        let source = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(root.join("source"))
            .unwrap();
        let dest = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(root.join("dest"))
            .unwrap();
        source
            .set_permissions(fs::Permissions::from_mode(0o640))
            .unwrap();
        let name = CString::new("user.tcopy_test").unwrap();
        // SAFETY: live descriptor, terminated attribute name, and one initialized value byte.
        assert_eq!(
            unsafe {
                fsetxattr(
                    source.as_raw_fd(),
                    name.as_ptr(),
                    b"x".as_ptr().cast(),
                    1,
                    0,
                )
            },
            0
        );
        preserve_file(&source, &dest, &source.metadata().unwrap(), true, true).unwrap();
        assert_eq!(dest.metadata().unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(
            dest.metadata().unwrap().modified().unwrap(),
            source.metadata().unwrap().modified().unwrap()
        );
        let data =
            value(|ptr, len| unsafe { fgetxattr(dest.as_raw_fd(), name.as_ptr(), ptr, len) })
                .unwrap();
        assert_eq!(data, b"x");
        drop(source);
        drop(dest);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_metadata_removes_attributes_missing_from_source() {
        let root = std::env::temp_dir().join(format!(
            "tcopy-dir-metadata-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("source")).unwrap();
        fs::create_dir(root.join("dest")).unwrap();
        let source = Directory::open(&root.join("source")).unwrap();
        let dest = Directory::open(&root.join("dest")).unwrap();
        let name = CString::new("user.tcopy_stale").unwrap();
        // SAFETY: live destination descriptor, terminated attribute name, and one initialized value byte.
        assert_eq!(
            unsafe {
                fsetxattr(
                    dest.as_file().as_raw_fd(),
                    name.as_ptr(),
                    b"x".as_ptr().cast(),
                    1,
                    0,
                )
            },
            0
        );
        preserve_directory(&source, &dest, &source.metadata().unwrap(), true, true).unwrap();
        assert_eq!(
            unsafe {
                fgetxattr(
                    dest.as_file().as_raw_fd(),
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                )
            },
            -1
        );
        drop(source);
        drop(dest);
        fs::remove_dir_all(root).unwrap();
    }
}
