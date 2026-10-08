//! The modern renderer's particles (renderer plan M9), CPU half: the frame's
//! particle quads as the renderer receives them, and where they draw.
//!
//! # Input: the faithful particle frame, not a new simulation
//!
//! The particle systems are the snapshot's (`SceneSnapshot::particles`, the
//! client's particle runtime: one simulation, ticked and collected by the
//! shell whatever draws). Which systems draw and where is decided by the
//! scene draw: each drawn owner's particles are drawn right after the
//! owner's models (dynamic locs, players, NPCs, spot animations and
//! projectiles), and owners that are culled draw nothing. The owner of a
//! plan entity (a dynamic loc, a player, an NPC, a spot or projectile
//! effect) is shell state the snapshot does not carry, so the shell hands
//! this renderer the faithful toolkit's CPU particle frame of the same
//! runtime (`rs910_render_gpu::particle_render::Frame`: depth buckets, far
//! first, split on material and lit changes, camera-facing quads with the
//! classic corners and rolls) as a [`ParticleFrame`]: the same particles,
//! in the faithful order, with the same owners. Nothing is re-simulated or
//! re-sorted here. The modern client sorts a system's particles back to
//! front too, per emitter; lane Q-FX applies that order: [`draw_order`].
//!
//! # The modern order and size skip
//!
//! The faithful frame's batches are one per owner and material, each the
//! faithful depth buckets walked far first (1,600 buckets of the view depth
//! range, the order inside a bucket the insertion order). The modern client
//! sorts each emitter's particles by their exact depth, far first, and
//! writes no vertices for a particle whose size (the half side: the corners
//! sit at `size * sqrt 2` from the centre) is 10 units or less.
//! [`draw_order`] applies both to a batch (one emitter's material list: the
//! exact order inside the faithful buckets; the half side is the faithful
//! quad's, `size >> 12` in fine units, `particle_render::Frame::add`). The
//! faithful set is still what the shell's check compares (the received
//! quads); the skip only changes what is drawn.
//!
//! # Placement
//!
//! A [`ParticleSegment`] is one owner's particle draw: `entity` is the
//! owner's position in the plan's opaque or transparent list (`list` 0 or
//! 1). [`placements`] resolves it to the owner's entity draws, as the
//! faithful scene pass splits its lists (`particle_render::Segment::
//! resolve`); without segments (no plan) every batch belongs after the
//! transparent list. The renderer records that owner for the shell's check
//! but draws each system where the modern client does: one alpha-blended
//! forward-pass drawable among the transparent ones, far first by its mean
//! depth (`crate::frame::sprites`).

use std::ops::Range;

use crate::models::draw_list::DrawList;
use crate::scene_snapshot::SceneSnapshot;

/// One quad corner (the faithful `ParticleVertex`, stride 24): camera-local
/// position (the camera target is the origin), RGBA bytes, texture
/// coordinate.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleVertex {
    pub pos: [f32; 3],
    pub colour: [u8; 4],
    pub uv: [f32; 2],
}

/// One batch: `quads` consecutive quads from `first_vertex`
/// sharing a material and the lit flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParticleBatch {
    /// The material (`-1`: untextured).
    pub texture: i32,
    /// `setSunAmbientIntensity` was on (the faithful shader ignores it; it
    /// only splits batches).
    pub lit: bool,
    pub first_vertex: u32,
    pub quads: u32,
}

/// One owner's batches (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParticleSegment {
    pub list: u8,
    /// The owner's position in the plan's list (`usize::MAX`: after the
    /// list).
    pub entity: usize,
    pub batches: Range<usize>,
}

/// One frame's particle quads (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct ParticleFrame {
    pub vertices: Vec<ParticleVertex>,
    pub batches: Vec<ParticleBatch>,
    pub segments: Vec<ParticleSegment>,
}

impl ParticleFrame {
    /// The quads of the frame.
    #[must_use]
    pub fn quads(&self) -> usize {
        self.batches.iter().map(|b| b.quads as usize).sum()
    }
}

/// The modern client draws no particle of this half side or less.
pub const MIN_HALF_SIDE: f32 = 10.0;

/// The half side of a faithful particle quad (corners `(-1,-1)`, `(-1,1)`,
/// `(1,1)`, `(1,-1)` times the size along the view's unit axes).
#[must_use]
pub fn half_side(quad: &[ParticleVertex]) -> f32 {
    let [a, d] = [quad[0].pos, quad[3].pos];
    ((d[0] - a[0]).powi(2) + (d[1] - a[1]).powi(2) + (d[2] - a[2]).powi(2)).sqrt() * 0.5
}

/// The quads of one batch (`corners`: four vertices per quad) to draw, in
/// order: with `on`, those larger than [`MIN_HALF_SIDE`], far first by the
/// view depth of their centre (`depth`, larger is farther; ties keep the
/// faithful order); without, every quad in the faithful order.
pub fn draw_order(corners: &[ParticleVertex], depth: impl Fn([f32; 3]) -> f32) -> Vec<usize> {
    let mut keyed: Vec<(f32, usize)> = corners
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, q)| half_side(q) > MIN_HALF_SIDE)
        .map(|(i, q)| {
            let centre: [f32; 3] =
                std::array::from_fn(|k| q.iter().map(|c| c.pos[k]).sum::<f32>() * 0.25);
            (depth(centre), i)
        })
        .collect();
    keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
    keyed.into_iter().map(|(_, i)| i).collect()
}

/// Where each owner's batches draw: `(list, at, batches)`, `at` being the
/// number of `list`'s entity draws before them (the faithful
/// `Segment::resolve` over the scene meshes, here over the frame's draw
/// list, which holds the same draws in the same order). An owner outside
/// the plan, or every batch of a frame without segments, draws after the
/// transparent list.
#[must_use]
pub fn placements(
    snapshot: &SceneSnapshot<'_>,
    list: &DrawList<'_>,
    frame: &ParticleFrame,
) -> Vec<(u8, usize, Range<usize>)> {
    if frame.batches.is_empty() {
        return Vec::new();
    }
    if frame.segments.is_empty() {
        return vec![(1, list.transparent.len(), 0..frame.batches.len())];
    }
    // ends[list][p]: the entity draws up to and including plan entity p.
    let ends = |ids: &[usize], draws: &[crate::models::draw_list::EntityDraw<'_>]| -> Vec<usize> {
        let mut at = 0;
        ids.iter()
            .map(|&id| {
                while draws.get(at).is_some_and(|d| d.id == id) {
                    at += 1;
                }
                at
            })
            .collect()
    };
    let (opaque, transparent) = snapshot
        .live_frame()
        .map_or((Vec::new(), Vec::new()), |live| {
            (
                ends(&live.draw.plan.opaque, &list.opaque),
                ends(&live.draw.plan.transparent, &list.transparent),
            )
        });
    frame
        .segments
        .iter()
        .map(|s| {
            let list_index = s.list.min(1);
            let (ends, len) = if list_index == 0 {
                (&opaque, list.opaque.len())
            } else {
                (&transparent, list.transparent.len())
            };
            let at = ends.get(s.entity).copied().unwrap_or(len).min(len);
            (list_index, at, s.batches.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(segments: Vec<ParticleSegment>) -> ParticleFrame {
        ParticleFrame {
            vertices: vec![
                ParticleVertex {
                    pos: [0.0; 3],
                    colour: [255; 4],
                    uv: [0.0; 2],
                };
                12
            ],
            batches: vec![
                ParticleBatch {
                    texture: -1,
                    lit: false,
                    first_vertex: 0,
                    quads: 2,
                },
                ParticleBatch {
                    texture: 7,
                    lit: true,
                    first_vertex: 8,
                    quads: 1,
                },
            ],
            segments,
        }
    }

    fn quad(centre: [f32; 3], half: f32) -> [ParticleVertex; 4] {
        let v = |dx: f32, dy: f32| ParticleVertex {
            pos: [centre[0] + dx * half, centre[1] + dy * half, centre[2]],
            colour: [255; 4],
            uv: [0.0; 2],
        };
        [v(-1., -1.), v(-1., 1.), v(1., 1.), v(1., -1.)]
    }

    /// Far first by exact depth (ties in the faithful order), half sides of
    /// 10 or less skipped.
    #[test]
    fn particles_draw_far_first_and_skip_the_small_ones() {
        let quads: Vec<ParticleVertex> = [
            quad([0.0, 0.0, 100.0], 16.0),
            quad([0.0, 0.0, 300.0], 16.0),
            quad([0.0, 0.0, 200.0], 10.0),
            quad([0.0, 0.0, 300.0], 12.0),
            quad([0.0, 0.0, 50.0], 10.5),
        ]
        .concat();
        assert!((half_side(&quads[0..4]) - 16.0).abs() < 1e-4);
        let depth = |p: [f32; 3]| p[2];
        assert_eq!(draw_order(&quads, depth), vec![1, 3, 0, 4]);
        assert!(draw_order(&[], depth).is_empty());
    }

    /// Without a plan (no segments) every batch draws after the transparent
    /// list; an owner outside a plan also goes after its list; no batches,
    /// no placement.
    #[test]
    fn owners_without_a_plan_draw_after_the_transparent_list() {
        let camera = crate::camera::SceneCamera::new([0; 3]);
        let (far, near_min) = camera.fog_reference();
        let env = rs910_scene::env::EnvFrame::default_for(far, near_min, &camera.view_entries());
        let snapshot = SceneSnapshot {
            owned: None,
            time_ms: None,
            camera,
            env: &env,
            live: None,
            scene: None,
            floors: &[],
            lights: &[],
            players: None,
            floor_base: [0, 0],
            materials: None,
            pack: None,
            blackout: false,
            local_player: None,
            particles: None,
            underwater: None,
            sky: None,
        };
        let list = DrawList::default();
        let all = frame(Vec::new());
        assert_eq!(all.quads(), 3);
        assert_eq!(placements(&snapshot, &list, &all), vec![(1, 0, 0..2)]);
        let owned = frame(vec![ParticleSegment {
            list: 0,
            entity: 4,
            batches: 1..2,
        }]);
        assert_eq!(placements(&snapshot, &list, &owned), vec![(0, 0, 1..2)]);
        assert!(placements(&snapshot, &list, &ParticleFrame::default()).is_empty());
    }
}
