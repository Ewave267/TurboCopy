use crate::{cli::Options, hardware::Hardware, progress::StoragePair, storage_detect::Storage};

fn fast(storage: &Storage) -> bool {
    storage.chunks_allowed() && matches!(storage.kind, "ssd" | "nvme" | "memory")
}

pub fn metadata_parallelism(options: Options, pairs: &[StoragePair]) -> bool {
    options.auto
        && !options.sync
        && !options.resume
        && !options.no_fsync
        && !options.dry_run
        && !options.manual_workers
        && !options.gpfs
        && !pairs.is_empty()
        && pairs.iter().all(|p| {
            p.source.kind == "nvme"
                && p.destination.kind == "nvme"
                && p.source.complete
                && p.destination.complete
                && p.source.chunks_allowed()
                && p.destination.chunks_allowed()
                && p.source.filesystem != "gpfs"
                && p.destination.filesystem != "gpfs"
                && p.source.shared_with(&p.destination) == Some(true)
        })
}

pub fn select(
    mut options: Options,
    hardware: &Hardware,
    pairs: &[StoragePair],
    many_small_files: bool,
) -> Options {
    let overlap_metadata = many_small_files && metadata_parallelism(options, pairs);
    options.gpfs |= options.auto
        && pairs
            .iter()
            .any(|p| p.source.filesystem == "gpfs" || p.destination.filesystem == "gpfs");
    if options.gpfs {
        if !options.manual_workers {
            // Remote I/O waits can overlap beyond the local CPU count.
            let memory_limit = hardware.memory_bytes.map_or(1, |n| {
                (n / 64 / (2 * options.buffer_size() as u64)).clamp(1, 32) as usize
            });
            options.workers = Some(memory_limit);
        }
        if !options.manual_chunks {
            options.chunk_size = Some(16 << 20);
        }
    }
    if options.auto && !options.gpfs {
        if !options.manual_workers {
            let storage_limit = pairs
                .iter()
                .map(|p| {
                    if overlap_metadata {
                        16
                    } else if p.source.kind == "hdd" || p.destination.kind == "hdd" {
                        if p.source.shared_with(&p.destination) != Some(false) {
                            1
                        } else {
                            2
                        }
                    } else if fast(&p.source) && fast(&p.destination) {
                        if p.source.shared_with(&p.destination) == Some(false) {
                            8
                        } else {
                            4
                        }
                    } else {
                        2
                    }
                })
                .min()
                .unwrap_or(2);
            // Reserve all file/helper buffers from at most 1/64 of available memory.
            let memory_limit = hardware
                .memory_bytes
                .map_or(2, |n| (n / 64 / (256 * 1024)).clamp(1, 128) as usize);
            // Small durable files wait on metadata I/O rather than occupying a CPU.
            let cpus = if overlap_metadata {
                hardware.logical_cpus.max(1).saturating_mul(2)
            } else {
                hardware
                    .physical_cores
                    .unwrap_or(hardware.logical_cpus)
                    .min(hardware.logical_cpus)
                    .max(1)
            };
            let limit = storage_limit.min(memory_limit).min(cpus);
            options.workers = Some(limit);
        }
        if !options.manual_chunks {
            options.chunk_size = pairs
                .iter()
                .any(|p| {
                    fast(&p.source)
                        && fast(&p.destination)
                        && p.source.shared_with(&p.destination) == Some(false)
                })
                .then_some(8 << 20);
        }
    }
    options.workers = Some(options.worker_count());
    options
}

pub fn chunk_size(
    options: Options,
    source: &Storage,
    destination: &Storage,
    length: u64,
    workers: usize,
) -> Option<u64> {
    if !source.chunks_allowed() || !destination.chunks_allowed() {
        return None;
    }
    if options.gpfs || !options.auto || options.manual_chunks {
        return options.chunk_size;
    }
    if length < 256 << 20
        || !fast(source)
        || !fast(destination)
        || source.shared_with(destination) != Some(false)
    {
        return None;
    }
    let mib = 1 << 20;
    Some((length / workers.max(1) as u64 / 8 / mib).clamp(4, 16) * mib)
}

pub struct Controller {
    pub limit: usize,
    pub state: &'static str,
    maximum: usize,
    best_limit: usize,
    pub best_rate: f64,
    pub samples: usize,
    windows: usize,
    calibrating: bool,
}
impl Controller {
    pub fn new(maximum: usize, tune: bool) -> Self {
        let limit = if tune { 1 } else { maximum };
        Self {
            limit,
            state: if tune { "insufficient-work" } else { "fixed" },
            maximum,
            best_limit: limit,
            best_rate: 0.0,
            samples: 0,
            windows: 0,
            calibrating: tune,
        }
    }
    pub fn sample(&mut self, rate: f64, saturated: bool) {
        self.windows += 1;
        if self.calibrating {
            if rate > 0.0 && saturated {
                self.samples += 1;
                if self.best_rate > 0.0 && rate < self.best_rate * 1.05 {
                    self.limit = self.best_limit;
                    self.calibrating = false;
                    self.state = "plateau";
                    return;
                }
                self.best_rate = rate;
                self.best_limit = self.limit;
                self.state = "calibrating";
                if self.limit < self.maximum && self.windows < 8 {
                    self.limit = (self.limit * 2).min(self.maximum);
                } else {
                    self.calibrating = false;
                    self.state = "complete";
                }
            }
            if self.windows >= 8 {
                self.limit = self.best_limit;
                self.calibrating = false;
                self.state = if self.best_rate > 0.0 {
                    "complete"
                } else {
                    "insufficient-work"
                };
            }
        }
    }

    fn finish(&mut self) {
        if self.calibrating {
            self.limit = self.best_limit;
            self.state = if self.samples == 0 {
                "insufficient-work"
            } else {
                "short-workload"
            };
        }
    }
}

pub fn monitor(
    summary: &crate::progress::Summary,
    budget: &crate::scheduler::Concurrency,
    mut controller: Controller,
    adaptive: bool,
    stop: std::sync::mpsc::Receiver<()>,
) {
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    let mut last = Instant::now();
    let mut bytes = summary.transferred.load(Ordering::Relaxed);
    let mut files = summary.copied.load(Ordering::Relaxed);
    let mut busy = summary.busy_ns.load(Ordering::Relaxed);
    while matches!(
        stop.recv_timeout(Duration::from_millis(250)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ) {
        if summary.options.progress {
            summary.progress();
        }
        if !adaptive || last.elapsed() < Duration::from_secs(1) {
            continue;
        }
        let seconds = last.elapsed().as_secs_f64();
        let next_bytes = summary.transferred.load(Ordering::Relaxed);
        let next_files = summary.copied.load(Ordering::Relaxed);
        let next_busy = summary.busy_ns.load(Ordering::Relaxed);
        let active = summary.active.load(Ordering::Relaxed) as usize;
        let saturated = active >= controller.limit
            || (next_busy - busy) as f64 / 1e9 / seconds >= controller.limit as f64 * 0.7;
        let rate = if next_bytes - bytes >= 1 << 20 || next_files - files >= 8 {
            // A fixed per-file weight keeps metadata-heavy and byte-heavy windows comparable.
            ((next_bytes - bytes) as f64 + (next_files - files) as f64 * 4096.0) / seconds
        } else {
            0.0
        };
        controller.sample(rate, saturated);
        budget.resize(controller.limit);
        *summary.tuning_state.lock().unwrap() = controller.state;
        summary
            .tuning_samples
            .store(controller.samples as u64, Ordering::Relaxed);
        *summary.tuning_rate.lock().unwrap() = controller.best_rate;
        bytes = next_bytes;
        files = next_files;
        busy = next_busy;
        last = Instant::now();
    }
    if adaptive {
        controller.finish();
        budget.resize(controller.limit);
        *summary.tuning_state.lock().unwrap() = controller.state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_detects_gpfs_and_preserves_concurrency_under_reported_pressure() {
        let mut remote = storage("unknown", 1);
        remote.filesystem = "gpfs".into();
        let pair = StoragePair {
            source: remote.clone(),
            destination: remote,
            available_bytes: None,
        };
        let mut host = hardware();
        host.cpu_pressure = Some(90.0);
        host.io_pressure = Some(90.0);
        let selected = select(
            Options {
                auto: true,
                ..Options::default()
            },
            &host,
            &[pair],
            false,
        );
        assert!(selected.gpfs);
        assert_eq!(selected.worker_count(), 32);
        assert_eq!(selected.buffer_size(), 1 << 20);
        assert_eq!(
            chunk_size(
                selected,
                &storage("nvme", 1),
                &storage("nvme", 1),
                64 << 20,
                32
            ),
            Some(16 << 20)
        );
        let mut fixed = Controller::new(selected.worker_count(), false);
        for _ in 0..20 {
            fixed.sample(100.0, true);
        }
        fixed.finish();
        assert_eq!(fixed.limit, 32);
        assert_eq!(fixed.samples, 0);
    }
    #[test]
    fn gpfs_profile_overlaps_remote_io_without_assuming_local_topology() {
        let profile = Options {
            gpfs: true,
            ..Options::default()
        };
        let mut host = hardware();
        host.logical_cpus = 1;
        let selected = select(profile, &host, &[], false);
        assert_eq!(selected.worker_count(), 32);
        assert_eq!(selected.chunk_size, Some(16 << 20));
        let mut remote = storage("unknown", 1);
        remote.filesystem = "gpfs".into();
        assert_eq!(
            chunk_size(selected, &remote, &remote, 1 << 30, 32),
            Some(16 << 20)
        );
        remote.filesystem = "nfs".into();
        assert_eq!(chunk_size(selected, &remote, &remote, 1 << 30, 32), None);
        host.memory_bytes = Some(128 << 20);
        assert_eq!(select(profile, &host, &[], false).worker_count(), 1);
        let manual = Options {
            workers: Some(7),
            manual_workers: true,
            chunk_size: Some(4 << 20),
            manual_chunks: true,
            ..profile
        };
        let selected = select(manual, &host, &[], false);
        assert_eq!(selected.worker_count(), 7);
        assert_eq!(selected.chunk_size, Some(4 << 20));
    }
    fn hardware() -> Hardware {
        Hardware {
            logical_cpus: 16,
            physical_cores: Some(8),
            memory_bytes: Some(4 << 30),
            cpu_pressure: Some(0.0),
            io_pressure: Some(0.0),
        }
    }
    fn storage(kind: &'static str, dev: u64) -> Storage {
        Storage {
            kind,
            dev,
            filesystem: "ext4".into(),
            device: "test".into(),
            backing: Default::default(),
            complete: true,
        }
    }
    #[test]
    fn selects_conservative_profiles_and_honors_overrides() {
        let mut pair = StoragePair {
            source: storage("nvme", 1),
            destination: storage("nvme", 2),
            available_bytes: None,
        };
        let auto = Options {
            auto: true,
            ..Options::default()
        };
        let selected = select(auto, &hardware(), std::slice::from_ref(&pair), false);
        assert_eq!(selected.worker_count(), 8);
        assert_eq!(selected.chunk_size, Some(8 << 20));
        pair.destination.dev = 1;
        assert_eq!(
            select(auto, &hardware(), std::slice::from_ref(&pair), false).worker_count(),
            4
        );
        assert_eq!(
            chunk_size(auto, &pair.source, &pair.destination, 1 << 30, 4),
            None
        );
        pair.source.kind = "hdd";
        assert_eq!(
            select(auto, &hardware(), std::slice::from_ref(&pair), false).worker_count(),
            1
        );
        pair.source.kind = "unknown";
        assert_eq!(
            select(auto, &hardware(), std::slice::from_ref(&pair), false).worker_count(),
            2
        );
        let mut limited = hardware();
        limited.memory_bytes = Some(16 << 20);
        assert_eq!(select(auto, &limited, &[], false).worker_count(), 1);
        let manual = Options {
            workers: Some(12),
            chunk_size: Some(1024),
            manual_workers: true,
            manual_chunks: true,
            ..auto
        };
        let selected = select(manual, &limited, &[pair], false);
        assert_eq!(selected.worker_count(), 12);
        assert_eq!(selected.chunk_size, Some(1024));
        assert_eq!(
            chunk_size(auto, &storage("ssd", 1), &storage("ssd", 2), 1 << 30, 8),
            Some(16 << 20)
        );
        assert_eq!(
            chunk_size(auto, &storage("ssd", 1), &storage("ssd", 2), 64 << 20, 8),
            None
        );
        let mut network = StoragePair {
            source: storage("ssd", 1),
            destination: storage("ssd", 2),
            available_bytes: None,
        };
        network.source.filesystem = "nfs".into();
        assert_eq!(
            select(auto, &hardware(), std::slice::from_ref(&network), false).worker_count(),
            2
        );
        assert_eq!(
            chunk_size(manual, &network.source, &network.destination, 1 << 30, 12),
            None
        );
        let mut single_cpu = hardware();
        single_cpu.logical_cpus = 1;
        assert_eq!(select(auto, &single_cpu, &[], false).worker_count(), 1);
        let ordinary = select(Options::default(), &hardware(), &[], false);
        assert!(ordinary.worker_count() <= 4);
        assert_eq!(ordinary.chunk_size, None);
    }
    #[test]
    fn many_small_files_overlap_nvme_waits_with_bounded_workers() {
        let auto = Options {
            auto: true,
            ..Options::default()
        };
        let pair = StoragePair {
            source: storage("nvme", 1),
            destination: storage("nvme", 1),
            available_bytes: None,
        };
        let mut host = hardware();
        host.physical_cores = Some(4);
        host.logical_cpus = 8;
        assert_eq!(
            select(auto, &host, std::slice::from_ref(&pair), true).worker_count(),
            16
        );
        assert_eq!(
            select(auto, &host, std::slice::from_ref(&pair), false).worker_count(),
            4
        );
        host.memory_bytes = Some(16 << 20);
        assert_eq!(
            select(auto, &host, std::slice::from_ref(&pair), true).worker_count(),
            1
        );
        let manual = Options {
            workers: Some(7),
            manual_workers: true,
            ..auto
        };
        assert_eq!(
            select(manual, &host, std::slice::from_ref(&pair), true).worker_count(),
            7
        );
    }

    #[test]
    fn small_file_policy_requires_confirmed_shared_nvme_and_keeps_other_profiles() {
        let auto = Options {
            auto: true,
            ..Options::default()
        };
        for (kind, filesystem, complete, destination_dev, expected) in [
            ("nvme", "ext4", true, 1, 16),
            ("nvme", "btrfs", true, 1, 16),
            ("nvme", "xfs", true, 1, 16),
            ("nvme", "f2fs", true, 1, 16),
            ("nvme", "nfs", true, 1, 2),
            ("nvme", "ext4", false, 1, 4),
            ("nvme", "ext4", true, 2, 8),
            ("ssd", "ext4", true, 1, 4),
            ("memory", "tmpfs", true, 1, 4),
            ("unknown", "ext4", false, 1, 2),
            ("hdd", "ext4", true, 1, 1),
            ("unknown", "gpfs", false, 1, 32),
        ] {
            let mut pair = StoragePair {
                source: storage(kind, 1),
                destination: storage(kind, destination_dev),
                available_bytes: None,
            };
            for side in [&mut pair.source, &mut pair.destination] {
                side.filesystem = filesystem.into();
                side.complete = complete;
            }
            assert_eq!(
                select(auto, &hardware(), &[pair], true).worker_count(),
                expected,
                "{kind}/{filesystem}, complete={complete}, destination={destination_dev}"
            );
        }
        let pair = StoragePair {
            source: storage("nvme", 1),
            destination: storage("nvme", 1),
            available_bytes: None,
        };
        for options in [
            Options { sync: true, ..auto },
            Options {
                no_fsync: true,
                ..auto
            },
            Options {
                dry_run: true,
                ..auto
            },
        ] {
            assert!(!metadata_parallelism(options, std::slice::from_ref(&pair)));
            assert_eq!(
                select(options, &hardware(), std::slice::from_ref(&pair), true).worker_count(),
                4
            );
        }
        let pair = StoragePair {
            source: storage("nvme", 1),
            destination: storage("nvme", 1),
            available_bytes: None,
        };
        let mut host = hardware();
        host.logical_cpus = 2;
        host.physical_cores = Some(1);
        assert_eq!(
            select(auto, &host, std::slice::from_ref(&pair), true).worker_count(),
            4
        );
        host.logical_cpus = 128;
        host.physical_cores = Some(128);
        assert_eq!(
            select(auto, &host, std::slice::from_ref(&pair), true).worker_count(),
            16
        );
        host.memory_bytes = None;
        assert_eq!(select(auto, &host, &[pair], true).worker_count(), 2);
    }

    #[test]
    fn calibration_requires_work_and_stops_on_measured_plateau() {
        let mut tuner = Controller::new(8, true);
        assert_eq!(tuner.limit, 1);
        tuner.sample(0.0, false);
        assert_eq!(tuner.limit, 1);
        tuner.sample(100.0, true);
        assert_eq!(tuner.limit, 2);
        tuner.sample(150.0, true);
        assert_eq!(tuner.limit, 4);
        tuner.sample(200.0, true);
        assert_eq!(tuner.limit, 8);
        tuner.sample(201.0, true);
        assert_eq!(tuner.limit, 4);
        assert_eq!(tuner.state, "plateau");
        let mut idle = Controller::new(8, true);
        for _ in 0..8 {
            idle.sample(0.0, false);
        }
        assert_eq!(idle.state, "insufficient-work");
        assert_eq!(idle.limit, 1);
    }

    #[test]
    fn short_calibration_retains_observed_setting_and_caps_trials() {
        let mut tuner = Controller::new(8, true);
        tuner.sample(100.0, true);
        assert_eq!(tuner.limit, 2);
        tuner.finish();
        assert_eq!(tuner.limit, 1);
        assert_eq!(tuner.state, "short-workload");
        let mut tuner = Controller::new(8, true);
        for _ in 0..7 {
            tuner.sample(0.0, false);
        }
        tuner.sample(100.0, true);
        assert_eq!(tuner.limit, 1);
        assert_eq!(tuner.state, "complete");
        assert_eq!(tuner.samples, 1);
        let mut fixed = Controller::new(1, true);
        fixed.sample(100.0, true);
        assert_eq!(fixed.limit, 1);
        assert_eq!(fixed.state, "complete");
    }
}
