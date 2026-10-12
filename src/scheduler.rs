use crate::{
    cli::Options,
    copy_engine::{self, Outcome, Task},
    scanner,
};
use std::{
    io,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Condvar, Mutex,
    },
    thread,
};

#[derive(Default)]
pub struct Summary {
    pub copied: AtomicU64,
    pub skipped: AtomicU64,
    pub bytes: AtomicU64,
    pub failures: AtomicU64,
}

impl Summary {
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
    pub fn print(&self, dry_run: bool) {
        println!(
            "{}: {}  Skipped: {}  Bytes: {}  Failures: {}",
            if dry_run { "Planned" } else { "Copied" },
            self.copied.load(Ordering::Relaxed),
            self.skipped.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
            self.failures.load(Ordering::Relaxed)
        );
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

pub fn run(roots: Vec<Task>, options: Options) -> Arc<Summary> {
    let summary = Arc::new(Summary::default());
    thread::scope(|scope| {
        let (sender, receiver) = mpsc::sync_channel::<Task>(64);
        let receiver = Arc::new(Mutex::new(receiver));
        let pending = Arc::new((Mutex::new(0), Condvar::new()));
        let pool = Pool {
            sender,
            pending: pending.clone(),
            summary: summary.clone(),
        };
        let workers = thread::available_parallelism().map_or(1, |n| n.get().min(4));
        for number in 0..workers {
            let receiver = receiver.clone();
            let pending = pending.clone();
            let worker_summary = summary.clone();
            let spawn = thread::Builder::new()
                .name(format!("tcopy-{number}"))
                .spawn_scoped(scope, move || {
                    let mut buffer = vec![0; 128 * 1024];
                    loop {
                        let task = match receiver.lock().unwrap().recv() {
                            Ok(task) => task,
                            Err(_) => break,
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            copy_engine::copy(&task, options, &mut buffer)
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
    });
    summary
}
