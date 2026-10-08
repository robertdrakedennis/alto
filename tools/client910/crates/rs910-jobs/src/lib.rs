//! Bounded CPU workers for scoped, independent work. Callers retain ownership
//! of the borrowed inputs until `map` returns; results retain input order.
//! A single-thread configuration executes on the caller for replay controls.

#![forbid(unsafe_code)]

use rayon::prelude::*;

const INLINE_THREADS: usize = 1;
const PARALLEL_WORK_ITEMS: usize = 2;
const FIRST_JOB: usize = 0;

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
                    .num_threads(threads)
                    .thread_name(|index| format!("engine-job-{index}"))
                    .build()?,
            )
        };
        Ok(Self { workers, threads })
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Complete borrowed work before returning, including panic unwinding.
    /// Indexed collection preserves order independently of worker scheduling.
    pub fn map<T: Send>(&self, count: usize, work: impl Fn(usize) -> T + Sync) -> Vec<T> {
        match &self.workers {
            Some(pool) if count >= PARALLEL_WORK_ITEMS => {
                pool.install(|| (FIRST_JOB..count).into_par_iter().map(&work).collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_batches_preserve_order_and_nested_work_survives_a_panic() {
        const WORKERS: usize = 3;
        const INPUT_COUNT: usize = 5;
        const NEXT_INPUT_OFFSET: usize = 1;
        const INPUTS: [usize; INPUT_COUNT] = [7, 2, 9, 1, 4];
        const PANIC_INDEX: usize = 2;
        const NESTED_ITEMS: usize = 2;
        for threads in [INLINE_THREADS, WORKERS] {
            let pool = Pool::new(threads).unwrap();
            let actual = pool.map(INPUTS.len(), |index| {
                pool.map(NESTED_ITEMS, |offset| INPUTS[index] + offset)
            });
            let expected: Vec<_> = INPUTS
                .iter()
                .map(|value| vec![*value, value + NEXT_INPUT_OFFSET])
                .collect();
            assert_eq!(actual, expected);
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pool.map(INPUTS.len(), |index| assert_ne!(index, PANIC_INDEX))
            }))
            .is_err());
            assert_eq!(pool.map(INPUTS.len(), |index| INPUTS[index]), INPUTS);
        }
    }
}
