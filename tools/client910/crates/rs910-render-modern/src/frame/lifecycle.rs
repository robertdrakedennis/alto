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
        let probes = &renderer.probes;
        let changes_grid = probes.captured
            != Some((
                renderer.scene_token.unwrap_or_default(),
                crate::frame::gpu::probes::env_key(snapshot),
            ))
            || probes.job.as_ref().is_some_and(|job| job.first);
        Self {
            frame,
            adapted_slot: renderer.post.adapted_slot,
            last_ms: renderer.post.last_ms,
            sun: renderer.shadow.sun.state.clone(),
            sun_atlas: renderer.shadow.atlas.0,
            sun_statics: renderer.shadow.sun.statics.as_ref().map(|s| s.0),
            point: renderer
                .shadow
                .point
                .checkpoint(renderer.lights.shadow_maps.size),
            probes: ProbeProgress {
                captured: probes.captured,
                last_env: probes.last_env,
                job: probes.job.clone(),
                grid: changes_grid.then(|| probes.grid.clone()),
                stats: probes.stats,
            },
            ambient: renderer.ambient.checkpoint(),
        }
    }

    pub(super) fn restore(self, renderer: &mut ModernRenderer) {
        renderer.frame = self.frame;
        renderer.post.adapted_slot = self.adapted_slot;
        renderer.post.last_ms = self.last_ms;
        renderer.shadow.sun.state = self.sun;
        if renderer.shadow.atlas.0 != self.sun_atlas {
            renderer.shadow.sun.state.forget_tiles();
        }
        if renderer.shadow.sun.statics.as_ref().map(|s| s.0) != self.sun_statics {
            renderer.shadow.sun.state.forget_cache();
        }
        renderer
            .shadow
            .point
            .restore(self.point, renderer.lights.shadow_maps.size);
        renderer.probes.captured = self.probes.captured;
        renderer.probes.last_env = self.probes.last_env;
        renderer.probes.job = self.probes.job;
        if let Some(grid) = self.probes.grid {
            renderer.probes.grid = grid;
        }
        renderer.probes.stats = self.probes.stats;
        renderer.probes.capture = None;
        renderer.discard_ambient_frame(self.ambient);
    }
}
