//! The renderer's worker pool: a few threads that take a frame's
//! independent jobs (the encode units, the posed models) and hand the
//! results back in job order, whichever thread ran which job and when (the
//! frame is a function of its inputs, not of the thread timing).
//!
//! The caller works too: [`Jobs::map`] runs job `i` for every `i` on the
//! caller and the workers, each taking the next unclaimed job, and returns
//! when every job has finished. With one thread ([`Jobs::new`]`(1)`,
//! `CLIENT910_MODERN_THREADS=1`) every job runs on the caller, in order: the
//! synchronous mode for tests and determinism debugging.
//!
//! A stand-in for the engine's job pool (`rs910_core::jobs`, see
//! `docs/engine-and-studio.md`: a fixed pool, results
//! consumed in submission order, `--sync-workers` inline). The renderer uses
//! only [`Jobs::map`] (a fork and a join), which that pool's fixed pool
//! and named join replace.

use std::any::Any;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use crate::shadows::casters::{CasterBatch, CasterJobs};

/// The most threads a renderer uses by default (the caller included). The
/// encode stops scaling at three or four: wgpu validates and encodes every
/// pass under registries, trackers and reference counts the threads share
/// (wgpu 22 when it records a pass, wgpu 30 when the command encoder
/// finishes), and the Metal driver encodes on each thread, so each unit
/// slows as more run at once (the thread scans of lanes P7 and M-WGPU: 1
/// to 4 threads take the Lumbridge frame's draw from 8.5 to 6.4 ms on wgpu
/// 30, 7.3 to 5.0 on wgpu 22, and 8 threads do no better).
pub(crate) const DEFAULT_MAX_THREADS: usize = 4;

/// The default thread count: `CLIENT910_MODERN_THREADS`, else the cores the
/// far scene's streaming workers leave (`frame::gpu::far::far_workers`: up
/// to five, busy while the ring streams in), at least two and at most
/// [`DEFAULT_MAX_THREADS`]. On a 10-core machine: the render thread and
/// three workers beside the far scene's five.
pub(crate) fn default_threads() -> usize {
    crate::modern_debug_flags::flags()
        .threads
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .saturating_sub(crate::frame::gpu::far::far_workers())
                .clamp(2, DEFAULT_MAX_THREADS)
        })
}

/// See the module docs.
pub(crate) struct Jobs {
    /// `None`: synchronous (one thread, the caller's).
    shared: Option<Arc<Shared>>,
    workers: Vec<JoinHandle<()>>,
}

/// One batch's job, its lifetime erased: [`Shared::run`] does not return
/// before every worker is done with it.
#[derive(Clone, Copy)]
struct Job {
    run: *const (dyn Fn(usize) + Sync),
    n: usize,
}

// SAFETY: `run` points at a `Sync` closure, and the pointer is only
// dereferenced while `Shared::run` (which owns the closure's borrow) waits.
unsafe impl Send for Job {}

struct State {
    /// Bumped per batch; a worker runs each batch once.
    batch: u64,
    job: Option<Job>,
    /// Workers still inside the current batch.
    busy: usize,
    /// The first panic a worker caught in this batch.
    panic: Option<Box<dyn Any + Send>>,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    /// Workers wait here for the next batch.
    wake: Condvar,
    /// The caller waits here for the workers to finish a batch.
    idle: Condvar,
    /// The batch's next unclaimed job.
    next: AtomicUsize,
    /// A batch is running (a nested [`Jobs::map`] from inside a job runs
    /// inline instead of waiting for itself).
    running: AtomicBool,
    workers: usize,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Claim and run jobs until none is left.
fn work(next: &AtomicUsize, n: usize, run: &(dyn Fn(usize) + Sync)) {
    loop {
        let i = next.fetch_add(1, Ordering::Relaxed);
        if i >= n {
            return;
        }
        run(i);
    }
}

impl Shared {
    fn worker(&self) {
        let mut seen = 0;
        loop {
            let job = {
                let mut s = lock(&self.state);
                while s.batch == seen && !s.stop {
                    s = self.wake.wait(s).unwrap_or_else(PoisonError::into_inner);
                }
                if s.stop {
                    return;
                }
                seen = s.batch;
                s.job
            };
            let result = job.map_or(Ok(()), |job| {
                // SAFETY: see `Job`: the caller waits for this worker's
                // `busy` decrement below before the closure goes away.
                let run = unsafe { &*job.run };
                catch_unwind(AssertUnwindSafe(|| work(&self.next, job.n, run)))
            });
            let mut s = lock(&self.state);
            if let Err(p) = result {
                s.panic.get_or_insert(p);
            }
            s.busy -= 1;
            if s.busy == 0 {
                self.idle.notify_all();
            }
        }
    }

    /// Run `run(i)` for `i` in `0..n` on the caller and every worker.
    fn run(&self, n: usize, run: &(dyn Fn(usize) + Sync)) {
        // SAFETY: only the lifetime is erased; the workers stop using the
        // pointer before this function returns (the `busy` wait below, also
        // when the caller's own jobs panic).
        let erased: *const (dyn Fn(usize) + Sync + 'static) = unsafe {
            std::mem::transmute::<
                *const (dyn Fn(usize) + Sync + '_),
                *const (dyn Fn(usize) + Sync + 'static),
            >(run)
        };
        {
            let mut s = lock(&self.state);
            self.next.store(0, Ordering::Relaxed);
            s.job = Some(Job { run: erased, n });
            s.batch += 1;
            s.busy = self.workers;
            self.wake.notify_all();
        }
        let caller = catch_unwind(AssertUnwindSafe(|| work(&self.next, n, run)));
        let panic = {
            let mut s = lock(&self.state);
            while s.busy > 0 {
                s = self.idle.wait(s).unwrap_or_else(PoisonError::into_inner);
            }
            s.job = None;
            s.panic.take()
        };
        if let Err(p) = caller {
            resume_unwind(p);
        }
        if let Some(p) = panic {
            resume_unwind(p);
        }
    }
}

impl Jobs {
    /// A pool of `threads` threads, the caller's included (1: synchronous).
    pub(crate) fn new(threads: usize) -> Self {
        let workers = threads.max(1) - 1;
        if workers == 0 {
            return Self {
                shared: None,
                workers: Vec::new(),
            };
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                batch: 0,
                job: None,
                busy: 0,
                panic: None,
                stop: false,
            }),
            wake: Condvar::new(),
            idle: Condvar::new(),
            next: AtomicUsize::new(0),
            running: AtomicBool::new(false),
            workers,
        });
        let workers = (0..workers)
            .map(|k| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("modern-render-{k}"))
                    .spawn(move || shared.worker())
                    .expect("spawn a renderer worker")
            })
            .collect();
        Self {
            shared: Some(shared),
            workers,
        }
    }

    /// Threads used, the caller's included.
    pub(crate) fn threads(&self) -> usize {
        self.workers.len() + 1
    }

    /// `f(i)` for every `i` in `0..n`, in index order (see the module docs).
    /// A panic in a job is raised here once every job has stopped.
    pub(crate) fn map<T: Send>(&self, n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
        let shared = match self.shared.as_ref() {
            Some(s) if n > 1 && !s.running.swap(true, Ordering::Acquire) => s,
            _ => return (0..n).map(f).collect(),
        };
        struct Release<'a>(&'a AtomicBool);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _release = Release(&shared.running);
        let slots: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
        let run = |i: usize| {
            let value = f(i);
            *slots[i].lock().unwrap_or_else(PoisonError::into_inner) = Some(value);
        };
        shared.run(n, &run);
        slots
            .into_iter()
            .map(|s| {
                s.into_inner()
                    .unwrap_or_else(PoisonError::into_inner)
                    .expect("every job ran")
            })
            .collect()
    }
}

impl CasterJobs for Jobs {
    fn select(
        &self,
        count: usize,
        select: &(dyn Fn(usize) -> CasterBatch + Sync),
    ) -> Vec<CasterBatch> {
        self.map(count, select)
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.as_ref() {
            lock(&shared.state).stop = true;
            shared.wake.notify_all();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

impl std::fmt::Debug for Jobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Jobs({} threads)", self.threads())
    }
}

#[cfg(test)]
mod tests {
    use super::Jobs;

    /// Results come back in job order whichever thread ran each job, a
    /// job's panic reaches the caller after the batch, and the pool keeps
    /// working after it.
    #[test]
    fn results_keep_job_order_and_panics_reach_the_caller() {
        let jobs = Jobs::new(4);
        for n in [0, 1, 7, 300] {
            let out = jobs.map(n, |i| {
                // Uneven work, so the threads finish out of order.
                std::thread::sleep(std::time::Duration::from_micros((i % 5) as u64 * 50));
                i * 3
            });
            assert_eq!(out, (0..n).map(|i| i * 3).collect::<Vec<_>>());
        }
        let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            jobs.map(16, |i| assert_ne!(i, 11, "job 11 fails"))
        }));
        assert!(hit.is_err());
        assert_eq!(jobs.map(3, |i| i), vec![0, 1, 2]);
    }
}
