//! The modern renderer's particles, CPU half: the frame's particle quads as
//! the renderer receives them, and where they draw.
//!
//! # Input
//!
//! The particle systems are the client's particle runtime (one simulation,
//! ticked and collected by the shell). Each drawn owner (a dynamic loc,
//! player, NPC, spot animation or projectile) adds its list right after its
//! models; culled owners draw nothing. The owner of a plan entity is shell
//! state the snapshot does not carry, so the shell builds the frame's quads
//! (`rs910_scene::particle_quads::Builder`: depth buckets, far first, split
//! on material and lit changes, camera-facing quads) and hands them over as
//! a [`ParticleFrame`]. Nothing is re-simulated here.
//!
//! # Order and size skip
//!
//! [`draw_order`] sorts each batch (one emitter's material list) by the exact
//! view depth of its quads, far first, and skips a particle whose half side
//! (the corners sit at `size * sqrt 2` from the centre) is 10 units or less.
//!
//! # Placement
//!
//! A [`ParticleSegment`] is one owner's particle draw: `entity` is the
//! owner's position in the plan's opaque or transparent list (`list` 0 or
//! 1). [`placements`] resolves it to the owner's entity draws; without
//! segments (no plan) every batch belongs after the transparent list. Each
//! system draws as one alpha-blended forward-pass drawable among the
//! transparent ones, far first by its mean depth (`crate::frame::sprites`).

use std::ops::Range;

use crate::models::draw_list::DrawList;
use crate::scene_snapshot::SceneSnapshot;

pub use rs910_scene::particle_quads::{
    ParticleBatch, ParticleFrame, ParticleSegment, ParticleVertex,
};

/// The modern client draws no particle of this half side or less.
pub const MIN_HALF_SIDE: f32 = 10.0;

/// The half side of a particle quad (corners `(-1,-1)`, `(-1,1)`,
/// `(1,1)`, `(1,-1)` times the size along the view's unit axes).
#[must_use]
pub fn half_side(quad: &[ParticleVertex]) -> f32 {
    let [a, d] = [quad[0].pos, quad[3].pos];
    ((d[0] - a[0]).powi(2) + (d[1] - a[1]).powi(2) + (d[2] - a[2]).powi(2)).sqrt() * 0.5
}

/// The quads of one batch (`corners`: four vertices per quad) to draw, in
/// order: those larger than [`MIN_HALF_SIDE`], far first by the view depth
/// of their centre (`depth`, larger is farther; ties keep the frame's
/// order).
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
/// number of `list`'s entity draws up to and including the owner. An owner outside
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
