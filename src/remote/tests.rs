use super::*;
use crate::cli::Options;
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{symlink, MetadataExt, PermissionsExt},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tcopy-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn run(&self, sources: &[&str], destination: &str, options: Options) -> Arc<Summary> {
        run(
            &Command {
                sources: sources.iter().map(|path| self.0.join(path)).collect(),
                destination: self.0.join(destination),
                options,
                ssh_program: OsString::from("tcopy"),
            },
            Instant::now(),
        )
    }
    fn endpoint(&self) -> Arc<Endpoint> {
        Endpoint::start(None, std::ffi::OsStr::new("tcopy"), 4).unwrap()
    }
    fn source(&self, endpoint: &Endpoint) -> wire::Info {
        endpoint
            .connect()
            .unwrap()
            .stat(&self.0.join("source"), false)
            .unwrap()
            .2
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn successful(summary: &Summary) {
    assert_eq!(summary.failures.load(Ordering::Relaxed), 0);
}

#[test]
fn native_parallel_copy_preserves_bytes_and_atomic_overwrite() {
    let f = Fixture::new();
    let bytes: Vec<_> = (0..3 * 1024 * 1024 + 17)
        .map(|index| (index % 251) as u8)
        .collect();
    fs::write(f.0.join("source"), &bytes).unwrap();
    fs::write(f.0.join("dest"), b"old").unwrap();
    fs::hard_link(f.0.join("dest"), f.0.join("alias")).unwrap();
    let summary = f.run(
        &["source"],
        "dest",
        Options {
            workers: Some(4),
            chunk_size: Some(1 << 20),
            manual_chunks: true,
            ..Options::default()
        },
    );
    successful(&summary);
    assert_eq!(summary.chunked.load(Ordering::Relaxed), 1);
    assert!(summary.peak.load(Ordering::Relaxed) <= 4);
    assert_eq!(fs::read(f.0.join("dest")).unwrap(), bytes);
    assert_eq!(fs::read(f.0.join("alias")).unwrap(), b"old");
    assert!(fs::read_dir(&f.0).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
}

#[test]
fn native_tree_mapping_metadata_links_and_contents() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("tree/empty")).unwrap();
    fs::write(f.0.join("tree/file"), b"contents").unwrap();
    fs::set_permissions(f.0.join("tree/file"), fs::Permissions::from_mode(0o640)).unwrap();
    symlink("file", f.0.join("tree/link")).unwrap();
    successful(&f.run(&["tree/"], "new/", Options::default()));
    assert_eq!(fs::read(f.0.join("new/file")).unwrap(), b"contents");
    assert!(f.0.join("new/empty").is_dir());
    assert_eq!(
        fs::read_link(f.0.join("new/link")).unwrap(),
        PathBuf::from("file")
    );
    assert_eq!(
        fs::metadata(f.0.join("new/file")).unwrap().mode() & 0o777,
        0o640
    );
    assert_eq!(
        fs::metadata(f.0.join("new/file"))
            .unwrap()
            .modified()
            .unwrap(),
        fs::metadata(f.0.join("tree/file"))
            .unwrap()
            .modified()
            .unwrap()
    );
    successful(&f.run(&["tree"], "new", Options::default()));
    assert!(f.0.join("new/tree/file").is_file());
    successful(&f.run(&["tree/."], "contents", Options::default()));
    assert!(f.0.join("contents/file").is_file());
}

#[test]
fn native_sync_checksum_sparse_and_dry_run() {
    let f = Fixture::new();
    fs::write(f.0.join("source"), b"new").unwrap();
    fs::write(f.0.join("dest"), b"old").unwrap();
    let sync = Options {
        sync: true,
        ..Options::default()
    };
    let summary = f.run(&["source"], "dest", sync);
    successful(&summary);
    assert_eq!(summary.skipped.load(Ordering::Relaxed), 1);
    assert_eq!(fs::read(f.0.join("dest")).unwrap(), b"old");
    successful(&f.run(
        &["source"],
        "dest",
        Options {
            checksum: true,
            dry_run: true,
            ..sync
        },
    ));
    assert_eq!(fs::read(f.0.join("dest")).unwrap(), b"old");
    successful(&f.run(
        &["source"],
        "dest",
        Options {
            checksum: true,
            ..sync
        },
    ));
    assert_eq!(fs::read(f.0.join("dest")).unwrap(), b"new");
    fs::write(f.0.join("source"), vec![0; 2 << 20]).unwrap();
    successful(&f.run(
        &["source"],
        "sparse",
        Options {
            sparse: true,
            ..Options::default()
        },
    ));
    assert_eq!(fs::metadata(f.0.join("sparse")).unwrap().len(), 2 << 20);
    assert_eq!(fs::metadata(f.0.join("sparse")).unwrap().blocks(), 0);
    successful(&f.run(
        &["source"],
        "preview",
        Options {
            dry_run: true,
            ..Options::default()
        },
    ));
    assert!(!f.0.join("preview").exists());
}

#[test]
fn native_rejects_symlink_parents_overlap_collisions_and_missing_sources() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("tree")).unwrap();
    fs::write(f.0.join("source"), b"bytes").unwrap();
    symlink("tree", f.0.join("link")).unwrap();
    for (sources, destination) in [
        (vec!["source"], "link/file"),
        (vec!["tree"], "tree/nested"),
        (vec!["source"], "source"),
        (vec!["source", "source"], "tree"),
        (vec!["tree"], "link"),
        (vec!["missing"], "dest"),
    ] {
        assert_ne!(
            f.run(&sources, destination, Options::default())
                .failures
                .load(Ordering::Relaxed),
            0
        );
    }
    assert!(!f.0.join("tree/file").exists());
    assert!(!f.0.join("tree/nested").exists());
}

#[test]
fn incomplete_duplicate_and_disconnected_ranges_never_publish() {
    let f = Fixture::new();
    fs::write(f.0.join("source"), vec![7; 2048]).unwrap();
    fs::write(f.0.join("dest"), b"old").unwrap();
    let endpoint = f.endpoint();
    let info = f.source(&endpoint);
    for duplicate in [false, true] {
        let mut owner = endpoint.connect().unwrap();
        let (parent, name, _) = owner.stat(&f.0.join("dest"), true).unwrap();
        let stage = owner.begin(parent, &name, &info, 1024, true).unwrap();
        let mut writer = endpoint.connect().unwrap();
        writer.write_start(stage, 0, false).unwrap();
        writer.stream.write_all(&[7; 1024]).unwrap();
        writer.write_finish().unwrap();
        if duplicate {
            assert!(writer.write_start(stage, 0, false).is_err());
        }
        assert!(owner.commit(stage).is_err());
        drop(owner);
        drop(writer);
        assert_eq!(fs::read(f.0.join("dest")).unwrap(), b"old");
    }
    let mut owner = endpoint.connect().unwrap();
    let (parent, name, _) = owner.stat(&f.0.join("dest"), true).unwrap();
    let stage = owner.begin(parent, &name, &info, 1024, true).unwrap();
    let mut writer = endpoint.connect().unwrap();
    writer.write_start(stage, 0, false).unwrap();
    writer.stream.write_all(&[7; 10]).unwrap();
    drop(writer);
    drop(owner);
    drop(endpoint);
    assert_eq!(fs::read(f.0.join("dest")).unwrap(), b"old");
    assert!(fs::read_dir(&f.0).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
}

#[test]
fn native_checks_source_identity_and_authentication() {
    let f = Fixture::new();
    fs::write(f.0.join("source"), b"before").unwrap();
    let endpoint = f.endpoint();
    let info = f.source(&endpoint);
    fs::write(f.0.join("source"), b"after!").unwrap();
    assert!(endpoint
        .connect()
        .unwrap()
        .read_start(&info, None, 0, info.size)
        .is_err());
    let mut socket = std::net::TcpStream::connect(endpoint.address).unwrap();
    socket.write_all(wire::MAGIC).unwrap();
    socket.write_all(&[0; 32]).unwrap();
    assert_eq!(socket.read(&mut [0]).unwrap(), 0);
}

#[test]
fn native_detects_source_changes_after_streaming_begins() {
    use std::io::Read;
    use std::os::unix::fs::FileExt;
    let f = Fixture::new();
    fs::write(f.0.join("source"), vec![7; 8 << 20]).unwrap();
    let endpoint = f.endpoint();
    let info = f.source(&endpoint);
    let mut client = endpoint.connect().unwrap();
    client.read_start(&info, None, 0, info.size).unwrap();
    client.stream.read_exact(&mut [0; 1]).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(f.0.join("source"))
        .unwrap()
        .write_all_at(b"changed", 0)
        .unwrap();
    std::io::copy(
        &mut (&mut client.stream).take(info.size - 1),
        &mut std::io::sink(),
    )
    .unwrap();
    assert!(client.read_finish().is_err());
}

#[test]
fn native_streams_multiple_directory_batches_with_reused_workers() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("tree")).unwrap();
    for index in 0..140 {
        fs::write(f.0.join(format!("tree/{index}")), index.to_string()).unwrap();
    }
    let summary = f.run(
        &["tree"],
        "copied",
        Options {
            workers: Some(4),
            ..Options::default()
        },
    );
    successful(&summary);
    assert_eq!(summary.copied.load(Ordering::Relaxed), 140);
    for index in 0..140 {
        assert_eq!(
            fs::read_to_string(f.0.join(format!("copied/{index}"))).unwrap(),
            index.to_string()
        );
    }
}

#[test]
fn destination_parent_remains_anchored_after_path_replacement() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("parent")).unwrap();
    fs::create_dir(f.0.join("outside")).unwrap();
    fs::write(f.0.join("source"), b"safe").unwrap();
    let endpoint = f.endpoint();
    let info = f.source(&endpoint);
    let mut owner = endpoint.connect().unwrap();
    let (parent, name, _) = owner.stat(&f.0.join("parent/dest"), true).unwrap();
    fs::rename(f.0.join("parent"), f.0.join("retained")).unwrap();
    symlink("outside", f.0.join("parent")).unwrap();
    let stage = owner.begin(parent, &name, &info, 1024, true).unwrap();
    owner.write_start(stage, 0, false).unwrap();
    owner.stream.write_all(b"safe").unwrap();
    owner.write_finish().unwrap();
    owner.commit(stage).unwrap();
    assert_eq!(fs::read(f.0.join("retained/dest")).unwrap(), b"safe");
    assert!(!f.0.join("outside/dest").exists());
}
