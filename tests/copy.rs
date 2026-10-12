use std::{
    ffi::{OsStr, OsString},
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{symlink, MetadataExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}
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
        vec!["--workers", "0"],
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
fn gpfs_profile_keeps_atomic_replacement_and_reports_durability() {
    let f = Fixture::new();
    fs::write(f.path("source"), b"new contents").unwrap();
    fs::write(f.path("dest"), b"old").unwrap();
    fs::hard_link(f.path("dest"), f.path("alias")).unwrap();
    let report = success(f.copy(&[
        "--gpfs",
        "--no-fsync",
        "--workers=3",
        "--json",
        "source",
        "dest",
    ]));
    assert!(report.contains("\"workers\":3"));
    assert!(report.contains("\"fsync\":false"));
    assert!(report.contains("\"gpfs\":true"));
    assert!(report.contains("\"concurrency_adjustments\":0"));
    assert_eq!(fs::read(f.path("dest")).unwrap(), b"new contents");
    assert_eq!(fs::read(f.path("alias")).unwrap(), b"old");
}

#[test]
fn no_fsync_preserves_tree_metadata_and_symlinks() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::write(f.path("source/file"), b"payload").unwrap();
    symlink("file", f.path("source/link")).unwrap();
    fs::set_permissions(f.path("source/file"), fs::Permissions::from_mode(0o640)).unwrap();
    success(f.copy(&["--no-fsync", "source", "dest"]));
    assert_eq!(fs::read(f.path("dest/file")).unwrap(), b"payload");
    assert_eq!(
        fs::read_link(f.path("dest/link")).unwrap(),
        Path::new("file")
    );
    for name in ["", "/file", "/link"] {
        let src = fs::symlink_metadata(f.path(format!("source{name}"))).unwrap();
        let dst = fs::symlink_metadata(f.path(format!("dest{name}"))).unwrap();
        assert_eq!(src.mode(), dst.mode());
        assert_eq!(src.mtime_nsec(), dst.mtime_nsec());
        assert_eq!(src.mtime(), dst.mtime());
    }
}

#[test]
fn gpfs_batches_directory_finalization_in_post_order() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::new(1_600_000_000, 123_456_789);
    for i in 0..130 {
        let dir = f.path(format!("source/{i}"));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("file"), i.to_string()).unwrap();
        fs::File::open(&dir)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o550)).unwrap();
    }
    fs::File::open(f.path("source"))
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    success(f.copy(&["--gpfs", "--no-fsync", "--workers=4", "source", "dest"]));
    for i in 0..130 {
        assert_eq!(
            fs::read_to_string(f.path(format!("dest/{i}/file"))).unwrap(),
            i.to_string()
        );
        let dir = fs::metadata(f.path(format!("dest/{i}"))).unwrap();
        assert_eq!(dir.mode() & 0o777, 0o550);
        assert_eq!(dir.modified().unwrap(), modified);
    }
    assert_eq!(
        fs::metadata(f.path("dest")).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn gpfs_profile_chunks_large_files_with_exact_partial_tail() {
    use std::io::{Seek, SeekFrom, Write};
    let f = Fixture::new();
    let mut file = fs::File::create(f.path("source")).unwrap();
    file.set_len((64 << 20) + 17).unwrap();
    file.seek(SeekFrom::Start(16 << 20)).unwrap();
    file.write_all(b"chunk boundary").unwrap();
    file.seek(SeekFrom::End(-17)).unwrap();
    file.write_all(b"partial tail test").unwrap();
    drop(file);
    let report = success(f.copy(&[
        "--gpfs",
        "--no-fsync",
        "--workers=4",
        "--json",
        "source",
        "dest",
    ]));
    assert!(report.contains("\"chunked_files\":1"));
    assert!(report.contains("\"buffer_size\":1048576"));
    assert_eq!(
        fs::read(f.path("source")).unwrap(),
        fs::read(f.path("dest")).unwrap()
    );
    no_temporaries(&f.0);
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
fn checksum_sync_compares_contents_even_in_dry_run() {
    let f = Fixture::new();
    fs::write(f.path("source"), b"abcd").unwrap();
    fs::write(f.path("dest"), b"wxyz").unwrap();
    assert_eq!(
        f.copy(&["source", "dest", "--checksum"]).status.code(),
        Some(2)
    );
    let preview = success(f.copy(&["source", "dest", "--sync", "--checksum", "--dry-run"]));
    assert!(preview.contains("copy "));
    assert_eq!(fs::read(f.path("dest")).unwrap(), b"wxyz");
    success(f.copy(&["source", "dest", "--sync", "--checksum"]));
    assert_eq!(fs::read(f.path("dest")).unwrap(), b"abcd");
    let skipped = success(f.copy(&["source", "dest", "--sync", "--checksum"]));
    assert!(skipped.contains("Skipped: 1"));
}

#[test]
fn sparse_and_reflink_options_copy_exact_data() {
    let f = Fixture::new();
    let source = fs::File::create(f.path("source")).unwrap();
    use std::os::unix::fs::FileExt;
    source.write_all_at(b"head", 0).unwrap();
    source.write_all_at(b"tail", 4 * 1024 * 1024).unwrap();
    source.set_len(8 * 1024 * 1024).unwrap();
    for (flag, dest) in [("--sparse", "sparse"), ("--reflink", "reflink")] {
        success(f.copy(&["source", dest, flag]));
        assert_eq!(
            fs::read(f.path(dest)).unwrap(),
            fs::read(f.path("source")).unwrap()
        );
        assert_eq!(fs::metadata(f.path(dest)).unwrap().len(), 8 * 1024 * 1024);
    }
}

#[test]
fn resume_retains_valid_partial_and_rejects_changed_source() {
    let f = Fixture::new();
    fs::write(f.path("source"), vec![42; 1024 * 1024]).unwrap();
    fs::write(f.path("dest"), b"keep").unwrap();
    let failed = Command::new("sh")
        .current_dir(&f.0)
        .args([
            "-c",
            "trap '' XFSZ; ulimit -f 1; exec \"$1\" source dest --resume",
            "sh",
            env!("CARGO_BIN_EXE_tcopy"),
        ])
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(1));
    assert_eq!(fs::read(f.path("dest")).unwrap(), b"keep");
    assert!(f.path(".tcopy-resume").is_dir());
    success(f.copy(&["source", "dest", "--resume"]));
    assert_eq!(
        fs::read(f.path("dest")).unwrap(),
        fs::read(f.path("source")).unwrap()
    );

    fs::write(f.path("source"), vec![43; 1024 * 1024]).unwrap();
    let failed = Command::new("sh")
        .current_dir(&f.0)
        .args([
            "-c",
            "trap '' XFSZ; ulimit -f 1; exec \"$1\" source dest --resume",
            "sh",
            env!("CARGO_BIN_EXE_tcopy"),
        ])
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(1));
    fs::write(f.path("source"), vec![44; 1024 * 1024]).unwrap();
    assert_eq!(
        f.copy(&["source", "dest", "--resume"]).status.code(),
        Some(1)
    );
    assert_eq!(fs::read(f.path("dest")).unwrap(), vec![42; 1024 * 1024]);
}

#[test]
fn resume_rejects_tampered_symlinked_hardlinked_and_locked_checkpoints() {
    let f = Fixture::new();
    fs::write(f.path("source"), vec![42; 1024 * 1024]).unwrap();
    fs::write(f.path("dest"), b"keep").unwrap();
    let failed = Command::new("sh")
        .current_dir(&f.0)
        .args([
            "-c",
            "trap '' XFSZ; ulimit -f 1; exec \"$1\" source dest --resume",
            "sh",
            env!("CARGO_BIN_EXE_tcopy"),
        ])
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(1));
    let checkpoint = f.path(".tcopy-resume");
    let entries: Vec<_> = fs::read_dir(&checkpoint)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    let part = entries
        .iter()
        .find(|p| p.extension() == Some(OsStr::new("part")))
        .unwrap();
    let state = entries
        .iter()
        .find(|p| p.extension() == Some(OsStr::new("state")))
        .unwrap();
    let file = fs::OpenOptions::new().write(true).open(part).unwrap();
    use std::os::unix::fs::FileExt;
    file.write_all_at(b"X", 0).unwrap();
    assert_eq!(
        f.copy(&["source", "dest", "--resume"]).status.code(),
        Some(1)
    );
    file.write_all_at(b"*", 0).unwrap();
    fs::hard_link(part, f.path("alias")).unwrap();
    assert_eq!(
        f.copy(&["source", "dest", "--resume"]).status.code(),
        Some(1)
    );
    fs::remove_file(f.path("alias")).unwrap();
    fs::rename(state, f.path("saved-state")).unwrap();
    symlink(f.path("source"), state).unwrap();
    assert_eq!(
        f.copy(&["source", "dest", "--resume"]).status.code(),
        Some(1)
    );
    fs::remove_file(state).unwrap();
    fs::rename(f.path("saved-state"), state).unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(part)
        .unwrap();
    // SAFETY: live fd and Linux LOCK_EX | LOCK_NB / LOCK_UN constants.
    assert_eq!(unsafe { flock(held.as_raw_fd(), 2 | 4) }, 0);
    assert_eq!(
        f.copy(&["source", "dest", "--resume"]).status.code(),
        Some(1)
    );
    assert_eq!(unsafe { flock(held.as_raw_fd(), 8) }, 0);
    success(f.copy(&["source", "dest", "--resume"]));
    assert_eq!(fs::read(f.path("dest")).unwrap(), vec![42; 1024 * 1024]);
}

#[test]
fn resume_refuses_reserved_destination_name_before_mutation() {
    let f = Fixture::new();
    fs::write(f.path("source"), b"data").unwrap();
    symlink("source", f.path("source-link")).unwrap();
    for source in ["source", "source-link"] {
        assert_eq!(
            f.copy(&[source, ".tcopy-resume", "--resume"]).status.code(),
            Some(1)
        );
        assert!(!f.path(".tcopy-resume").exists());
    }
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
    for mode in ["buffered", "chunks", "tune", "gpfs"] {
        let f = Fixture::new();
        fs::write(f.path("source"), vec![42; 1024 * 1024]).unwrap();
        if mode == "chunks" {
            fs::OpenOptions::new()
                .write(true)
                .open(f.path("source"))
                .unwrap()
                .set_len(64 * 1024 * 1024)
                .unwrap();
        }
        fs::write(f.path("dest"), b"keep").unwrap();
        let script = if mode == "chunks" {
            "trap '' XFSZ; ulimit -f 1; exec \"$1\" source dest --workers=4 --chunk-size=8M"
        } else if mode == "tune" {
            "trap '' XFSZ; ulimit -f 1; exec \"$1\" source dest --tune --json"
        } else if mode == "gpfs" {
            "trap '' XFSZ; ulimit -f 1; exec \"$1\" source dest --gpfs --no-fsync --workers=3"
        } else {
            "trap '' XFSZ; ulimit -f 1; exec \"$1\" source dest"
        };
        let output = Command::new("sh")
            .current_dir(&f.0)
            .args(["-c", script, "sh", env!("CARGO_BIN_EXE_tcopy")])
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
                        && entry.metadata().is_ok_and(|m| m.blocks() > 0)
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
    for (signal, mode) in [
        (2, "buffered"),
        (15, "buffered"),
        (2, "chunks"),
        (15, "chunks"),
        (2, "tune"),
        (15, "tune"),
        (2, "gpfs"),
        (15, "gpfs"),
    ] {
        let f = Fixture::new();
        fs::File::create(f.path("source"))
            .unwrap()
            .set_len(2 * 1024 * 1024 * 1024)
            .unwrap();
        fs::write(f.path("dest"), b"keep").unwrap();
        let args = if mode == "chunks" {
            vec!["source", "dest", "--workers=4", "--chunk-size=8M", "--json"]
        } else if mode == "tune" {
            vec!["source", "dest", "--tune", "--json"]
        } else if mode == "gpfs" {
            vec!["source", "dest", "--gpfs", "--no-fsync", "--workers=4"]
        } else {
            vec!["source", "dest"]
        };
        let mut child = Running::start(&f, &args);
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
    for mode in ["buffered", "chunks", "tune", "gpfs"] {
        let f = Fixture::new();
        let source = fs::File::create(f.path("source")).unwrap();
        source.set_len(2 * 1024 * 1024 * 1024).unwrap();
        fs::write(f.path("dest"), b"keep").unwrap();
        let args = if mode == "chunks" {
            vec!["source", "dest", "--workers=4", "--chunk-size=8M", "--json"]
        } else if mode == "tune" {
            vec!["source", "dest", "--tune", "--json"]
        } else if mode == "gpfs" {
            vec!["source", "dest", "--gpfs", "--no-fsync", "--workers=4"]
        } else {
            vec!["source", "dest"]
        };
        let mut child = Running::start(&f, &args);
        child.wait_for_data(&f.0);
        source.set_len(0).unwrap();
        let output = child.finish();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("source"));
        assert_eq!(fs::read(f.path("dest")).unwrap(), b"keep");
        no_temporaries(&f.0);
    }
}

#[test]
fn destination_parent_symlink_swap_cannot_redirect_opened_directory() {
    for args in [
        vec!["source", "dest"],
        vec!["source", "dest", "--workers=4", "--chunk-size=8M"],
        vec!["source", "dest", "--tune", "--json"],
    ] {
        let f = Fixture::new();
        fs::File::create(f.path("source"))
            .unwrap()
            .set_len(64 * 1024 * 1024)
            .unwrap();
        fs::create_dir(f.path("dest")).unwrap();
        fs::create_dir(f.path("outside")).unwrap();
        fs::write(f.path("outside/source"), b"keep").unwrap();
        let mut child = Running::start(&f, &args);
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

fn json_number(summary: &str, name: &str) -> f64 {
    let key = format!("\"{name}\":");
    summary
        .split_once(&key)
        .unwrap()
        .1
        .split([',', '}'])
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn adaptive_flags_preserve_manual_controls_and_dry_run() {
    let f = Fixture::new();
    fs::write(f.path("source"), b"adaptive copy").unwrap();
    let default = success(f.copy(&["--json", "source", "default-dest"]));
    assert!(default.contains("\"auto\":true"));
    assert_eq!(json_number(&default, "concurrency_adjustments"), 0.0);
    assert_eq!(
        json_number(&default, "final_workers"),
        json_number(&default, "workers")
    );
    for flag in ["--auto", "--tune"] {
        let output = success(f.copy(&[
            flag,
            "--workers=1",
            "--chunk-size=4M",
            "--json",
            "source",
            "dest",
        ]));
        assert_eq!(json_number(&output, "workers"), 1.0);
        assert_eq!(json_number(&output, "chunk_size"), 4194304.0);
        assert_eq!(json_number(&output, "final_workers"), 1.0);
        assert_eq!(fs::read(f.path("dest")).unwrap(), b"adaptive copy");
        no_temporaries(&f.0);
    }
    let output = success(f.copy(&[
        "--auto",
        "--no-tune",
        "--dry-run",
        "--json",
        "source",
        "preview",
    ]));
    assert!(!f.path("preview").exists());
    assert!(output.contains("\"tuning_state\":\"disabled\""));
    assert!(output.contains("\"hardware\":{") && output.contains("\"workload\":{"));
    let bad = f.copy(&["--tune", "--no-tune", "source", "bad"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(!f.path("bad").exists());
    let skip = success(f.copy(&["--tune", "--sync", "--json", "source", "dest"]));
    assert_eq!(json_number(&skip, "skipped"), 1.0);
    assert_eq!(json_number(&skip, "transferred_bytes"), 0.0);
    let failure = f.copy(&["--auto", "--json", "missing", "dest"]);
    assert_eq!(failure.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&failure.stdout).lines().count(), 1);
}

#[test]
fn chunk_controls_copy_partial_tail_and_share_budget_across_files() {
    use std::io::Write;
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    let mut input = fs::File::create(f.path("source/large")).unwrap();
    let mut block = vec![0; 1024 * 1024];
    for index in 0..64 {
        for (offset, byte) in block.iter_mut().enumerate() {
            *byte = ((offset * 17 + index * 31) % 251) as u8;
        }
        input.write_all(&block).unwrap();
    }
    input.write_all(b"partial last tail").unwrap();
    input
        .set_permissions(fs::Permissions::from_mode(0o640))
        .unwrap();
    input
        .set_times(
            fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::new(1_600_000_001, 1234),
            ),
        )
        .unwrap();
    drop(input);
    for flag in ["--auto", "--tune"] {
        let dest = format!("adaptive-{}", &flag[2..]);
        let summary = success(f.copy(&["source/large", &dest, flag, "--json"]));
        assert_eq!(
            json_number(&summary, "bytes"),
            (64 * 1024 * 1024 + 17) as f64
        );
        assert_eq!(json_number(&summary, "large_files"), 1.0);
        assert!(json_number(&summary, "peak_active_workers") <= json_number(&summary, "workers"));
        assert_eq!(
            fs::read(f.path(&dest)).unwrap(),
            fs::read(f.path("source/large")).unwrap()
        );
    }
    fs::copy(f.path("source/large"), f.path("source/other")).unwrap();
    fs::write(f.path("source/small"), b"small").unwrap();
    for workers in [1, 4] {
        let dest = format!("dest-{workers}");
        let output = f.copy(&[
            "source",
            &dest,
            &format!("--workers={workers}"),
            "--chunk-size=8M",
            "--json",
            "--progress",
        ]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("discovered"));
        let summary = success(output);
        assert_eq!(summary.lines().count(), 1);
        assert_eq!(json_number(&summary, "workers"), workers as f64);
        assert_eq!(json_number(&summary, "copied"), 3.0);
        assert_eq!(json_number(&summary, "failures"), 0.0);
        assert!(json_number(&summary, "peak_active_workers") <= workers as f64);
        assert!(json_number(&summary, "worker_utilization") <= 1.0);
        assert_eq!(
            json_number(&summary, "bytes"),
            json_number(&summary, "transferred_bytes")
        );
        if workers == 1 {
            assert_eq!(json_number(&summary, "chunked_files"), 0.0);
        }
        for name in ["large", "other", "small"] {
            let source = f.path(format!("source/{name}"));
            let destination = f.path(format!("{dest}/{name}"));
            assert_eq!(fs::read(&destination).unwrap(), fs::read(&source).unwrap());
            let before = fs::metadata(source).unwrap();
            let after = fs::metadata(destination).unwrap();
            assert_eq!(before.mode() & 0o7777, after.mode() & 0o7777);
            assert_eq!(
                (before.mtime(), before.mtime_nsec()),
                (after.mtime(), after.mtime_nsec())
            );
        }
        no_temporaries(&f.path(dest));
    }
    let single = success(f.copy(&[
        "source/large",
        "single",
        "--workers=4",
        "--chunk-size=8M",
        "--json",
    ]));
    // On recognized local storage a lone large file has idle slots to use.
    if ["btrfs", "ext4", "xfs", "tmpfs", "ramfs", "f2fs", "zfs"]
        .iter()
        .any(|fs| single.contains(&format!("\"filesystem\":\"{fs}\"")))
        && !single.contains("\"kind\":\"hdd\"")
    {
        assert_eq!(json_number(&single, "chunked_files"), 1.0);
        assert_eq!(json_number(&single, "peak_active_workers"), 4.0);
    }
}

#[test]
fn json_dry_run_sync_and_failure_keep_stdout_machine_readable() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::write(f.path("source/a\"\n"), b"data").unwrap();
    let output = f.copy(&["source", "dest", "--dry-run", "--json"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("mkdir"));
    let summary = success(output);
    assert_eq!(summary.lines().count(), 1);
    assert!(summary.starts_with('{') && summary.trim_end().ends_with('}'));
    assert_eq!(json_number(&summary, "bytes"), 4.0);
    assert_eq!(json_number(&summary, "transferred_bytes"), 0.0);
    assert_eq!(json_number(&summary, "throughput_bytes_per_second"), 0.0);
    assert!(!f.path("dest").exists());
    success(f.copy(&["source", "dest"]));
    let summary = success(f.copy(&["source/.", "dest", "--sync", "--json"]));
    assert_eq!(json_number(&summary, "skipped"), 1.0);
    assert_eq!(json_number(&summary, "transferred_bytes"), 0.0);
    let output = f.copy(&["missing", "dest", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let summary = String::from_utf8(output.stdout).unwrap();
    assert_eq!(summary.lines().count(), 1);
    assert_eq!(json_number(&summary, "failures"), 1.0);
}
