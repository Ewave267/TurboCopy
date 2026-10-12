use std::{
    ffi::{OsStr, OsString},
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{symlink, MetadataExt, PermissionsExt},
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
            "tcopy-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.0.join(name)
    }
    fn run(&self, args: &[&OsStr]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tcopy"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
    fn copy(&self, args: &[&str]) -> Output {
        self.run(&args.iter().map(OsStr::new).collect::<Vec<_>>())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
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

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn cli_help_version_and_invalid_arguments() {
    let f = Fixture::new();
    assert!(success(f.copy(&["--help"])).contains("Matching sizes"));
    assert!(success(f.copy(&["--version"])).contains("0.1.0"));
    for args in [
        vec![],
        vec!["one"],
        vec!["--workers", "8"],
        vec!["", "dest"],
    ] {
        assert_eq!(f.copy(&args).status.code(), Some(2));
    }
}

#[test]
fn copies_file_and_atomically_overwrites_without_changing_hardlink_alias() {
    let f = Fixture::new();
    fs::write(f.path("source"), b"new contents").unwrap();
    fs::write(f.path("dest"), b"old").unwrap();
    fs::hard_link(f.path("dest"), f.path("old-alias")).unwrap();
    success(f.copy(&["source", "dest"]));
    assert_eq!(fs::read(f.path("dest")).unwrap(), b"new contents");
    assert_eq!(fs::read(f.path("old-alias")).unwrap(), b"old");
    assert!(!fs::read_dir(&f.0)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().as_encoded_bytes().starts_with(b".tcopy-")));
}

#[test]
fn recursive_mapping_empty_entries_and_trailing_slashes() {
    let f = Fixture::new();
    fs::create_dir_all(f.path("source/sub/empty")).unwrap();
    fs::write(f.path("source/sub/file with spaces"), b"data").unwrap();
    fs::write(f.path("source/empty-file"), b"").unwrap();
    success(f.copy(&["source/", "new/"]));
    assert_eq!(
        fs::read(f.path("new/sub/file with spaces")).unwrap(),
        b"data"
    );
    assert!(f.path("new/sub/empty").is_dir());
    assert_eq!(fs::metadata(f.path("new/empty-file")).unwrap().len(), 0);
    fs::create_dir(f.path("container")).unwrap();
    success(f.copy(&["source/", "container/"]));
    assert!(f.path("container/source/sub/empty").is_dir());
}

#[test]
fn sync_skips_same_size_different_contents_and_retains_extras() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::create_dir_all(f.path("dest/source")).unwrap();
    fs::write(f.path("source/same"), b"abcd").unwrap();
    fs::write(f.path("source/different"), b"longer").unwrap();
    fs::write(f.path("dest/source/same"), b"wxyz").unwrap();
    fs::write(f.path("dest/source/different"), b"old").unwrap();
    fs::write(f.path("dest/source/extra"), b"keep").unwrap();
    let output = f.copy(&["source", "dest", "--sync"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("do not guarantee"));
    assert!(success(output).contains("Skipped: 1"));
    assert_eq!(fs::read(f.path("dest/source/same")).unwrap(), b"wxyz");
    assert_eq!(
        fs::read(f.path("dest/source/different")).unwrap(),
        b"longer"
    );
    assert_eq!(fs::read(f.path("dest/source/extra")).unwrap(), b"keep");
    success(f.copy(&["source", "dest"]));
    assert_eq!(fs::read(f.path("dest/source/same")).unwrap(), b"abcd");
}

#[test]
fn dry_run_creates_nothing_and_preserves_destination_metadata() {
    let f = Fixture::new();
    fs::create_dir_all(f.path("source/sub")).unwrap();
    fs::write(f.path("source/sub/file"), b"data").unwrap();
    let output = success(f.copy(&["source", "absent", "--dry-run"]));
    assert!(output.contains("Planned: 1"));
    assert!(output.contains("mkdir"));
    assert!(!f.path("absent").exists());
    fs::create_dir_all(f.path("dest/source/sub")).unwrap();
    fs::write(f.path("dest/source/sub/file"), b"same").unwrap();
    let before = fs::metadata(f.path("dest/source/sub")).unwrap();
    let output = success(f.copy(&["source", "dest", "--dry-run", "--sync"]));
    assert!(output.contains("Skipped: 1"));
    assert_eq!(
        fs::metadata(f.path("dest/source/sub"))
            .unwrap()
            .mtime_nsec(),
        before.mtime_nsec()
    );
    assert_eq!(fs::read(f.path("dest/source/sub/file")).unwrap(), b"same");
}

#[test]
fn multiple_sources_and_collision_rejection() {
    let f = Fixture::new();
    fs::write(f.path("a"), b"a").unwrap();
    fs::write(f.path("b"), b"b").unwrap();
    fs::create_dir(f.path("dest")).unwrap();
    success(f.copy(&["a", "b", "dest"]));
    assert_eq!(fs::read(f.path("dest/a")).unwrap(), b"a");
    assert_eq!(fs::read(f.path("dest/b")).unwrap(), b"b");
    assert!(!f.copy(&["a", "b", "absent"]).status.success());
    fs::create_dir(f.path("other")).unwrap();
    fs::write(f.path("other/a"), b"other").unwrap();
    assert!(!f.copy(&["a", "other/a", "dest"]).status.success());
    assert_eq!(fs::read(f.path("dest/a")).unwrap(), b"a");
}

#[test]
fn rejects_self_copy_nested_destination_and_hardlink_alias() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::write(f.path("source/file"), b"keep").unwrap();
    fs::hard_link(f.path("source/file"), f.path("alias")).unwrap();
    for args in [
        vec!["source", "source/new"],
        vec!["source/file", "source/file"],
        vec!["source/file", "alias"],
        vec!["source", "."],
    ] {
        assert!(!f.copy(&args).status.success(), "{args:?}");
    }
    assert!(!f.path("source/new").exists());
    assert_eq!(fs::read(f.path("source/file")).unwrap(), b"keep");
}

#[test]
fn recreates_links_and_never_writes_through_destination_symlinks() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::write(f.path("source/file"), b"copy").unwrap();
    symlink("file", f.path("source/link")).unwrap();
    symlink("missing", f.path("source/dangling")).unwrap();
    symlink(".", f.path("source/loop")).unwrap();
    fs::write(f.path("victim"), b"keep").unwrap();
    symlink("victim", f.path("dest-file")).unwrap();
    success(f.copy(&["source/file", "dest-file", "--sync"]));
    assert!(!fs::symlink_metadata(f.path("dest-file"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(f.path("victim")).unwrap(), b"keep");
    success(f.copy(&["source", "dest"]));
    assert_eq!(
        fs::read_link(f.path("dest/link")).unwrap(),
        Path::new("file")
    );
    assert_eq!(
        fs::read_link(f.path("dest/dangling")).unwrap(),
        Path::new("missing")
    );
    assert_eq!(fs::read_link(f.path("dest/loop")).unwrap(), Path::new("."));
    assert_eq!(
        fs::symlink_metadata(f.path("source/link"))
            .unwrap()
            .mtime_nsec(),
        fs::symlink_metadata(f.path("dest/link"))
            .unwrap()
            .mtime_nsec()
    );
    success(f.copy(&["source/dangling/", "dangling-copy"]));
    assert_eq!(
        fs::read_link(f.path("dangling-copy")).unwrap(),
        Path::new("missing")
    );
}

#[test]
fn rejects_symlink_destination_directories_and_parents() {
    let f = Fixture::new();
    fs::create_dir_all(f.path("source/sub")).unwrap();
    fs::write(f.path("source/sub/file"), b"copy").unwrap();
    fs::create_dir(f.path("outside")).unwrap();
    symlink("outside", f.path("parent-link")).unwrap();
    for flags in [vec![], vec!["--dry-run"]] {
        let mut args = vec!["source/sub/file", "parent-link/file"];
        args.extend(flags);
        assert!(!f.copy(&args).status.success());
    }
    fs::create_dir_all(f.path("dest/source")).unwrap();
    symlink("../../outside", f.path("dest/source/sub")).unwrap();
    assert!(!f.copy(&["source", "dest"]).status.success());
    assert!(fs::read_dir(f.path("outside")).unwrap().next().is_none());
}

#[test]
fn preserves_permissions_and_nanosecond_mtimes_after_children_finish() {
    let f = Fixture::new();
    fs::create_dir_all(f.path("source/sub")).unwrap();
    fs::write(f.path("source/sub/file"), b"metadata").unwrap();
    let mtime = std::time::UNIX_EPOCH + std::time::Duration::new(1_600_000_000, 123_456_789);
    for (name, mode) in [
        ("source/sub/file", 0o640),
        ("source/sub", 0o550),
        ("source", 0o750),
    ] {
        fs::File::open(f.path(name))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(mtime))
            .unwrap();
        fs::set_permissions(f.path(name), fs::Permissions::from_mode(mode)).unwrap();
    }
    success(f.copy(&["source", "dest"]));
    for name in ["", "sub", "sub/file"] {
        let source = fs::metadata(f.path("source").join(name)).unwrap();
        let dest = fs::metadata(f.path("dest").join(name)).unwrap();
        assert_eq!(source.mode() & 0o7777, dest.mode() & 0o7777);
        assert_eq!(source.mtime(), dest.mtime());
        assert_eq!(source.mtime_nsec(), dest.mtime_nsec());
    }
}

#[test]
fn supports_unix_filenames_and_end_of_options() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    for name in [
        OsString::from("日本語\nfile"),
        OsString::from_vec(vec![b'f', 0xff, b'x']),
    ] {
        fs::write(f.path("source").join(&name), b"data").unwrap();
    }
    success(f.copy(&["source", "dest"]));
    for entry in fs::read_dir(f.path("source")).unwrap().flatten() {
        assert_eq!(
            fs::read(f.path("dest").join(entry.file_name())).unwrap(),
            b"data"
        );
    }
    fs::write(f.path("--sync"), b"literal").unwrap();
    success(f.copy(&["--", "--sync", "literal-copy"]));
    assert_eq!(fs::read(f.path("literal-copy")).unwrap(), b"literal");
}

#[test]
fn reports_special_files_and_continues_independent_copies() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::write(f.path("source/good"), b"data").unwrap();
    extern "C" {
        fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
    }
    use std::os::unix::ffi::OsStrExt;
    let fifo = std::ffi::CString::new(f.path("source/fifo").as_os_str().as_bytes()).unwrap();
    // SAFETY: the filename is NUL-terminated; mode_t is u32 on supported Linux targets.
    assert_eq!(unsafe { mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let output = f.copy(&["source", "dest"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported special file"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Failures: 1"));
    assert_eq!(fs::read(f.path("dest/good")).unwrap(), b"data");
    assert!(!f.path("dest/fifo").exists());
}

#[test]
fn directory_file_conflicts_fail_without_truncating_existing_data() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::write(f.path("source/file"), b"new").unwrap();
    fs::create_dir_all(f.path("dest/source/file")).unwrap();
    fs::write(f.path("dest/source/file/keep"), b"keep").unwrap();
    assert!(!f.copy(&["source", "dest"]).status.success());
    assert_eq!(fs::read(f.path("dest/source/file/keep")).unwrap(), b"keep");
    fs::write(f.path("target-file"), b"old").unwrap();
    assert!(!f.copy(&["source", "target-file"]).status.success());
    assert_eq!(fs::read(f.path("target-file")).unwrap(), b"old");
}

#[test]
fn streams_wide_and_deep_trees_and_copies_sparse_contents() {
    use std::io::{Seek, SeekFrom, Write};
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    for i in 0..512 {
        fs::write(f.path(format!("source/file-{i}")), i.to_string()).unwrap();
    }
    let mut deep = f.path("source");
    for _ in 0..128 {
        deep.push("d");
        fs::create_dir(&deep).unwrap();
    }
    fs::write(deep.join("deep"), b"deep").unwrap();
    let mut sparse = fs::File::create(f.path("source/sparse")).unwrap();
    sparse.seek(SeekFrom::Start(4 * 1024 * 1024)).unwrap();
    sparse.write_all(b"end").unwrap();
    success(f.copy(&["source", "dest"]));
    let deep_relative = deep.strip_prefix(f.path("source")).unwrap();
    assert_eq!(
        fs::read(f.path("dest").join(deep_relative).join("deep")).unwrap(),
        b"deep"
    );
    for i in 0..512 {
        assert_eq!(
            fs::read_to_string(f.path(format!("dest/file-{i}"))).unwrap(),
            i.to_string()
        );
    }
    assert_eq!(
        fs::read(f.path("source/sparse")).unwrap(),
        fs::read(f.path("dest/sparse")).unwrap()
    );
}

#[test]
fn explicit_dot_copies_contents_without_an_extra_directory() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::write(f.path("source/file"), b"data").unwrap();
    fs::create_dir(f.path("dest")).unwrap();
    success(f.copy(&["source/.", "dest"]));
    assert_eq!(fs::read(f.path("dest/file")).unwrap(), b"data");
    assert!(!f.path("dest/source").exists());
}

fn no_temporaries(directory: &Path) {
    assert!(!fs::read_dir(directory)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().as_encoded_bytes().starts_with(b".tcopy-")));
}

#[test]
fn permission_failure_keeps_existing_destination() {
    let f = Fixture::new();
    fs::write(f.path("source"), b"unreadable").unwrap();
    fs::write(f.path("dest"), b"keep").unwrap();
    fs::set_permissions(f.path("source"), fs::Permissions::from_mode(0o0)).unwrap();
    let output = f.copy(&["source", "dest"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read(f.path("dest")).unwrap(), b"keep");
    no_temporaries(&f.0);
}

#[test]
fn write_failure_cleans_temporary_and_keeps_existing_destination() {
    let f = Fixture::new();
    fs::write(f.path("source"), vec![42; 1024 * 1024]).unwrap();
    fs::write(f.path("dest"), b"keep").unwrap();
    let output = Command::new("sh")
        .current_dir(&f.0)
        .args([
            "-c",
            "trap '' XFSZ; ulimit -f 1; exec \"$1\" source dest",
            "sh",
            env!("CARGO_BIN_EXE_tcopy"),
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(f.path("dest")).unwrap(), b"keep");
    no_temporaries(&f.0);
}

struct Running(Option<std::process::Child>);
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Running {
    fn start(f: &Fixture, args: &[&str]) -> Self {
        Self(Some(
            Command::new(env!("CARGO_BIN_EXE_tcopy"))
                .current_dir(&f.0)
                .args(args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        ))
    }
    fn wait_for_data(&mut self, directory: &Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if fs::read_dir(directory).is_ok_and(|entries| {
                entries.flatten().any(|entry| {
                    entry.file_name().as_encoded_bytes().starts_with(b".tcopy-")
                        && entry.metadata().is_ok_and(|m| m.len() > 0)
                })
            }) {
                return;
            }
            assert!(
                self.0.as_mut().unwrap().try_wait().unwrap().is_none(),
                "copy finished before observation"
            );
            assert!(std::time::Instant::now() < deadline, "copy did not start");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    fn finish(mut self) -> Output {
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}

#[test]
fn sigint_and_sigterm_keep_old_destination_and_clean_up() {
    extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    for signal in [2, 15] {
        let f = Fixture::new();
        fs::File::create(f.path("source"))
            .unwrap()
            .set_len(2 * 1024 * 1024 * 1024)
            .unwrap();
        fs::write(f.path("dest"), b"keep").unwrap();
        let mut child = Running::start(&f, &["source", "dest"]);
        child.wait_for_data(&f.0);
        // SAFETY: sends a supported signal to our own live child process.
        assert_eq!(
            unsafe { kill(child.0.as_ref().unwrap().id() as i32, signal) },
            0
        );
        let output = child.finish();
        assert_eq!(output.status.code(), Some(128 + signal));
        assert_eq!(fs::read(f.path("dest")).unwrap(), b"keep");
        no_temporaries(&f.0);
    }
}

#[test]
fn changing_source_is_reported_without_publishing_partial_data() {
    let f = Fixture::new();
    let source = fs::File::create(f.path("source")).unwrap();
    source.set_len(2 * 1024 * 1024 * 1024).unwrap();
    fs::write(f.path("dest"), b"keep").unwrap();
    let mut child = Running::start(&f, &["source", "dest"]);
    child.wait_for_data(&f.0);
    source.set_len(0).unwrap();
    let output = child.finish();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("source"));
    assert_eq!(fs::read(f.path("dest")).unwrap(), b"keep");
    no_temporaries(&f.0);
}

#[test]
fn destination_parent_symlink_swap_cannot_redirect_opened_directory() {
    let f = Fixture::new();
    fs::File::create(f.path("source"))
        .unwrap()
        .set_len(64 * 1024 * 1024)
        .unwrap();
    fs::create_dir(f.path("dest")).unwrap();
    fs::create_dir(f.path("outside")).unwrap();
    fs::write(f.path("outside/source"), b"keep").unwrap();
    let mut child = Running::start(&f, &["source", "dest"]);
    child.wait_for_data(&f.path("dest"));
    fs::rename(f.path("dest"), f.path("moved")).unwrap();
    symlink("outside", f.path("dest")).unwrap();
    success(child.finish());
    assert_eq!(fs::read(f.path("outside/source")).unwrap(), b"keep");
    assert_eq!(
        fs::metadata(f.path("moved/source")).unwrap().len(),
        64 * 1024 * 1024
    );
    no_temporaries(&f.path("moved"));
}

#[test]
fn refuses_special_destinations_and_file_paths_with_trailing_slashes() {
    extern "C" {
        fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
    }
    use std::os::unix::{ffi::OsStrExt, fs::FileTypeExt};
    let f = Fixture::new();
    fs::write(f.path("source"), b"data").unwrap();
    let path = std::ffi::CString::new(f.path("fifo").as_os_str().as_bytes()).unwrap();
    // SAFETY: terminated path; mode_t is u32 on supported Linux targets.
    assert_eq!(unsafe { mkfifo(path.as_ptr(), 0o600) }, 0);
    for args in [
        vec!["source", "fifo"],
        vec!["source", "fifo", "--dry-run"],
        vec!["source/", "dest"],
        vec!["source", "dest/"],
    ] {
        assert!(!f.copy(&args).status.success(), "{args:?}");
    }
    assert!(fs::symlink_metadata(f.path("fifo"))
        .unwrap()
        .file_type()
        .is_fifo());
    assert!(!f.path("dest").exists());
}
