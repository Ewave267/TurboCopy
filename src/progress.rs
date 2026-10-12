use crate::{cli::Options, copy_engine::Outcome, storage_detect::Storage};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    sync::Mutex,
    time::Instant,
};

pub struct StoragePair {
    pub source: Storage,
    pub destination: Storage,
    pub available_bytes: Option<u64>,
}

pub struct Summary {
    pub hardware: crate::hardware::Hardware,
    pub limit: AtomicU64,
    pub adjustments: AtomicU64,
    pub tuning_state: Mutex<&'static str>,
    pub tuning_samples: AtomicU64,
    pub tuning_rate: Mutex<f64>,
    pub small_files: AtomicU64,
    pub medium_files: AtomicU64,
    pub large_files: AtomicU64,
    pub copied: AtomicU64,
    pub skipped: AtomicU64,
    pub bytes: AtomicU64,
    pub failures: AtomicU64,
    pub transferred: AtomicU64,
    pub discovered: AtomicU64,
    pub chunked: AtomicU64,
    pub fallbacks: AtomicU64,
    pub active: AtomicU64,
    pub peak: AtomicU64,
    pub busy_ns: AtomicU64,
    pub metadata_ns: AtomicU64,
    pub transfer_ns: AtomicU64,
    pub start: Instant,
    pub options: Options,
    pub storage: Vec<StoragePair>,
}
impl Summary {
    pub fn new(options: Options, storage: Vec<StoragePair>, start: Instant) -> Self {
        Self {
            hardware: crate::hardware::Hardware::load(),
            limit: (options.worker_count() as u64).into(),
            adjustments: 0.into(),
            tuning_state: Mutex::new("disabled"),
            tuning_samples: 0.into(),
            tuning_rate: Mutex::new(0.0),
            small_files: 0.into(),
            medium_files: 0.into(),
            large_files: 0.into(),
            copied: 0.into(),
            skipped: 0.into(),
            bytes: 0.into(),
            failures: 0.into(),
            transferred: 0.into(),
            discovered: 0.into(),
            chunked: 0.into(),
            fallbacks: 0.into(),
            active: 0.into(),
            peak: 0.into(),
            busy_ns: 0.into(),
            metadata_ns: 0.into(),
            transfer_ns: 0.into(),
            start,
            options,
            storage,
        }
    }
    pub fn discover(&self, metadata: &std::fs::Metadata) {
        if metadata.is_file() {
            let counter = if metadata.len() < 1 << 20 {
                &self.small_files
            } else if metadata.len() < 64 << 20 {
                &self.medium_files
            } else {
                &self.large_files
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn observe(&self, outcome: Outcome) {
        match outcome {
            Outcome::Copied(bytes) => {
                self.copied.fetch_add(1, Ordering::Relaxed);
                self.bytes.fetch_add(bytes, Ordering::Relaxed);
            }
            Outcome::Skipped => {
                self.skipped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    pub fn fail(&self, message: impl std::fmt::Display) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        eprintln!("tcopy: {message}");
    }
    pub fn metadata<T>(&self, operation: impl FnOnce() -> T) -> T {
        let _timer = Timer::new(&self.metadata_ns);
        operation()
    }
    pub fn progress(&self) {
        let seconds = self.start.elapsed().as_secs_f64().max(1e-9);
        eprintln!("tcopy: copied {} / discovered {} files; transferred {} bytes ({:.2} MiB/s); active {}/{}",
            count(&self.copied), count(&self.discovered), count(&self.transferred),
            count(&self.transferred) as f64 / seconds / 1048576.0, count(&self.active), count(&self.limit));
    }
    pub fn print(&self) {
        let elapsed = self.start.elapsed().as_secs_f64().max(1e-9);
        let throughput = if self.options.dry_run {
            0.0
        } else {
            count(&self.bytes) as f64 / elapsed
        };
        let utilization =
            count(&self.busy_ns) as f64 / 1e9 / elapsed / self.options.worker_count() as f64;
        if self.options.json {
            let storage = self.storage.iter().map(|pair| format!(
                "{{\"source\":{},\"destination\":{},\"same_filesystem\":{},\"shared_storage\":{},\"available_bytes\":{}}}",
                storage_json(&pair.source), storage_json(&pair.destination), pair.source.dev == pair.destination.dev,
                pair.source.shared_with(&pair.destination).map_or("null", |v| if v { "true" } else { "false" }),
                optional_number(pair.available_bytes)
            )).collect::<Vec<_>>().join(",");
            println!("{{\"dry_run\":{},\"gpfs\":{},\"fsync\":{},\"buffer_size\":{},\"workers\":{},\"chunk_size\":{},\"copied\":{},\"skipped\":{},\"bytes\":{},\"failures\":{},\"discovered\":{},\"transferred_bytes\":{},\"chunked_files\":{},\"positioned_fallbacks\":{},\"elapsed_seconds\":{:.6},\"throughput_bytes_per_second\":{:.3},\"worker_utilization\":{:.6},\"peak_active_workers\":{},\"metadata_seconds\":{:.6},\"transfer_seconds\":{:.6},\"storage\":[{}],\"auto\":{},\"final_workers\":{},\"concurrency_adjustments\":{},\"tuning_state\":{},\"tuning_samples\":{},\"tuning_best_score\":{:.3},\"chunk_policy\":{},\"hardware\":{},\"workload\":{{\"small_files\":{},\"medium_files\":{},\"large_files\":{}}}}}",
                self.options.dry_run, self.options.gpfs, !self.options.no_fsync, self.options.buffer_size(), self.options.worker_count(), self.options.chunk_size.map_or("null".into(), |n| n.to_string()),
                count(&self.copied), count(&self.skipped), count(&self.bytes), count(&self.failures), count(&self.discovered),
                count(&self.transferred), count(&self.chunked), count(&self.fallbacks), elapsed, throughput, utilization,
                count(&self.peak), count(&self.metadata_ns) as f64 / 1e9, count(&self.transfer_ns) as f64 / 1e9, storage,
                self.options.auto, count(&self.limit), count(&self.adjustments), quote(&self.tuning_state.lock().unwrap()), count(&self.tuning_samples), *self.tuning_rate.lock().unwrap(),
                quote(if self.options.gpfs && !self.options.manual_chunks { "gpfs" } else if self.options.auto && !self.options.manual_chunks { "automatic" } else if self.options.chunk_size.is_some() { "manual" } else { "streaming" }), self.hardware_json(),
                count(&self.small_files), count(&self.medium_files), count(&self.large_files));
        } else {
            println!("{}: {}  Skipped: {}  Bytes: {}  Failures: {}  Elapsed: {:.3}s  Throughput: {:.2} MiB/s",
                if self.options.dry_run { "Planned" } else { "Copied" }, count(&self.copied), count(&self.skipped),
                count(&self.bytes), count(&self.failures), elapsed, throughput / 1048576.0);
            println!("Workers: {}  Peak active: {}  Utilization: {:.1}%  Metadata: {:.3}s  Transfer: {:.3}s  Chunked files: {}",
                self.options.worker_count(), count(&self.peak), utilization * 100.0,
                count(&self.metadata_ns) as f64 / 1e9, count(&self.transfer_ns) as f64 / 1e9, count(&self.chunked));
            if self.options.gpfs || self.options.no_fsync {
                println!(
                    "Profile: {}  Durability barriers: {}  Buffer: {} bytes",
                    if self.options.gpfs { "gpfs" } else { "default" },
                    if self.options.no_fsync {
                        "disabled"
                    } else {
                        "enabled"
                    },
                    self.options.buffer_size()
                );
            }
            if self.options.auto {
                println!("Adaptation: {}  Final workers: {}  Adjustments: {}  CPU: {} logical / {} physical  Available memory: {} bytes",
                    *self.tuning_state.lock().unwrap(), count(&self.limit), count(&self.adjustments), self.hardware.logical_cpus,
                    optional_number(self.hardware.physical_cores), optional_number(self.hardware.memory_bytes));
                println!("Workload: {} small / {} medium / {} large files; automatic large-file chunks selected per file",
                    count(&self.small_files), count(&self.medium_files), count(&self.large_files));
            }
            for pair in &self.storage {
                println!(
                    "Storage: {} ({}, {}) -> {} ({}, {}); shared storage: {}",
                    pair.source.device,
                    pair.source.filesystem,
                    pair.source.kind,
                    pair.destination.device,
                    pair.destination.filesystem,
                    pair.destination.kind,
                    pair.source
                        .shared_with(&pair.destination)
                        .map_or("unknown", |shared| if shared { "yes" } else { "no" })
                );
            }
        }
    }
    fn hardware_json(&self) -> String {
        format!("{{\"logical_cpus\":{},\"physical_cores\":{},\"available_memory_bytes\":{},\"cpu_pressure_avg10\":{},\"io_pressure_avg10\":{}}}",
            self.hardware.logical_cpus, optional_number(self.hardware.physical_cores), optional_number(self.hardware.memory_bytes),
            optional_number(self.hardware.cpu_pressure), optional_number(self.hardware.io_pressure))
    }
}
fn optional_number(value: Option<impl std::fmt::Display>) -> String {
    value.map_or_else(|| "null".into(), |v| v.to_string())
}
fn count(value: &AtomicU64) -> u64 {
    value.load(Ordering::Relaxed)
}

pub struct Timer<'a> {
    counter: &'a AtomicU64,
    start: Instant,
}
impl<'a> Timer<'a> {
    pub fn new(counter: &'a AtomicU64) -> Self {
        Self {
            counter,
            start: Instant::now(),
        }
    }
}
impl Drop for Timer<'_> {
    fn drop(&mut self) {
        self.counter.fetch_add(
            self.start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
}
fn storage_json(storage: &Storage) -> String {
    format!("{{\"filesystem\":{},\"device\":{},\"kind\":{},\"backing_devices\":[{}],\"topology_complete\":{}}}",
        quote(&storage.filesystem), quote(&storage.device), quote(storage.kind),
        storage.backing.iter().map(|s| quote(s)).collect::<Vec<_>>().join(","), storage.complete)
}
fn quote(text: &str) -> String {
    let mut output = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            ch if ch < '\u{20}' => {
                use std::fmt::Write;
                write!(output, "\\u{:04x}", ch as u32).unwrap();
            }
            ch => output.push(ch),
        }
    }
    output.push('"');
    output
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_strings_escape_controls_and_keep_unicode() {
        assert_eq!(quote("\"\\\n\t\0é"), "\"\\\"\\\\\\u000a\\u0009\\u0000é\"");
    }
}
