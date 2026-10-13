use std::{
    ffi::{c_char, c_int, c_void, CString},
    fs, io,
    os::unix::{
        ffi::OsStrExt,
        fs::{symlink, MetadataExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tcopy-phase4-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn copy(&self, source: &str, destination: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tcopy"))
            .current_dir(&self.0)
            .args([source, destination, "--preserve-extended"])
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
                for entry in fs::read_dir(path).unwrap().flatten() {
                    writable(&entry.path());
                }
            }
        }
        writable(&self.0);
        fs::remove_dir_all(&self.0).unwrap();
    }
}

extern "C" {
    fn setxattr(
        path: *const c_char,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        flags: c_int,
    ) -> c_int;
    fn getxattr(path: *const c_char, name: *const c_char, value: *mut c_void, size: usize)
        -> isize;
    fn lsetxattr(
        path: *const c_char,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        flags: c_int,
    ) -> c_int;
    fn lgetxattr(
        path: *const c_char,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
    ) -> isize;
    fn geteuid() -> u32;
    fn setgid(gid: u32) -> c_int;
    fn setuid(uid: u32) -> c_int;
}

fn cpath(path: &Path) -> CString {
    CString::new(path.as_os_str().as_bytes()).unwrap()
}

fn set_attr(path: &Path, name: &str, data: &[u8], link: bool) -> io::Result<()> {
    let path = cpath(path);
    let name = CString::new(name).unwrap();
    let result = unsafe {
        if link {
            lsetxattr(
                path.as_ptr(),
                name.as_ptr(),
                data.as_ptr().cast(),
                data.len(),
                0,
            )
        } else {
            setxattr(
                path.as_ptr(),
                name.as_ptr(),
                data.as_ptr().cast(),
                data.len(),
                0,
            )
        }
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn attr(path: &Path, name: &str, link: bool) -> io::Result<Vec<u8>> {
    let path = cpath(path);
    let name = CString::new(name).unwrap();
    let get = |buffer: *mut c_void, size| unsafe {
        if link {
            lgetxattr(path.as_ptr(), name.as_ptr(), buffer, size)
        } else {
            getxattr(path.as_ptr(), name.as_ptr(), buffer, size)
        }
    };
    let size = get(std::ptr::null_mut(), 0);
    if size < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut value = vec![0; size as usize];
    let count = get(value.as_mut_ptr().cast(), value.len());
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    value.truncate(count as usize);
    Ok(value)
}

fn supported_setup(result: io::Result<()>, link: bool) -> bool {
    match result {
        Ok(()) => true,
        Err(error)
            if error.raw_os_error() == Some(95) || link && error.raw_os_error() == Some(1) =>
        {
            false
        }
        Err(error) => panic!("source xattr setup failed: {error}"),
    }
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn preserves_file_and_directory_ownership_and_xattrs_and_removes_stale_directory_xattr() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::write(f.path("source/file"), b"data").unwrap();
    fs::create_dir(f.path("dest")).unwrap();
    fs::create_dir(f.path("dest/source")).unwrap();
    if !supported_setup(
        set_attr(
            &f.path("source/file"),
            "user.tcopy_file",
            b"file-value",
            false,
        ),
        false,
    ) || !supported_setup(
        set_attr(&f.path("source"), "user.tcopy_dir", b"dir-value", false),
        false,
    ) || !supported_setup(
        set_attr(&f.path("dest/source"), "user.tcopy_stale", b"stale", false),
        false,
    ) {
        return;
    }

    assert_success(f.copy("source", "dest"));
    assert_eq!(
        attr(&f.path("dest/source/file"), "user.tcopy_file", false).unwrap(),
        b"file-value"
    );
    assert_eq!(
        attr(&f.path("dest/source"), "user.tcopy_dir", false).unwrap(),
        b"dir-value"
    );
    assert_eq!(
        attr(&f.path("dest/source"), "user.tcopy_stale", false)
            .unwrap_err()
            .raw_os_error(),
        Some(61)
    );
    for (source, dest) in [
        ("source", "dest/source"),
        ("source/file", "dest/source/file"),
    ] {
        let source = fs::metadata(f.path(source)).unwrap();
        let dest = fs::metadata(f.path(dest)).unwrap();
        assert_eq!((dest.uid(), dest.gid()), (source.uid(), source.gid()));
    }
}

#[test]
fn preserves_symlink_xattr_when_filesystem_allows_it() {
    let f = Fixture::new();
    fs::write(f.path("target"), b"data").unwrap();
    symlink("target", f.path("source")).unwrap();
    if !supported_setup(
        set_attr(&f.path("source"), "user.tcopy_link", b"link-value", true),
        true,
    ) {
        return;
    }
    assert_success(f.copy("source", "dest"));
    assert_eq!(fs::read_link(f.path("dest")).unwrap(), Path::new("target"));
    assert_eq!(
        attr(&f.path("dest"), "user.tcopy_link", true).unwrap(),
        b"link-value"
    );
    let source = fs::symlink_metadata(f.path("source")).unwrap();
    let dest = fs::symlink_metadata(f.path("dest")).unwrap();
    assert_eq!((dest.uid(), dest.gid()), (source.uid(), source.gid()));
}

#[test]
fn preserves_acl_xattr_when_acl_tools_and_filesystem_allow_it() {
    let f = Fixture::new();
    fs::write(f.path("source"), b"data").unwrap();
    let setup = match Command::new("setfacl")
        .args(["-m", "u:65534:r--"])
        .arg(f.path("source"))
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => panic!("setfacl failed to run: {error}"),
    };
    if !setup.status.success() {
        if ["Operation not supported", "Invalid argument"]
            .iter()
            .any(|message| String::from_utf8_lossy(&setup.stderr).contains(message))
        {
            return;
        }
        panic!(
            "setfacl setup failed: {}",
            String::from_utf8_lossy(&setup.stderr)
        );
    }
    assert_success(f.copy("source", "dest"));
    assert_eq!(
        attr(&f.path("dest"), "system.posix_acl_access", false).unwrap(),
        attr(&f.path("source"), "system.posix_acl_access", false).unwrap()
    );
    if let Ok(source_acl) = Command::new("getfacl")
        .arg("-cp")
        .arg(f.path("source"))
        .output()
    {
        let dest_acl = Command::new("getfacl")
            .arg("-cp")
            .arg(f.path("dest"))
            .output()
            .unwrap();
        assert!(source_acl.status.success() && dest_acl.status.success());
        assert_eq!(source_acl.stdout, dest_acl.stdout);
    }
    fs::create_dir(f.path("source-directory")).unwrap();
    let setup = Command::new("setfacl")
        .args(["-m", "u:65534:r-x,d:u:65534:r-x"])
        .arg(f.path("source-directory"))
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    fs::write(f.path("source-directory/child"), b"inherited ACL").unwrap();
    assert_success(f.copy("source-directory", "dest-directory"));
    for name in ["system.posix_acl_access", "system.posix_acl_default"] {
        assert_eq!(
            attr(&f.path("dest-directory"), name, false).unwrap(),
            attr(&f.path("source-directory"), name, false).unwrap()
        );
    }
    assert_eq!(
        attr(
            &f.path("dest-directory/child"),
            "system.posix_acl_access",
            false
        )
        .unwrap(),
        attr(
            &f.path("source-directory/child"),
            "system.posix_acl_access",
            false
        )
        .unwrap()
    );
}

#[test]
fn requested_owner_preservation_failure_does_not_publish_file() {
    if unsafe { geteuid() } != 0 {
        return;
    }
    let f = Fixture::new();
    fs::write(f.path("source"), b"data").unwrap();
    fs::set_permissions(&f.0, fs::Permissions::from_mode(0o777)).unwrap();
    fs::set_permissions(f.path("source"), fs::Permissions::from_mode(0o644)).unwrap();
    let output = unsafe {
        Command::new(env!("CARGO_BIN_EXE_tcopy"))
            .current_dir(&f.0)
            .args(["source", "dest", "--preserve-extended"])
            .pre_exec(|| {
                if setgid(65534) != 0 || setuid(65534) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            })
            .output()
            .unwrap()
    };
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Operation not permitted"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!f.path("dest").exists());
}
