//! Bounded owned asset work, independent of the scoped frame pool. Priority
//! promotion is read when selecting the next job; canceled queued jobs never run.
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    mpsc, Arc, Condvar, Mutex,
};

const INLINE_WORKERS: usize = 0;
const FIRST_WORKER: usize = 0;
const NO_ACTIVE_JOBS: usize = 0;
const ONE_ACTIVE_JOB: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum AssetPriority {
    Background,
    Probe,
    Shadow,
    Visible,
    Scene,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AssetState {
    Queued,
    Running,
    Ready,
    Canceled,
}

struct Control {
    priority: AtomicU8,
    state: AtomicU8,
    canceled: AtomicBool,
}

/// One owned request. Keeping the handle pins interest in its result; dropping
/// it cancels pending work. Running jobs finish safely and discard their result.
pub struct AssetHandle<T> {
    completion: Mutex<mpsc::Receiver<std::thread::Result<T>>>,
    control: Arc<Control>,
}

impl<T> AssetHandle<T> {
    pub fn promote(&self, priority: AssetPriority) {
        self.control
            .priority
            .fetch_max(priority as u8, Ordering::Relaxed);
    }

    pub fn state(&self) -> AssetState {
        let state = self.control.state.load(Ordering::Acquire);
        if state == AssetState::Queued as u8 {
            AssetState::Queued
        } else if state == AssetState::Running as u8 {
            AssetState::Running
        } else if state == AssetState::Ready as u8 {
            AssetState::Ready
        } else {
            AssetState::Canceled
        }
    }

    pub fn try_recv(&self) -> Result<std::thread::Result<T>, mpsc::TryRecvError> {
        self.completion.lock().expect("asset completion").try_recv()
    }

    pub fn cancel(&self) {
        self.control.canceled.store(true, Ordering::Release);
        self.control
            .state
            .store(AssetState::Canceled as u8, Ordering::Release);
    }
}

impl<T> Drop for AssetHandle<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Job {
    control: Arc<Control>,
    run: Box<dyn FnOnce() + Send>,
}

#[derive(Default)]
struct Pending {
    jobs: Vec<Job>,
    active: usize,
    stopped: bool,
}

struct Shared {
    pending: Mutex<Pending>,
    wake: Condvar,
    capacity: usize,
}

struct Workers {
    shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
    inline: bool,
}

impl Drop for Workers {
    fn drop(&mut self) {
        {
            let mut pending = self.shared.pending.lock().expect("asset queue");
            pending.stopped = true;
            for job in pending.jobs.drain(..) {
                job.control
                    .state
                    .store(AssetState::Canceled as u8, Ordering::Release);
            }
        }
        self.shared.wake.notify_all();
        for worker in self.threads.drain(..) {
            if worker.thread().id() != std::thread::current().id() {
                worker.join().expect("asset worker shutdown");
            }
        }
    }
}

/// A shared admission limit and priority queue for heterogeneous asset types.
/// Typed handles keep installation and cache ownership with their consumers.
#[derive(Clone)]
pub struct AssetQueue(Arc<Workers>);

impl AssetQueue {
    pub fn new(workers: usize, capacity: usize) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending::default()),
            wake: Condvar::new(),
            capacity,
        });
        let mut owner = Workers {
            shared,
            threads: Vec::new(),
            inline: workers == INLINE_WORKERS,
        };
        for index in FIRST_WORKER..workers {
            let shared = owner.shared.clone();
            owner.threads.push(
                std::thread::Builder::new()
                    .name(format!("engine-asset-{index}"))
                    .spawn(move || worker(&shared))?,
            );
        }
        Ok(Self(Arc::new(owner)))
    }

    /// Admission is nonblocking. A full queue asks the consumer to retry later.
    pub fn spawn<T: Send + 'static>(
        &self,
        priority: AssetPriority,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<AssetHandle<T>> {
        let control = Arc::new(Control {
            priority: AtomicU8::new(priority as u8),
            state: AtomicU8::new(AssetState::Queued as u8),
            canceled: AtomicBool::new(false),
        });
        let (sender, completion) = mpsc::channel();
        let result_control = control.clone();
        let run: Box<dyn FnOnce() + Send> = Box::new(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            if !result_control.canceled.load(Ordering::Acquire) {
                // Publish before Ready so a ready handle always has a completion.
                let _ = sender.send(result);
                let _ = result_control.state.compare_exchange(
                    AssetState::Running as u8,
                    AssetState::Ready as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        });
        if self.0.inline {
            control
                .state
                .store(AssetState::Running as u8, Ordering::Release);
            run();
        } else {
            let mut pending = self.0.shared.pending.lock().expect("asset queue");
            pending
                .jobs
                .retain(|job| !job.control.canceled.load(Ordering::Acquire));
            if pending.stopped || pending.jobs.len() + pending.active >= self.0.shared.capacity {
                return None;
            }
            pending.jobs.push(Job {
                control: control.clone(),
                run,
            });
            self.0.shared.wake.notify_one();
        }
        Some(AssetHandle {
            completion: Mutex::new(completion),
            control,
        })
    }
}

fn worker(shared: &Shared) {
    loop {
        let job = {
            let mut pending = shared.pending.lock().expect("asset queue");
            loop {
                pending
                    .jobs
                    .retain(|job| !job.control.canceled.load(Ordering::Acquire));
                if pending.stopped {
                    return;
                }
                // Strictly greater preserves FIFO among equal priorities, including
                // promoted requests. No stale heap entries accumulate on promotion.
                let next = pending
                    .jobs
                    .iter()
                    .enumerate()
                    .fold(None, |best, (index, job)| {
                        let priority = job.control.priority.load(Ordering::Relaxed);
                        match best {
                            Some((_, previous)) if previous >= priority => best,
                            _ => Some((index, priority)),
                        }
                    });
                if let Some((index, _)) = next {
                    let job = pending.jobs.remove(index);
                    pending.active += ONE_ACTIVE_JOB;
                    let _ = job.control.state.compare_exchange(
                        AssetState::Queued as u8,
                        AssetState::Running as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    break job;
                }
                debug_assert!(pending.active <= shared.capacity);
                pending = shared.wake.wait(pending).expect("asset queue wake");
            }
        };
        if !job.control.canceled.load(Ordering::Acquire) {
            (job.run)();
        }
        let mut pending = shared.pending.lock().expect("asset queue");
        debug_assert!(pending.active > NO_ACTIVE_JOBS);
        pending.active -= ONE_ACTIVE_JOB;
    }
}
