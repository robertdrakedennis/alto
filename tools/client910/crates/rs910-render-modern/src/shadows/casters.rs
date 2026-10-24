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
//! plan culled. Every installed storey is tested against the light volume;
//! the colour plan and roof removal never limit the caster levels. Planned
//! entities are skipped here so every loc is gathered once.

use glam::DVec3;

use crate::models::draw_list::EntityDraw;
use crate::scene::EntityRef;
use crate::scene_snapshot::SceneSnapshot;
use crate::shadows::interior::RoofHidden;
use crate::shadows::SunlightShadows;

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
pub fn cascades_met(frame: &SunlightShadows, p: [f64; 3], margin: f64) -> u8 {
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
pub fn visible_cascades(snapshot: &SceneSnapshot<'_>, frame: &SunlightShadows, id: usize) -> u8 {
    entity_cascades(
        frame,
        snapshot.live_frame().and_then(|live| live.entities.get(id)),
    )
}

pub(crate) fn entity_cascades(
    frame: &SunlightShadows,
    entity: Option<&crate::draw::DrawEntity>,
) -> u8 {
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
pub fn meets_cascades(frame: &SunlightShadows, p: [f64; 3], margin: f64) -> bool {
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
    frame: &SunlightShadows,
    hidden: Option<&RoofHidden>,
) -> Vec<(EntityDraw<'a>, u8, bool)> {
    gather(snapshot, frame, hidden, None)
}

pub(crate) fn off_screen_parallel<'a>(
    snapshot: &SceneSnapshot<'a>,
    frame: &SunlightShadows,
    hidden: Option<&RoofHidden>,
    jobs: &dyn CasterJobs,
) -> Vec<(EntityDraw<'a>, u8, bool)> {
    gather(snapshot, frame, hidden, Some(jobs))
}

/// Coarse job batches amortize the pool's ordered result slots.
const CASTERS_PER_JOB: usize = 32;

fn gather<'a>(
    snapshot: &SceneSnapshot<'a>,
    frame: &SunlightShadows,
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

    for &id in plan.opaque.iter().chain(&plan.transparent) {
        if let Some(slot) = planned.get_mut(id) {
            *slot = true;
        }
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
            // Colour visibility and roof removal never limit the caster planes.
            // Only the light volume decides whether an installed loc can shade it.
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
