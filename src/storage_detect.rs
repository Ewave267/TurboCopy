use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{FileTypeExt, MetadataExt},
    },
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Storage {
    pub filesystem: String,
    pub device: String,
    pub kind: &'static str,
    pub backing: BTreeSet<String>,
    pub complete: bool,
    pub dev: u64,
}
impl Storage {
    pub fn chunks_allowed(&self) -> bool {
        self.kind != "hdd"
            && matches!(
                self.filesystem.as_str(),
                "ext4" | "xfs" | "btrfs" | "tmpfs" | "ramfs" | "f2fs" | "zfs"
            )
    }
    pub fn shared_with(&self, other: &Self) -> Option<bool> {
        if self.dev == other.dev || !self.backing.is_disjoint(&other.backing) {
            Some(true)
        } else if self.complete && other.complete {
            Some(false)
        } else {
            None
        }
    }
}

struct Mount {
    root: PathBuf,
    point: PathBuf,
    dev: String,
    filesystem: String,
    source: PathBuf,
}

pub struct Detector {
    mounts: Vec<Mount>,
}
impl Detector {
    pub fn cgroup_path(&self, group: &Path) -> Option<(PathBuf, PathBuf)> {
        if !group.is_absolute()
            || group
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return None;
        }
        self.mounts
            .iter()
            .filter(|m| m.filesystem == "cgroup2")
            .find_map(|m| {
                let relative = group.strip_prefix(&m.root).ok().or_else(|| {
                    // Cgroup namespaces may expose paths relative to the mounted subtree.
                    let relative = group.strip_prefix("/").ok()?;
                    m.point.join(relative).is_dir().then_some(relative)
                })?;
                Some((m.point.join(relative), m.point.clone()))
            })
    }
    pub fn load() -> Self {
        Self {
            mounts: parse_mounts(&fs::read("/proc/self/mountinfo").unwrap_or_default()),
        }
    }
    pub fn detect(&self, path: &Path, dev: u64) -> Storage {
        self.detect_with_sysfs(path, dev, Path::new("/sys"))
    }
    fn detect_with_sysfs(&self, path: &Path, dev: u64, sysfs: &Path) -> Storage {
        let mount = self
            .mounts
            .iter()
            .filter(|m| path.starts_with(&m.point))
            .max_by_key(|m| m.point.as_os_str().len());
        let mut result = Storage {
            filesystem: "unknown".into(),
            device: "unknown".into(),
            kind: "unknown",
            backing: BTreeSet::new(),
            complete: false,
            dev,
        };
        let Some(mount) = mount else {
            return result;
        };
        // Btrfs subvolumes report an anonymous st_dev distinct from the superblock.
        let subvolume_matches =
            mount.filesystem == "btrfs" && fs::metadata(&mount.point).is_ok_and(|m| m.dev() == dev);
        if mount.dev != device_id(dev) && !subvolume_matches {
            return result;
        }
        result.filesystem.clone_from(&mount.filesystem);
        result.device = mount.source.to_string_lossy().into_owned();
        if matches!(result.filesystem.as_str(), "tmpfs" | "ramfs") {
            result.kind = "memory";
            result.complete = true;
            return result;
        }
        let mount_block = sysfs.join("dev/block").join(&mount.dev);
        let source = if mount.filesystem == "btrfs" || !mount_block.exists() {
            source_block(sysfs, &mount.source)
        } else {
            None
        };
        let block = if mount.filesystem != "btrfs" && mount_block.exists() {
            Some(mount_block)
        } else {
            source.clone()
        };
        let mut kinds = BTreeSet::new();
        if mount.filesystem == "btrfs" {
            if let Some((members, membership_complete)) = source
                .as_deref()
                .and_then(|source| btrfs_members(sysfs, source))
            {
                result.complete = membership_complete;
                if !membership_complete {
                    kinds.insert("unknown");
                }
                for member in members {
                    result.complete &= topology(
                        &member,
                        &mut BTreeSet::new(),
                        &mut result.backing,
                        &mut kinds,
                        sysfs,
                    );
                }
            } else if let Some(block) = block {
                topology(
                    &block,
                    &mut BTreeSet::new(),
                    &mut result.backing,
                    &mut kinds,
                    sysfs,
                );
                kinds.insert("unknown");
            }
        } else if let Some(block) = block {
            result.complete = topology(
                &block,
                &mut BTreeSet::new(),
                &mut result.backing,
                &mut kinds,
                sysfs,
            );
        }
        result.kind = if kinds.contains("hdd") {
            "hdd"
        } else if !kinds.is_empty() && !kinds.contains("unknown") {
            if kinds.len() == 1 && kinds.contains("nvme") {
                "nvme"
            } else {
                "ssd"
            }
        } else {
            "unknown"
        };
        result
    }
}

fn source_block(sysfs: &Path, source: &Path) -> Option<PathBuf> {
    if !source.starts_with("/dev") {
        return None;
    }
    if let Ok(metadata) = fs::metadata(source) {
        if metadata.file_type().is_block_device() {
            let path = sysfs.join("dev/block").join(device_id(metadata.rdev()));
            if path.exists() {
                return Some(path);
            }
        }
    }
    let name = source.file_name()?;
    if source.parent() == Some(Path::new("/dev")) {
        let path = sysfs.join("class/block").join(name);
        return path.exists().then_some(path);
    }
    if source.parent() == Some(Path::new("/dev/mapper")) {
        for entry in fs::read_dir(sysfs.join("class/block")).ok()?.flatten() {
            let path = entry.path();
            if fs::read_to_string(path.join("dm/name"))
                .is_ok_and(|alias| alias.trim_end() == name.to_string_lossy())
            {
                return Some(path);
            }
        }
    }
    None
}

fn btrfs_members(sysfs: &Path, source: &Path) -> Option<(Vec<PathBuf>, bool)> {
    let source = fs::canonicalize(source).ok()?;
    for filesystem in fs::read_dir(sysfs.join("fs/btrfs")).ok()?.flatten() {
        let filesystem = filesystem.path();
        let Ok(entries) = fs::read_dir(filesystem.join("devices")) else {
            continue;
        };
        let mut members = Vec::new();
        let mut complete = true;
        let mut matches_source = false;
        for entry in entries {
            match entry {
                Ok(entry) => {
                    matches_source |= fs::canonicalize(entry.path()).is_ok_and(|p| p == source);
                    members.push(entry.path());
                }
                Err(_) => complete = false,
            }
        }
        if !matches_source {
            continue;
        }
        let mut devinfo_count = 0;
        match fs::read_dir(filesystem.join("devinfo")) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => {
                            devinfo_count += 1;
                            complete &= fs::read_to_string(entry.path().join("missing"))
                                .is_ok_and(|flag| flag.trim() == "0");
                        }
                        Err(_) => complete = false,
                    }
                }
            }
            Err(_) => complete = false,
        }
        complete &= !members.is_empty() && devinfo_count == members.len();
        return Some((members, complete));
    }
    None
}

fn device_id(dev: u64) -> String {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xfffff000);
    let minor = (dev & 0xff) | ((dev >> 12) & 0xffffff00);
    format!("{major}:{minor}")
}

fn topology(
    path: &Path,
    active: &mut BTreeSet<PathBuf>,
    backing: &mut BTreeSet<String>,
    kinds: &mut BTreeSet<&'static str>,
    sysfs: &Path,
) -> bool {
    let Ok(mut path) = fs::canonicalize(path) else {
        kinds.insert("unknown");
        return false;
    };
    if path.join("partition").exists() {
        let Some(parent) = path.parent() else {
            return false;
        };
        path = parent.to_path_buf();
    }
    if active.contains(&path) {
        kinds.insert("unknown");
        return false;
    }
    if active.len() >= 128 {
        kinds.insert("unknown");
        return false;
    }
    active.insert(path.clone());
    let mut complete = true;
    let slaves = match fs::read_dir(path.join("slaves")) {
        Ok(entries) => {
            let mut slaves = Vec::new();
            for entry in entries {
                match entry {
                    Ok(entry) => slaves.push(entry.path()),
                    Err(_) => {
                        complete = false;
                        kinds.insert("unknown");
                    }
                }
            }
            slaves
        }
        Err(_) => {
            kinds.insert("unknown");
            active.remove(&path);
            return false;
        }
    };
    if !slaves.is_empty() {
        for slave in slaves {
            complete &= topology(&slave, active, backing, kinds, sysfs);
        }
        active.remove(&path);
        return complete;
    }
    active.remove(&path);
    if path.starts_with(sysfs.join("devices/virtual/block")) {
        kinds.insert("unknown");
        return false;
    }
    let Ok(id) = fs::read_to_string(path.join("dev")) else {
        kinds.insert("unknown");
        return false;
    };
    backing.insert(id.trim().into());
    let rotational = fs::read_to_string(path.join("queue/rotational")).unwrap_or_default();
    let kind = match rotational.trim() {
        "1" => "hdd",
        "0" if path
            .file_name()
            .is_some_and(|n| n.as_encoded_bytes().starts_with(b"nvme")) =>
        {
            "nvme"
        }
        "0" => "ssd",
        _ => "unknown",
    };
    kinds.insert(kind);
    complete && kind != "unknown"
}

fn unescape(bytes: &[u8]) -> Option<OsString> {
    let mut decoded = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            let code = bytes.get(i + 1..i + 4)?;
            let byte = match code {
                b"040" => b' ',
                b"011" => b'\t',
                b"012" => b'\n',
                b"134" => b'\\',
                _ => return None,
            };
            decoded.push(byte);
            i += 4;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    Some(OsString::from_vec(decoded))
}
fn parse_mounts(bytes: &[u8]) -> Vec<Mount> {
    bytes
        .split(|b| *b == b'\n')
        .filter_map(|line| {
            let fields: Vec<_> = line.split(|b| *b == b' ').collect();
            let separator = fields.iter().position(|s| *s == b"-")?;
            if separator < 6 {
                return None;
            }
            let point = PathBuf::from(unescape(fields[4])?);
            if !point.is_absolute() {
                return None;
            }
            Some(Mount {
                root: PathBuf::from(unescape(fields[3])?),
                point,
                dev: std::str::from_utf8(fields[2]).ok()?.into(),
                filesystem: std::str::from_utf8(fields.get(separator + 1)?).ok()?.into(),
                source: PathBuf::from(unescape(fields.get(separator + 2)?)?),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mount_escapes_longest_match_and_unknown_are_conservative() {
        let detector = Detector { mounts: parse_mounts(b"1 0 0:5 / / rw - tmpfs tmpfs rw\n2 1 0:6 / /space\\040dir rw shared:3 - ext4 /dev/absent rw\ninvalid\n3 1 0:7 / /bad\\777 rw - ext4 /dev/x rw\n") };
        assert_eq!(detector.mounts.len(), 2);
        let storage = detector.detect(Path::new("/space dir/a"), 6);
        assert_eq!(storage.filesystem, "ext4");
        assert_eq!(storage.kind, "unknown");
        assert!(storage.chunks_allowed());
        let mut gated = storage.clone();
        gated.kind = "hdd";
        assert!(!gated.chunks_allowed());
        gated.kind = "unknown";
        gated.filesystem = "nfs".into();
        assert!(!gated.chunks_allowed());
        assert_eq!(
            detector.detect(Path::new("/space directory"), 5).kind,
            "memory"
        );
        assert_eq!(
            detector.detect(Path::new("/space dir/a"), 99).filesystem,
            "unknown"
        );
        assert_eq!(
            storage.shared_with(&detector.detect(Path::new("/other"), 5)),
            None
        );
        assert_eq!(
            unescape(b"x\\134y\\011\\012\xff").unwrap().into_vec(),
            b"x\\y\t\n\xff"
        );
    }
    #[test]
    fn cgroup_mount_roots_map_process_paths_without_parent_traversal() {
        let detector = Detector {
            mounts: parse_mounts(
                b"1 0 0:5 /user.slice /control\\040groups rw - cgroup2 cgroup rw\n",
            ),
        };
        assert_eq!(
            detector.cgroup_path(Path::new("/user.slice/session")),
            Some(("/control groups/session".into(), "/control groups".into()))
        );
        assert_eq!(
            detector.cgroup_path(Path::new("/user.slice/../escape")),
            None
        );
        assert_eq!(detector.cgroup_path(Path::new("relative")), None);
    }
    #[test]
    fn topology_resolves_partition_mapper_and_rotational_leaf() {
        use std::os::unix::fs::symlink;
        let fixture = SysfsFixture::new();
        let root = &fixture.0;
        let detector = Detector {
            mounts: vec![Mount {
                root: "/".into(),
                point: root.to_path_buf(),
                dev: "999:999".into(),
                filesystem: "btrfs".into(),
                source: "/dev/absent".into(),
            }],
        };
        let storage = detector.detect(root, fs::metadata(root).unwrap().dev());
        assert_eq!(storage.filesystem, "btrfs");
        assert!(storage.chunks_allowed());
        assert!(!storage.complete);
        let disk = root.join("nvme0n1");
        fs::create_dir_all(disk.join("queue")).unwrap();
        fs::write(disk.join("dev"), "259:0\n").unwrap();
        fs::write(disk.join("queue/rotational"), "0\n").unwrap();
        fs::create_dir(disk.join("slaves")).unwrap();
        fs::create_dir(disk.join("nvme0n1p1")).unwrap();
        fs::write(disk.join("nvme0n1p1/partition"), "1").unwrap();
        fs::create_dir_all(root.join("dm-0/slaves")).unwrap();
        symlink(disk.join("nvme0n1p1"), root.join("dm-0/slaves/partition")).unwrap();
        let mut backing = BTreeSet::new();
        let mut kinds = BTreeSet::new();
        assert!(topology(
            &root.join("dm-0"),
            &mut BTreeSet::new(),
            &mut backing,
            &mut kinds,
            root
        ));
        assert_eq!(backing, BTreeSet::from(["259:0".into()]));
        assert_eq!(kinds, BTreeSet::from(["nvme"]));
        fs::write(disk.join("queue/rotational"), "1").unwrap();
        kinds.clear();
        assert!(topology(
            &disk,
            &mut BTreeSet::new(),
            &mut backing,
            &mut kinds,
            root
        ));
        assert!(kinds.contains("hdd"));
    }

    #[test]
    fn convergent_slaves_do_not_make_topology_incomplete() {
        use std::os::unix::fs::symlink;
        let fixture = SysfsFixture::new();
        let root = &fixture.0;
        let disk = root.join("nvme0n1");
        fs::create_dir_all(disk.join("queue")).unwrap();
        fs::create_dir(disk.join("slaves")).unwrap();
        fs::write(disk.join("dev"), "259:0").unwrap();
        fs::write(disk.join("queue/rotational"), "0").unwrap();
        for part in ["nvme0n1p1", "nvme0n1p2"] {
            fs::create_dir(disk.join(part)).unwrap();
            fs::write(disk.join(part).join("partition"), "1").unwrap();
        }
        fs::create_dir_all(root.join("dm-0/slaves")).unwrap();
        symlink(disk.join("nvme0n1p1"), root.join("dm-0/slaves/one")).unwrap();
        symlink(disk.join("nvme0n1p2"), root.join("dm-0/slaves/two")).unwrap();
        let mut backing = BTreeSet::new();
        let mut kinds = BTreeSet::new();
        assert!(topology(
            &root.join("dm-0"),
            &mut BTreeSet::new(),
            &mut backing,
            &mut kinds,
            root
        ));
        assert_eq!(backing, BTreeSet::from(["259:0".into()]));
        assert_eq!(kinds, BTreeSet::from(["nvme"]));
    }

    struct SysfsFixture(PathBuf);
    impl SysfsFixture {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "tcopy-sysfs-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn disk(&self, name: &str, id: &str, rotational: &str) -> PathBuf {
            use std::os::unix::fs::symlink;
            let disk = self.0.join("devices/pci").join(name);
            fs::create_dir_all(disk.join("queue")).unwrap();
            fs::create_dir(disk.join("slaves")).unwrap();
            fs::write(disk.join("dev"), id).unwrap();
            fs::write(disk.join("queue/rotational"), rotational).unwrap();
            fs::create_dir_all(self.0.join("class/block")).unwrap();
            symlink(&disk, self.0.join("class/block").join(name)).unwrap();
            disk
        }
        fn partition(&self, disk: &Path, name: &str) {
            use std::os::unix::fs::symlink;
            let partition = disk.join(name);
            fs::create_dir(&partition).unwrap();
            fs::write(partition.join("partition"), "1").unwrap();
            symlink(&partition, self.0.join("class/block").join(name)).unwrap();
        }
        fn mapper(&self, name: &str, alias: &str, slaves: &[&Path]) -> PathBuf {
            use std::os::unix::fs::symlink;
            let device = self.0.join("devices/virtual/block").join(name);
            fs::create_dir_all(device.join("dm")).unwrap();
            fs::create_dir(device.join("slaves")).unwrap();
            fs::write(device.join("dm/name"), alias).unwrap();
            symlink(&device, self.0.join("class/block").join(name)).unwrap();
            for (i, slave) in slaves.iter().enumerate() {
                symlink(slave, device.join("slaves").join(i.to_string())).unwrap();
            }
            device
        }
        fn detector(&self, mounts: &[(&str, &str, &str)]) -> Detector {
            let mounts = mounts
                .iter()
                .enumerate()
                .map(|(index, (point, filesystem, source))| Mount {
                    root: "/".into(),
                    point: (*point).into(),
                    dev: format!("0:{}", index + 1),
                    filesystem: (*filesystem).into(),
                    source: (*source).into(),
                })
                .collect();
            Detector { mounts }
        }
    }
    impl Drop for SysfsFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn mapper_alias_nested_slaves_and_partitions_identify_physical_disk() {
        let fixture = SysfsFixture::new();
        let nvme = fixture.disk("nvme0n1", "259:0", "0");
        fixture.partition(&nvme, "nvme0n1p1");
        fixture.partition(&nvme, "nvme0n1p2");
        fixture.mapper("dm-0", "crypt-root", &[&nvme.join("nvme0n1p1")]);
        fixture.mapper(
            "dm-1",
            "nested-root",
            &[&fixture.0.join("class/block/dm-0")],
        );
        let other = fixture.disk("nvme1n1", "259:1", "0");
        fixture.partition(&other, "nvme1n1p1");
        let detector = fixture.detector(&[
            ("/source", "ext4", "/dev/mapper/nested-root"),
            ("/same", "ext4", "/dev/nvme0n1p2"),
            ("/other", "ext4", "/dev/nvme1n1p1"),
        ]);
        let source = detector.detect_with_sysfs(Path::new("/source/file"), 1, &fixture.0);
        let same = detector.detect_with_sysfs(Path::new("/same/file"), 2, &fixture.0);
        let other = detector.detect_with_sysfs(Path::new("/other/file"), 3, &fixture.0);
        assert_eq!(source.kind, "nvme");
        assert_eq!(source.backing, BTreeSet::from(["259:0".into()]));
        assert!(source.complete && same.complete && other.complete);
        assert_eq!(source.shared_with(&same), Some(true));
        assert_eq!(source.shared_with(&other), Some(false));
    }

    #[test]
    fn mapper_alias_does_not_collide_with_native_block_name() {
        let fixture = SysfsFixture::new();
        fixture.disk("nvme0n1", "259:0", "0");
        let hdd = fixture.disk("sda", "8:0", "1");
        fixture.mapper("dm-0", "nvme0n1", &[&hdd]);
        let detector = fixture.detector(&[("/source", "ext4", "/dev/mapper/nvme0n1")]);
        let storage = detector.detect_with_sysfs(Path::new("/source/file"), 1, &fixture.0);
        assert_eq!(storage.kind, "hdd");
        assert_eq!(storage.backing, BTreeSet::from(["8:0".into()]));
    }

    #[test]
    fn mixed_backing_and_unavailable_topology_are_conservative() {
        let fixture = SysfsFixture::new();
        let ssd = fixture.disk("nvme0n1", "259:0", "0");
        let hdd = fixture.disk("sda", "8:0", "1");
        let mixed = fixture.mapper("dm-0", "mixed", &[&ssd, &hdd]);
        let mut backing = BTreeSet::new();
        let mut kinds = BTreeSet::new();
        assert!(topology(
            &mixed,
            &mut BTreeSet::new(),
            &mut backing,
            &mut kinds,
            &fixture.0
        ));
        assert_eq!(backing, BTreeSet::from(["259:0".into(), "8:0".into()]));
        assert!(kinds.contains("hdd"));

        fs::remove_dir(ssd.join("slaves")).unwrap();
        kinds.clear();
        assert!(!topology(
            &ssd,
            &mut BTreeSet::new(),
            &mut backing,
            &mut kinds,
            &fixture.0
        ));
        assert!(kinds.contains("unknown"));
        fs::create_dir(ssd.join("slaves")).unwrap();
        fs::remove_file(ssd.join("queue/rotational")).unwrap();
        kinds.clear();
        assert!(!topology(
            &ssd,
            &mut BTreeSet::new(),
            &mut backing,
            &mut kinds,
            &fixture.0
        ));
        assert!(kinds.contains("unknown"));
        let virtual_disk = fixture.mapper("dm-1", "empty", &[]);
        kinds.clear();
        assert!(!topology(
            &virtual_disk,
            &mut BTreeSet::new(),
            &mut backing,
            &mut kinds,
            &fixture.0
        ));
        assert!(kinds.contains("unknown"));
        use std::os::unix::fs::symlink;
        symlink(&virtual_disk, virtual_disk.join("slaves/cycle")).unwrap();
        kinds.clear();
        assert!(!topology(
            &virtual_disk,
            &mut BTreeSet::new(),
            &mut backing,
            &mut kinds,
            &fixture.0
        ));
        assert!(kinds.contains("unknown"));
    }

    #[test]
    fn btrfs_membership_must_be_complete_to_prove_independence() {
        use std::os::unix::fs::symlink;
        let fixture = SysfsFixture::new();
        let ssd = fixture.disk("nvme0n1", "259:0", "0");
        let hdd = fixture.disk("sda", "8:0", "1");
        let mapper = fixture.mapper("dm-0", "crypt-home", &[&ssd]);
        let filesystem = fixture.0.join("fs/btrfs/uuid");
        fs::create_dir_all(filesystem.join("devices")).unwrap();
        symlink(&mapper, filesystem.join("devices/dm-0")).unwrap();
        fs::create_dir_all(filesystem.join("devinfo/1")).unwrap();
        fs::write(filesystem.join("devinfo/1/missing"), "0").unwrap();
        let detector = fixture.detector(&[
            ("/btrfs", "btrfs", "/dev/mapper/crypt-home"),
            ("/other", "ext4", "/dev/sda"),
        ]);
        let btrfs = || detector.detect_with_sysfs(Path::new("/btrfs/file"), 1, &fixture.0);
        let other = detector.detect_with_sysfs(Path::new("/other/file"), 2, &fixture.0);
        assert_eq!(btrfs().kind, "nvme");
        assert!(btrfs().complete);
        assert_eq!(btrfs().shared_with(&other), Some(false));

        symlink(&hdd, filesystem.join("devices/sda")).unwrap();
        fs::create_dir(filesystem.join("devinfo/2")).unwrap();
        fs::write(filesystem.join("devinfo/2/missing"), "0").unwrap();
        assert_eq!(btrfs().kind, "hdd");
        assert_eq!(
            btrfs().backing,
            BTreeSet::from(["259:0".into(), "8:0".into()])
        );
        assert_eq!(btrfs().shared_with(&other), Some(true));

        fs::write(filesystem.join("devinfo/2/missing"), "1").unwrap();
        assert!(!btrfs().complete);
        fs::remove_file(filesystem.join("devices/sda")).unwrap();
        assert_eq!(btrfs().kind, "unknown");
        assert_eq!(btrfs().shared_with(&other), None);
    }

    #[test]
    fn unknown_network_and_unlisted_btrfs_are_not_independent() {
        let fixture = SysfsFixture::new();
        fixture.disk("nvme0n1", "259:0", "0");
        let detector = fixture.detector(&[
            ("/network", "nfs", "server:/share"),
            ("/btrfs", "btrfs", "/dev/nvme0n1"),
            ("/local", "ext4", "/dev/nvme0n1"),
        ]);
        let network = detector.detect_with_sysfs(Path::new("/network/file"), 1, &fixture.0);
        let btrfs = detector.detect_with_sysfs(Path::new("/btrfs/file"), 2, &fixture.0);
        let local = detector.detect_with_sysfs(Path::new("/local/file"), 3, &fixture.0);
        assert_eq!(network.kind, "unknown");
        assert!(!network.complete);
        assert_eq!(network.shared_with(&local), None);
        assert_eq!(btrfs.backing, BTreeSet::from(["259:0".into()]));
        assert_eq!(btrfs.kind, "unknown");
        assert!(!btrfs.complete);
        assert_eq!(btrfs.shared_with(&local), Some(true));
    }
}
