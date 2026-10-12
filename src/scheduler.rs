use crate::{
    cli::Options,
    copy_engine::{self, Task},
    scanner,
};
use std::{
    io,
    sync::{
        atomic::Ordering,
        mpsc::{self, SyncSender},
        Arc, Condvar, Mutex,
    },
    thread,
};

pub use crate::progress::Summary;

pub struct Concurrency {
    active: Mutex<usize>,
    ready: Condvar,
    pub summary: Arc<Summary>,
}
impl Concurrency {
    pub fn new(summary: Arc<Summary>) -> Self {
        Self {
            active: Mutex::new(0),
            ready: Condvar::new(),
            summary,
        }
    }
    fn permit(&self) -> Permit<'_> {
        let active = self.summary.active.fetch_add(1, Ordering::Relaxed) + 1;
        self.summary.peak.fetch_max(active, Ordering::Relaxed);
        Permit {
            budget: self,
            start: std::time::Instant::now(),
        }
    }
    fn acquire(&self) -> Permit<'_> {
        let mut active = self.active.lock().unwrap();
        while *active >= self.summary.limit.load(Ordering::Relaxed) as usize {
            active = self.ready.wait(active).unwrap();
        }
        *active += 1;
        self.permit()
    }
    pub fn try_acquire(&self) -> Option<Permit<'_>> {
        let mut active = self.active.lock().unwrap();
        if *active >= self.summary.limit.load(Ordering::Relaxed) as usize {
            return None;
        }
        *active += 1;
        Some(self.permit())
    }
    pub fn resize(&self, limit: usize) {
        let _active = self.active.lock().unwrap();
        let limit = limit.clamp(1, self.summary.options.worker_count()) as u64;
        if self.summary.limit.swap(limit, Ordering::Relaxed) != limit {
            self.summary.adjustments.fetch_add(1, Ordering::Relaxed);
            self.ready.notify_all();
        }
    }
}
pub struct Permit<'a> {
    budget: &'a Concurrency,
    start: std::time::Instant,
}
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.budget.summary.busy_ns.fetch_add(
            self.start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
        let mut active = self.budget.active.lock().unwrap();
        self.budget.summary.active.fetch_sub(1, Ordering::Relaxed);
        *active -= 1;
        self.budget.ready.notify_one();
    }
}

pub struct Pool {
    sender: SyncSender<Task>,
    pending: Arc<(Mutex<usize>, Condvar)>,
    pub summary: Arc<Summary>,
}

impl Pool {
    pub fn submit(&self, task: Task) -> io::Result<()> {
        crate::fsutil::check_cancelled()?;
        self.summary.discover(&task.metadata);
        self.summary.discovered.fetch_add(1, Ordering::Relaxed);
        *self.pending.0.lock().unwrap() += 1;
        if self.sender.send(task).is_err() {
            *self.pending.0.lock().unwrap() -= 1;
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "copy workers stopped",
            ));
        }
        Ok(())
    }
    pub fn drain(&self) {
        let mut count = self.pending.0.lock().unwrap();
        while *count != 0 {
            count = self.pending.1.wait(count).unwrap();
        }
    }
}

pub fn run(roots: Vec<Task>, options: Options, start: std::time::Instant) -> Arc<Summary> {
    use std::os::unix::fs::MetadataExt;
    let detector = Arc::new(crate::storage_detect::Detector::load());
    let storage = roots
        .iter()
        .map(|root| {
            let destination_dev =
                crate::fsutil::optional_metadata(&root.parent.path().join(&root.name))
                    .ok()
                    .flatten()
                    .filter(|m| m.is_dir())
                    .map_or_else(
                        || root.parent.metadata().map(|m| m.dev()).unwrap_or(0),
                        |m| m.dev(),
                    );
            crate::progress::StoragePair {
                source: detector.detect(&root.source_path, root.metadata.dev()),
                destination: detector.detect(&root.destination, destination_dev),
                available_bytes: root
                    .parent
                    .child(&root.name)
                    .unwrap_or_else(|_| root.parent.clone())
                    .available_bytes(),
            }
        })
        .collect();
    let mut summary = Summary::new(options, storage, start);
    let options = crate::tuner::select(options, &summary.hardware, &summary.storage);
    summary.options = options;
    let adaptive = options.auto && !options.no_tune && !options.manual_workers && !options.dry_run;
    let mut controller = crate::tuner::Controller::new(options.worker_count(), options.tune);
    if adaptive && summary.hardware.pressured() {
        controller.sample(0.0, false, true);
    }
    summary.limit.store(
        if adaptive {
            controller.limit
        } else {
            options.worker_count()
        } as u64,
        Ordering::Relaxed,
    );
    *summary.tuning_state.lock().unwrap() = if adaptive {
        controller.state
    } else if options.manual_workers && options.auto {
        "manual-workers"
    } else {
        "disabled"
    };
    let summary = Arc::new(summary);
    summary.metadata_ns.store(
        start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
    let budget = Arc::new(Concurrency::new(summary.clone()));
    thread::scope(|scope| {
        let (stop_sender, stop_receiver) = mpsc::channel::<()>();
        if options.progress || adaptive {
            if options.progress {
                summary.progress();
            }
            let reporter = summary.clone();
            let feedback_budget = budget.clone();
            if let Err(error) = thread::Builder::new()
                .name("tcopy-progress".into())
                .spawn_scoped(scope, move || {
                    crate::tuner::monitor(
                        &reporter,
                        &feedback_budget,
                        controller,
                        adaptive,
                        stop_receiver,
                    );
                })
            {
                summary.fail(format!("cannot start progress reporter: {error}"));
            }
        }
        let (sender, receiver) = mpsc::sync_channel::<Task>(64);
        let receiver = Arc::new(Mutex::new(receiver));
        let pending = Arc::new((Mutex::new(0), Condvar::new()));
        let pool = Pool {
            sender,
            pending: pending.clone(),
            summary: summary.clone(),
        };
        let workers = options.worker_count();
        for number in 0..workers {
            let receiver = receiver.clone();
            let pending = pending.clone();
            let worker_summary = summary.clone();
            let budget = budget.clone();
            let detector = detector.clone();
            let spawn = thread::Builder::new()
                .name(format!("tcopy-{number}"))
                .spawn_scoped(scope, move || {
                    let mut buffer = vec![0; options.buffer_size()];
                    loop {
                        let task = match receiver.lock().unwrap().recv() {
                            Ok(task) => task,
                            Err(_) => break,
                        };
                        let _permit = budget.acquire();
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            copy_engine::copy(&task, options, &mut buffer, &budget, &detector)
                        }));
                        match result {
                            Ok(Ok(outcome)) => worker_summary.observe(outcome),
                            Ok(Err(error)) => worker_summary.fail(format!(
                                "{:?} -> {:?}: {error}",
                                task.source_path, task.destination
                            )),
                            Err(_) => worker_summary
                                .fail(format!("worker panicked copying {:?}", task.source_path)),
                        }
                        let mut count = pending.0.lock().unwrap();
                        *count -= 1;
                        pending.1.notify_all();
                    }
                });
            if let Err(error) = spawn {
                summary.fail(format!("cannot start worker: {error}"));
                return;
            }
        }
        for root in roots {
            if let Err(error) = scanner::scan(root, options, &pool) {
                summary.fail(error);
            }
            if crate::fsutil::received_signal() != 0 {
                break;
            }
        }
        pool.drain();
        drop(stop_sender);
    });
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permit_budget_bounds_concurrent_work_and_returns_slots() {
        let summary = Arc::new(Summary::new(
            Options {
                workers: Some(2),
                ..Options::default()
            },
            vec![],
            std::time::Instant::now(),
        ));
        let budget = Concurrency::new(summary.clone());
        {
            let _first = budget.acquire();
            let second = budget.try_acquire().unwrap();
            assert!(budget.try_acquire().is_none());
            drop(second);
            assert!(budget.try_acquire().is_some());
        }
        thread::scope(|scope| {
            for _ in 0..8 {
                let budget = &budget;
                scope.spawn(move || {
                    for _ in 0..100 {
                        let _slot = budget.acquire();
                        thread::yield_now();
                    }
                });
            }
        });
        assert_eq!(summary.active.load(Ordering::Relaxed), 0);
        assert_eq!(summary.peak.load(Ordering::Relaxed), 2);
        assert_eq!(*budget.active.lock().unwrap(), 0);
    }

    #[test]
    fn shrinking_live_budget_drains_existing_tasks_and_growth_wakes_waiters() {
        let summary = Arc::new(Summary::new(
            Options {
                workers: Some(3),
                ..Options::default()
            },
            vec![],
            std::time::Instant::now(),
        ));
        let budget = Concurrency::new(summary.clone());
        let first = budget.acquire();
        let second = budget.acquire();
        budget.resize(1);
        assert!(budget.try_acquire().is_none());
        drop(second);
        assert!(budget.try_acquire().is_none());
        thread::scope(|scope| {
            let (started, ready) = mpsc::channel();
            let (finished, done) = mpsc::channel();
            let budget = &budget;
            scope.spawn(move || {
                started.send(()).unwrap();
                let _slot = budget.acquire();
                finished.send(()).unwrap();
            });
            ready.recv().unwrap();
            assert!(done.try_recv().is_err());
            budget.resize(2);
            done.recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
        });
        drop(first);
        budget.resize(999);
        assert_eq!(summary.limit.load(Ordering::Relaxed), 3);
        budget.resize(0);
        assert_eq!(summary.limit.load(Ordering::Relaxed), 1);
        assert_eq!(*budget.active.lock().unwrap(), 0);
    }
}
