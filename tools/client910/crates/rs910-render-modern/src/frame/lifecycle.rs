//! Metadata that becomes valid only when a prepared frame is submitted.
//! Uploads and GPU resources stay installed when acquisition skips a frame;
//! only unsent producer progress is restored.

use super::*;
use crate::frame::gpu::ambient::AmbientProgress;
use crate::frame::gpu::point_shadows::PointProgress;
use crate::frame::gpu::probes::{Job, ProbeStats};
use crate::lighting::probes::ProbeGrid;
use crate::shadows::cache::SunCache;

pub(super) struct FrameCheckpoint {
    frame: u64,
    adapted_slot: usize,
    last_ms: Option<i64>,
    sun: SunCache,
    sun_atlas: u32,
    sun_statics: Option<u32>,
    point: PointProgress,
    probes: ProbeProgress,
    ambient: AmbientProgress,
}

struct ProbeProgress {
    captured: Option<((usize, usize), u64)>,
    last_env: Option<u64>,
    job: Option<Job>,
    // The grid changes only on the first frame of a new capture. Steady
    // frames need neither a grid clone nor its allocation.
    grid: Option<Option<ProbeGrid>>,
    stats: ProbeStats,
}

impl FrameCheckpoint {
    pub(super) fn capture(
        renderer: &ModernRenderer,
        snapshot: &SceneSnapshot<'_>,
        frame: u64,
    ) -> Self {
        let probes = &renderer.history.probes;
        let changes_grid = probes.captured
            != Some((
                renderer.scene_resources.scene_token.unwrap_or_default(),
                crate::frame::gpu::probes::env_key(snapshot),
            ))
            || probes.job.as_ref().is_some_and(|job| job.first);
        Self {
            frame,
            adapted_slot: renderer.history.post.adapted_slot,
            last_ms: renderer.history.post.last_ms,
            sun: renderer.history.shadow.sun.state.clone(),
            sun_atlas: renderer.history.shadow.atlas.0,
            sun_statics: renderer.history.shadow.sun.statics.as_ref().map(|s| s.0),
            point: renderer
                .history
                .shadow
                .point
                .checkpoint(renderer.scene_resources.lights.shadow_maps.size),
            probes: ProbeProgress {
                captured: probes.captured,
                last_env: probes.last_env,
                job: probes.job.clone(),
                grid: changes_grid.then(|| probes.grid.clone()),
                stats: probes.stats,
            },
            ambient: renderer.history.ambient.checkpoint(),
        }
    }

    pub(super) fn restore(self, renderer: &mut ModernRenderer) {
        renderer.history.frame = self.frame;
        renderer.history.post.adapted_slot = self.adapted_slot;
        renderer.history.post.last_ms = self.last_ms;
        renderer.history.shadow.sun.state = self.sun;
        if renderer.history.shadow.atlas.0 != self.sun_atlas {
            renderer.history.shadow.sun.state.forget_tiles();
        }
        if renderer.history.shadow.sun.statics.as_ref().map(|s| s.0) != self.sun_statics {
            renderer.history.shadow.sun.state.forget_cache();
        }
        renderer
            .history
            .shadow
            .point
            .restore(self.point, renderer.scene_resources.lights.shadow_maps.size);
        renderer.history.probes.captured = self.probes.captured;
        renderer.history.probes.last_env = self.probes.last_env;
        renderer.history.probes.job = self.probes.job;
        if let Some(grid) = self.probes.grid {
            renderer.history.probes.grid = grid;
        }
        renderer.history.probes.stats = self.probes.stats;
        renderer.history.probes.capture = None;
        renderer.discard_ambient_frame(self.ambient);
    }
}
