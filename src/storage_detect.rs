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
        let mut block = Path::new("/sys/dev/block").join(&mount.dev);
        if !block.exists() {
            if let Ok(metadata) = fs::metadata(&mount.source) {
                if metadata.file_type().is_block_device() {
                    block = Path::new("/sys/dev/block").join(device_id(metadata.rdev()));
                }
            }
        }
        let mut visited = BTreeSet::new();
        let mut kinds = BTreeSet::new();
        result.complete = topology(&block, &mut visited, &mut result.backing, &mut kinds);
        // shortcut: a Btrfs mount source may expose only one member, keep independence unknown until multi-device discovery is added.
        if result.filesystem == "btrfs" {
            result.complete = false;
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

fn device_id(dev: u64) -> String {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xfffff000);
    let minor = (dev & 0xff) | ((dev >> 12) & 0xffffff00);
    format!("{major}:{minor}")
}

fn topology(
    path: &Path,
    visited: &mut BTreeSet<PathBuf>,
    backing: &mut BTreeSet<String>,
    kinds: &mut BTreeSet<&'static str>,
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
    if visited.contains(&path) {
        kinds.insert("unknown");
        return false;
    }
    if visited.len() >= 128 {
        kinds.insert("unknown");
        return false;
    }
    visited.insert(path.clone());
    let mut complete = true;
    let slaves = match fs::read_dir(path.join("slaves")) {
        Ok(entries) => {
            let mut slaves = Vec::new();
            for entry in entries {
                match entry {
                    Ok(entry) => slaves.push(entry.path()),
                    Err(_) => complete = false,
                }
            }
            slaves
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => {
            kinds.insert("unknown");
            return false;
        }
    };
    if !slaves.is_empty() {
        for slave in slaves {
            complete &= topology(&slave, visited, backing, kinds);
        }
        return complete;
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
        let root = std::env::temp_dir().join(format!("tcopy-topology-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let detector = Detector {
            mounts: vec![Mount {
                root: "/".into(),
                point: root.clone(),
                dev: "999:999".into(),
                filesystem: "btrfs".into(),
                source: "/dev/absent".into(),
            }],
        };
        let storage = detector.detect(&root, fs::metadata(&root).unwrap().dev());
        assert_eq!(storage.filesystem, "btrfs");
        assert!(storage.chunks_allowed());
        assert!(!storage.complete);
        let disk = root.join("nvme0n1");
        fs::create_dir_all(disk.join("queue")).unwrap();
        fs::write(disk.join("dev"), "259:0\n").unwrap();
        fs::write(disk.join("queue/rotational"), "0\n").unwrap();
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
            &mut kinds
        ));
        assert_eq!(backing, BTreeSet::from(["259:0".into()]));
        assert_eq!(kinds, BTreeSet::from(["nvme"]));
        fs::write(disk.join("queue/rotational"), "1").unwrap();
        kinds.clear();
        assert!(topology(
            &disk,
            &mut BTreeSet::new(),
            &mut backing,
            &mut kinds
        ));
        assert!(kinds.contains("hdd"));
        fs::remove_dir_all(root).unwrap();
    }
}
