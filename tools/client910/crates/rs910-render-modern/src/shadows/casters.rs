//! The sun shadows' off-screen casters (renderer plan §4(l)).
//!
//! The M3 caster set is the frame's draws: the classic draw plan's entities,
//! culled to the camera. A loc outside the view still throws
//! its shadow into it: the trees and houses by the Lumbridge river shade
//! the river in the faithful frame but cast nothing in the modern frame when
//! they are off screen, which is why the river showed no shadow even where
//! the water receives the cascades.
//!
//! The modern client gathers the shadow pass's casters per cascade, not per
//! camera: the pass draws a drawable queue that the scene gather fills for the
//! shadow pass type from the cascades' volumes. This module adds, as
//! shadow-only draws, the live scene's locs the plan left out whose
//! light-space footprint (position and extent, projected along the sun) meets
//! a cascade's square: the draws that the per-cascade gather would add. Only
//! locs (static and dynamic), never the transient models or players the
//! plan culled; only levels up to the highest level the plan draws, so the
//! roofs the classic roof removal hides do not cast, except the entities the
//! roof removal hid this frame ([`crate::shadows::interior::RoofHidden`]):
//! the modern client applies the roof removal and the level cap only when
//! gathering for the main colour pass, so they cast, each culled per cascade
//! like any other loc and gathered once (an earlier version drew them into
//! every cascade, and those at or below the level cap a second time through
//! this gather).

use glam::DVec3;

use crate::models::draw_list::EntityDraw;
use crate::scene::EntityRef;
use crate::scene_snapshot::SceneSnapshot;
use crate::shadows::interior::RoofHidden;
use crate::shadows::ShadowFrame;

/// The footprint half-size (fine units) of a loc without bounds.
pub const DEFAULT_MARGIN: f64 = 1024.0;

/// The cascades (bit `k`: cascade `k`) a caster at scene-local `p` with
/// footprint half-size `margin` can shade: in light space its footprint
/// meets the region whose receivers read the cascade, grown by the
/// filter's reach (`Profile::reach_texels`: two texels for the classic
/// filters, 3.5 for the modern client's shifted 4x4 taps): the slice's
/// bounding sphere seen along the light
/// (a circle of `Fit::radius`) where the cascade is selected by sphere
/// (low and medium), the whole map (`Fit::centre` +- `Fit::radius`) where
/// it is selected by map (high and ultra), and it lies
/// between the caster range's near end (the sphere pulled back towards the
/// sun, `Fit::depth`) and the sphere's far side (a caster beyond every
/// receiver shades none of them). A caster outside writes no texel any
/// receiver reads, so culling by this mask leaves the frame unchanged.
#[must_use]
pub fn cascades_met(frame: &ShadowFrame, p: [f64; 3], margin: f64) -> u8 {
    let l = frame.basis.apply(DVec3::from(p));
    let mut mask = 0;
    for (k, fit) in frame.fits.iter().enumerate() {
        let reach = fit.radius + margin + frame.profile.reach_texels * fit.texel;
        let (dx, dy) = (l.x - fit.centre[0], l.y - fit.centre[1]);
        let across = if frame.profile.select_by_map {
            dx.abs() <= reach && dy.abs() <= reach
        } else {
            dx * dx + dy * dy <= reach * reach
        };
        let along = l.z >= fit.depth[0] - margin && l.z <= fit.depth[1] + margin;
        if across && along {
            mask |= 1 << k;
        }
    }
    mask
}

/// The cascades a planned (visible) entity casts into: a
/// loc's from its footprint ([`cascades_met`], as the modern client gathers
/// each cascade's casters from its own volume); every cascade for what the live
/// scene does not place (temporaries, entities without a slot).
#[must_use]
pub fn visible_cascades(snapshot: &SceneSnapshot<'_>, frame: &ShadowFrame, id: usize) -> u8 {
    entity_cascades(
        frame,
        snapshot.live_frame().and_then(|live| live.entities.get(id)),
    )
}

pub(crate) fn entity_cascades(frame: &ShadowFrame, entity: Option<&crate::draw::DrawEntity>) -> u8 {
    let Some(e) = entity else {
        return u8::MAX;
    };
    if matches!(e.source, EntityRef::Temporary(_)) {
        return u8::MAX;
    }
    let p = [e.x, e.y, e.z].map(f64::from);
    cascades_met(frame, p, margin(e.bounds))
}

/// Whether [`cascades_met`] is any.
#[must_use]
pub fn meets_cascades(frame: &ShadowFrame, p: [f64; 3], margin: f64) -> bool {
    cascades_met(frame, p, margin) != 0
}

/// A loc's footprint half-size from its draw bounds (`DrawEntity::bounds`:
/// minimum corner then sizes, `DynamicScene`), else [`DEFAULT_MARGIN`]: its
/// largest size, as the corner may lie that far from the position.
fn margin(bounds: Option<[i32; 6]>) -> f64 {
    bounds.map_or(DEFAULT_MARGIN, |b| {
        f64::from(b[3].max(b[4]).max(b[5]).max(0))
    })
}

/// Selected caster indices, cascade masks, and roof-hidden flags for one job.
pub(crate) type CasterBatch = Vec<(usize, u8, bool)>;

/// Run immutable caster selection jobs and return results in job order.
/// The frame owner supplies the scheduler; shadow policy never owns its pool.
pub(crate) trait CasterJobs {
    fn select(
        &self,
        count: usize,
        select: &(dyn Fn(usize) -> CasterBatch + Sync),
    ) -> Vec<CasterBatch>;
}

/// The shadow-only draws of `snapshot` under `frame` (see the module docs),
/// in the live scene's entity order, each with the cascades it meets
/// ([`cascades_met`]: it is drawn into those only), and whether it is a
/// roof-hidden entity of `hidden` (`None`: lane Q-INT's casters off).
#[must_use]
pub fn off_screen<'a>(
    snapshot: &SceneSnapshot<'a>,
    frame: &ShadowFrame,
    hidden: Option<&RoofHidden>,
) -> Vec<(EntityDraw<'a>, u8, bool)> {
    gather(snapshot, frame, hidden, None)
}

pub(crate) fn off_screen_parallel<'a>(
    snapshot: &SceneSnapshot<'a>,
    frame: &ShadowFrame,
    hidden: Option<&RoofHidden>,
    jobs: &dyn CasterJobs,
) -> Vec<(EntityDraw<'a>, u8, bool)> {
    gather(snapshot, frame, hidden, Some(jobs))
}

/// Coarse job batches amortize the pool's ordered result slots.
const CASTERS_PER_JOB: usize = 32;

fn gather<'a>(
    snapshot: &SceneSnapshot<'a>,
    frame: &ShadowFrame,
    hidden: Option<&RoofHidden>,
    jobs: Option<&dyn CasterJobs>,
) -> Vec<(EntityDraw<'a>, u8, bool)> {
    let mut out = Vec::new();
    if snapshot.blackout {
        return out;
    }
    let (Some(live), Some(scene)) = (snapshot.live_frame(), snapshot.scene) else {
        return out;
    };
    let plan = &live.draw.plan;
    let mut planned = vec![false; live.entities.len()];
    let mut top_level = i32::MIN;
    for &id in plan.opaque.iter().chain(&plan.transparent) {
        if let Some(slot) = planned.get_mut(id) {
            *slot = true;
        }
        if let Some(e) = live.entities.get(id) {
            top_level = top_level.max(e.level);
        }
    }
    if top_level == i32::MIN && hidden.is_none_or(|h| h.entities.is_empty()) {
        return out;
    }
    // Capture only immutable, Sync metadata in the pool. Entity model lookup
    // and cache installation remain ordered on the render thread afterwards.
    let entities = live.entities;
    let select = |job: usize| {
        let start = job * CASTERS_PER_JOB;
        let end = (start + CASTERS_PER_JOB).min(entities.len());
        let mut selected = Vec::new();
        for (offset, entity) in entities[start..end].iter().enumerate() {
            let id = start + offset;
            if planned[id] || matches!(entity.source, EntityRef::Temporary(_)) {
                continue;
            }
            let roof = hidden.is_some_and(|hidden| hidden.hides(id));
            if entity.level > top_level && !roof {
                continue;
            }
            let position = [entity.x, entity.y, entity.z].map(f64::from);
            let mask = cascades_met(frame, position, margin(entity.bounds));
            if mask != 0 {
                selected.push((id, mask, roof));
            }
        }
        selected
    };
    let count = entities.len().div_ceil(CASTERS_PER_JOB);
    let selected = match jobs {
        Some(jobs) => jobs.select(count, &select),
        None => (0..count).map(select).collect(),
    };
    let mut draws = Vec::new();
    for (id, mask, roof) in selected.into_iter().flatten() {
        crate::models::draw_list::entity(snapshot, live, scene, id, &mut draws);
        out.extend(draws.drain(..).map(|draw| (draw, mask, roof)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A caster over a cascade's receivers meets it from the sun's side up
    /// to the far side of the receivers; one beyond them, one far across
    /// the light or past the pulled-back near end misses; a margin reaches
    /// a cascade.
    #[test]
    fn casters_meet_the_cascade_squares_across_the_light() {
        let profile = crate::shadows::presets::profile(crate::shadows::Quality::Low);
        let camera = crate::camera::SceneCamera::new([0; 3]);
        let frame = ShadowFrame::new(
            profile,
            &camera.view_entries(),
            &camera.projection(),
            [0.0; 3],
            [-0.5, -0.6, -0.5],
        );
        let fit = &frame.fits[frame.fits.len() - 1];
        let b = frame.basis;
        let at = |x: f64, y: f64, z: f64| b.right * x + b.up * y + b.forward * z;
        let mid = (fit.depth[0] + fit.depth[1]) / 2.0;
        let inside = at(fit.centre[0], fit.centre[1], mid);
        assert!(meets_cascades(&frame, inside.to_array(), 0.0));
        let sunward = at(fit.centre[0], fit.centre[1], fit.depth[0] + 1.0);
        assert!(meets_cascades(&frame, sunward.to_array(), 0.0));
        let beyond = at(fit.centre[0], fit.centre[1], fit.depth[1] + 10.0);
        let far_side = frame
            .fits
            .iter()
            .map(|f| f.depth[1])
            .fold(f64::MIN, f64::max);
        if fit.depth[1] >= far_side {
            assert!(!meets_cascades(&frame, beyond.to_array(), 0.0));
        }
        let behind_near = at(fit.centre[0], fit.centre[1], -1.0e7);
        assert!(!meets_cascades(&frame, behind_near.to_array(), 0.0));
        let widest = frame
            .fits
            .iter()
            .max_by(|a, b| (a.centre[0] + a.radius).total_cmp(&(b.centre[0] + b.radius)))
            .expect("a cascade");
        let beside = at(
            widest.centre[0] + widest.radius + 100.0,
            widest.centre[1],
            (widest.depth[0] + widest.depth[1]) / 2.0,
        );
        assert!(!meets_cascades(&frame, beside.to_array(), 0.0));
        assert!(meets_cascades(&frame, beside.to_array(), 200.0));
        let far = at(fit.centre[0] + 1.0e6, fit.centre[1], mid);
        assert!(!meets_cascades(&frame, far.to_array(), DEFAULT_MARGIN));
    }

    #[test]
    fn margins_come_from_the_bounds() {
        assert_eq!(margin(None), DEFAULT_MARGIN);
        assert_eq!(margin(Some([-100, -300, -50, 200, 600, 100])), 600.0);
    }
}
