//! Bounded CPU workers for scoped, independent work. Callers retain ownership
//! of the borrowed inputs until `map` returns; results retain input order.
//! The configured parallelism includes the caller, which executes a batch.
//! A single-thread configuration executes everything on it for replay controls.

#![forbid(unsafe_code)]

pub mod assets;

use std::sync::Mutex;

const INLINE_THREADS: usize = 1;
const PARALLEL_WORK_ITEMS: usize = 2;
const FIRST_JOB: usize = 0;
const CALLER_THREADS: usize = 1;
const FIRST_BATCH: usize = 0;
const FIRST_WORKER_BATCH: usize = 1;

pub struct Pool {
    workers: Option<rayon::ThreadPool>,
    threads: usize,
}

impl Pool {
    pub fn new(threads: usize) -> Result<Self, rayon::ThreadPoolBuildError> {
        let threads = threads.max(INLINE_THREADS);
        let workers = if threads == INLINE_THREADS {
            None
        } else {
            Some(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads - CALLER_THREADS)
                    .thread_name(|index| format!("engine-job-{index}"))
                    .build()?,
            )
        };
        Ok(Self { workers, threads })
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Queue owned work without waiting. The completion owns either its value or
    /// panic, so a failed job cannot strand an asset request or abort a worker.
    /// Callers bound outstanding requests and keep installation on their owner.
    pub fn spawn<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> std::sync::mpsc::Receiver<std::thread::Result<T>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let run = move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            let _ = sender.send(result);
        };
        match &self.workers {
            Some(pool) => pool.spawn(run),
            None => run(),
        }
        receiver
    }

    /// Complete borrowed work before returning, including panic unwinding.
    /// The caller executes one coarse batch while the scoped workers execute
    /// the rest. This avoids delegating the whole frame and parking its owner.
    /// Batch slots preserve order independently of worker scheduling.
    pub fn map<T: Send>(&self, count: usize, work: impl Fn(usize) -> T + Sync) -> Vec<T> {
        match &self.workers {
            Some(pool) if count >= PARALLEL_WORK_ITEMS => {
                let batch_size = count.div_ceil(self.threads);
                let batches = count.div_ceil(batch_size);
                let results: Vec<Mutex<Option<Vec<T>>>> =
                    (FIRST_BATCH..batches).map(|_| Mutex::new(None)).collect();
                pool.in_place_scope(|scope| {
                    for (batch, result) in results.iter().enumerate().skip(FIRST_WORKER_BATCH) {
                        let work = &work;
                        scope.spawn(move |_| {
                            let start = batch * batch_size;
                            let end = (start + batch_size).min(count);
                            let values = (start..end).map(work).collect();
                            *result.lock().expect("job result slot") = Some(values);
                        });
                    }
                    let values = (FIRST_JOB..batch_size).map(&work).collect();
                    *results[FIRST_BATCH].lock().expect("caller result slot") = Some(values);
                });
                results
                    .into_iter()
                    .flat_map(|result| {
                        result
                            .into_inner()
                            .expect("job result slot")
                            .expect("completed job batch")
                    })
                    .collect()
            }
            _ => (FIRST_JOB..count).map(work).collect(),
        }
    }
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Pool")
            .field("threads", &self.threads)
            .finish()
    }
}
