//! Scoped frame work on the engine CPU pool. Results retain draw order,
//! and the inline mode remains the determinism and performance control.

use crate::shadows::casters::{CasterBatch, CasterJobs};

/// The most compute workers a renderer uses by default. The
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
/// [`DEFAULT_MAX_THREADS`]. On a 10-core machine the render thread
/// waits for four compute workers beside the far scene's five. Inline mode
/// executes directly on the render thread.
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

/// The renderer's bounded scheduling owner.
#[derive(Debug)]
pub(crate) struct Jobs(rs910_jobs::Pool);

impl Jobs {
    pub(crate) fn new(threads: usize) -> Self {
        Self(rs910_jobs::Pool::new(threads).expect("start renderer CPU workers"))
    }

    pub(crate) fn threads(&self) -> usize {
        self.0.threads()
    }

    pub(crate) fn map<T: Send>(&self, count: usize, work: impl Fn(usize) -> T + Sync) -> Vec<T> {
        self.0.map(count, work)
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
