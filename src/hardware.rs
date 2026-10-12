use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Hardware {
    pub logical_cpus: usize,
    pub physical_cores: Option<usize>,
    pub memory_bytes: Option<u64>,
    pub cpu_pressure: Option<f64>,
    pub io_pressure: Option<f64>,
}

impl Hardware {
    pub fn load() -> Self {
        let mut result = Self {
            logical_cpus: std::thread::available_parallelism().map_or(1, usize::from),
            physical_cores: physical_cores(&read("/proc/cpuinfo")),
            memory_bytes: memory_available(&read("/proc/meminfo")),
            cpu_pressure: pressure(&read("/proc/pressure/cpu")),
            io_pressure: pressure(&read("/proc/pressure/io")),
        };
        if let Some((current, root)) = cgroup() {
            result.cgroup_limits(&current, &root);
        }
        result
    }
    fn cgroup_limits(&mut self, current: &Path, root: &Path) {
        for path in current.ancestors().take_while(|p| p.starts_with(root)) {
            if let Some(quota) = cpu_quota(&read(path.join("cpu.max"))) {
                self.logical_cpus = self.logical_cpus.min(quota);
            }
            if let (Ok(limit), Ok(used)) = (
                read(path.join("memory.max")).trim().parse::<u64>(),
                read(path.join("memory.current")).trim().parse::<u64>(),
            ) {
                let remaining = limit.saturating_sub(used);
                self.memory_bytes = Some(self.memory_bytes.map_or(remaining, |n| n.min(remaining)));
            }
        }
    }
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn cgroup() -> Option<(PathBuf, PathBuf)> {
    // shortcut: inspect visible cgroup v2 limits, add v1 parsing for legacy deployments.
    let group = read("/proc/self/cgroup")
        .lines()
        .find_map(|line| line.strip_prefix("0::").map(str::to_owned))?;
    crate::storage_detect::Detector::load().cgroup_path(Path::new(&group))
}

fn physical_cores(text: &str) -> Option<usize> {
    let cores: BTreeSet<_> = text
        .split("\n\n")
        .filter_map(|block| {
            let field = |name| {
                block
                    .lines()
                    .filter_map(|l| l.split_once(':'))
                    .find(|(key, _)| key.trim() == name)
                    .map(|(_, value)| value.trim())
            };
            Some((field("physical id")?, field("core id")?))
        })
        .collect();
    (!cores.is_empty()).then_some(cores.len())
}

fn memory_available(text: &str) -> Option<u64> {
    let mut fields = text
        .lines()
        .find(|l| l.starts_with("MemAvailable:"))?
        .split_whitespace();
    fields.next()?;
    let value = fields.next()?.parse::<u64>().ok()?;
    (fields.next()? == "kB")
        .then(|| value.checked_mul(1024))
        .flatten()
}

fn cpu_quota(text: &str) -> Option<usize> {
    let mut fields = text.split_whitespace();
    let quota = fields.next()?.parse::<u64>().ok()?;
    let period = fields.next()?.parse::<u64>().ok()?;
    if quota == 0 || period == 0 {
        return None;
    }
    usize::try_from(quota / period + u64::from(quota % period != 0)).ok()
}

fn pressure(text: &str) -> Option<f64> {
    text.lines()
        .find(|l| l.starts_with("some "))?
        .split_whitespace()
        .find_map(|v| v.strip_prefix("avg10="))?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_cpu_memory_and_pressure_without_inventing_missing_values() {
        assert_eq!(physical_cores("physical id : 0\ncore id : 0\n\nphysical id : 0\ncore id : 0\n\nphysical id : 0\ncore id : 1"), Some(2));
        assert_eq!(physical_cores("processor : 0"), None);
        assert_eq!(
            memory_available("MemTotal: 999 kB\nMemAvailable: 123 kB\n"),
            Some(123 * 1024)
        );
        assert_eq!(
            memory_available("MemAvailable: 18446744073709551615 kB"),
            None
        );
        assert_eq!(cpu_quota("150000 100000"), Some(2));
        assert_eq!(cpu_quota("max 100000"), None);
        assert_eq!(cpu_quota("10 0"), None);
        assert_eq!(
            pressure("some avg10=12.34 avg60=1.0 total=123"),
            Some(12.34)
        );
        for bad in ["", "some avg10=NaN", "some avg10=101", "some avg10=-1"] {
            assert_eq!(pressure(bad), None);
        }
    }
    #[test]
    fn ancestor_cgroup_limits_use_remaining_memory_and_smallest_quota() {
        let root = std::env::temp_dir().join(format!("tcopy-cgroup-{}", std::process::id()));
        fs::create_dir_all(root.join("child")).unwrap();
        fs::write(root.join("memory.max"), "1000").unwrap();
        fs::write(root.join("memory.current"), "700").unwrap();
        fs::write(root.join("cpu.max"), "100000 100000").unwrap();
        fs::write(root.join("child/memory.max"), "max").unwrap();
        fs::write(root.join("child/cpu.max"), "400000 100000").unwrap();
        let mut hardware = Hardware {
            logical_cpus: 8,
            physical_cores: Some(4),
            memory_bytes: Some(800),
            cpu_pressure: None,
            io_pressure: None,
        };
        hardware.cgroup_limits(&root.join("child"), &root);
        assert_eq!(hardware.logical_cpus, 1);
        assert_eq!(hardware.memory_bytes, Some(300));
        fs::write(root.join("memory.current"), "1200").unwrap();
        hardware.cgroup_limits(&root.join("child"), &root);
        assert_eq!(hardware.memory_bytes, Some(0));
        fs::remove_dir_all(root).unwrap();
    }
}
