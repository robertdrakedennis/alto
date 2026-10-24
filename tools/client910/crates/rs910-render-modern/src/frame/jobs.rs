//! Scoped frame work on the engine CPU pool. Results retain draw order,
//! and the inline mode remains the determinism and performance control.

use crate::shadows::casters::{CasterBatch, CasterJobs};

/// Maximum concurrent compute batches, including the calling render thread.
/// A bounded pool avoids oversubscribing the GPU driver and streaming workers.
pub(crate) const DEFAULT_MAX_THREADS: usize = 4;
const FIRST_CASTER_JOB: usize = 0;

/// The default thread count: `CLIENT910_MODERN_THREADS`, else the cores the
/// far scene's streaming workers leave (`frame::gpu::far::far_workers`: up
/// to five, busy while the ring streams in), at least two and at most
/// [`DEFAULT_MAX_THREADS`]. The render thread executes one batch alongside
/// the remaining compute workers. Inline mode executes all work on it.
pub(crate) fn default_threads() -> usize {
    crate::modern_debug_flags::flags()
        .threads
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .saturating_sub(crate::frame::gpu::far::far_workers())
                .saturating_sub(MAX_ASSET_WORKERS)
                .clamp(2, DEFAULT_MAX_THREADS)
        })
}

/// The renderer's bounded scheduling owner.
pub(crate) struct Jobs {
    frame: std::sync::Arc<rs910_jobs::Pool>,
    assets: rs910_jobs::assets::AssetQueue,
}

const MAX_ASSET_WORKERS: usize = 2;
const MAX_ASSET_REQUESTS: usize = 96;
const INLINE_ASSET_WORKERS: usize = 0;
const INLINE_THREADS: usize = 1;

impl Jobs {
    pub(crate) fn new(threads: usize) -> Self {
        let workers = if threads == INLINE_THREADS {
            INLINE_ASSET_WORKERS
        } else {
            threads
                .saturating_sub(INLINE_THREADS)
                .min(MAX_ASSET_WORKERS)
        };
        Self {
            frame: std::sync::Arc::new(
                rs910_jobs::Pool::new(threads).expect("start renderer CPU workers"),
            ),
            assets: rs910_jobs::assets::AssetQueue::new(workers, MAX_ASSET_REQUESTS)
                .expect("start asset workers"),
        }
    }

    pub(crate) fn assets(&self) -> rs910_jobs::assets::AssetQueue {
        self.assets.clone()
    }

    pub(crate) fn threads(&self) -> usize {
        self.frame.threads()
    }

    pub(crate) fn map<T: Send>(&self, count: usize, work: impl Fn(usize) -> T + Sync) -> Vec<T> {
        self.frame.map(count, work)
    }
}

impl CasterJobs for Jobs {
    fn select(
        &self,
        count: usize,
        select: &(dyn Fn(usize) -> CasterBatch + Sync),
    ) -> Vec<CasterBatch> {
        // A batch only tests scalar bounds. Queueing it costs more than its
        // arithmetic; keep preparation local and preserve selection order.
        (FIRST_CASTER_JOB..count).map(select).collect()
    }
}
