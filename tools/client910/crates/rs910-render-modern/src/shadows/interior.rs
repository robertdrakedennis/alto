//! Interior sun shadows (renderer plan §4(m)): the geometry
//! that roof removal hides still casts into the sun's cascades.
//!
//! # Why
//!
//! The classic renderer darkens a floor under a roof or an upper storey
//! without any light model: every upper level's solid tiles and every
//! hard-shadow loc stamp a mask along the sun onto the levels below
//! (`rs910_model::hardshadow`), and the floor bakes it in (a fixed shade
//! step and the hard-shadow texture). Those stamps are built once per scene,
//! so a roof hidden by the roof removal keeps darkening the rooms under it.
//! The modern backend lights the ground with the cascades (M3) over the
//! frame's draw list (M10's terrain carries no baked shade), and the plan
//! leaves the hidden roofs, upper walls and upper floors out, so the sun lit
//! the rooms as if they had no roof.
//!
//! # What the modern client does
//!
//! See the plan §4(m) for the evidence. In short: its shadow pass draws a
//! queue of its own, gathered for the cascades' volumes, not the camera's;
//! its terrain vertices carry no occlusion term (position, normal, colour,
//! material slots and weights only), and its terrain and model shaders read
//! no occlusion input besides SSAO and the shadow map. So the darkening
//! indoors has to come from the casters: this module adds the roof-hidden
//! geometry to the caster pass only.
//!
//! # What this module computes
//!
//! [`roof_hidden`]: the live scene's entities and the floor tiles that this
//! frame's roof removal hid, by the classic renderer's own tests over the
//! stamps (`LiveScene::roof_removal`): an entity whose level is at or above the
//! first hidden level and whose anchor tile (the first footprint tile in
//! the draw window) carries the hiding stamp; a floor tile of draw level
//! `L` whose owning plane is hidden the same way. The frustum and the
//! occluders are not applied (a caster need not be in view). Nothing else
//! changes: the colour passes draw the plan as before.
//!
//! # Merged with the caster gather
//!
//! The caster gather (`shadows::casters`) gathers the plan-culled locs per
//! cascade but stops at the plan's highest drawn level. It admits an entity
//! above that cap when [`RoofHidden::hides`] it, with its cascades from
//! `cascades_met` as for any other loc, and gathers each entity once (an
//! earlier version drew the hidden entities into every cascade from a loop of
//! its own, and those at or below the cap a second time through the gather).
//! The terrain half (the hidden tiles' index buffers,
//! `crate::frame::interior`) stays here, as the gather covers no floors.

use crate::draw::FloorSelection;
use crate::scene::EntityRef;
use crate::scene_snapshot::SceneSnapshot;

/// What this frame's roof removal hid (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoofHidden {
    /// `live.entities` ids, in the live scene's order.
    pub entities: Vec<usize>,
    /// Per draw level: the hidden floor tiles as a selection over the
    /// plan's draw window (`None`: none hidden on that level).
    pub tiles: Vec<Option<FloorSelection>>,
}

impl RoofHidden {
    /// Whether live entity `id` is roof-hidden (the ids are ascending).
    #[must_use]
    pub fn hides(&self, id: usize) -> bool {
        self.entities.binary_search(&id).is_ok()
    }

    /// The hidden floor tiles over every level.
    #[must_use]
    pub fn tile_count(&self) -> usize {
        self.tiles
            .iter()
            .flatten()
            .map(|s| s.mask.iter().flatten().filter(|&&v| v).count())
            .sum()
    }
}

/// The entities and floor tiles `snapshot`'s roof removal hid this frame
/// (empty without a live scene, roof stamps or a floor window).
#[must_use]
pub fn roof_hidden(snapshot: &SceneSnapshot<'_>) -> RoofHidden {
    let mut out = RoofHidden::default();
    if snapshot.blackout {
        return out;
    }
    let (Some(live), Some(scene)) = (snapshot.live_frame(), snapshot.scene) else {
        return out;
    };
    let Some(roof) = live.roof_removal() else {
        return out;
    };
    let (stamps, stamp, first_hidden) = (roof.stamps, roof.stamp, roof.first_plane);
    if first_hidden >= scene.max_level {
        return out;
    }
    // The draw window (`Scene.draw` :1037-1046): the plan's floor origin
    // is `eye - distance`.
    let Some(window) = live.draw.plan.floors.first() else {
        return out;
    };
    let [ox, oz] = window.origin;
    let d = window.distance;
    let (min_x, min_z) = (ox.max(0), oz.max(0));
    let (max_x, max_z) = (
        (ox + 2 * d).min(scene.max_x as i32),
        (oz + 2 * d).min(scene.max_z as i32),
    );
    let stamp_at = |plane: usize, x: i32, z: i32| -> Option<i8> {
        stamps
            .get(plane)?
            .get(usize::try_from(x).ok()?)?
            .get(usize::try_from(z).ok()?)
            .copied()
    };
    // Entities (`Scene.draw` :1283-1300): the anchor tile decides.
    for (id, e) in live.entities.iter().enumerate() {
        if matches!(e.source, EntityRef::Temporary(_)) {
            continue;
        }
        if e.level < first_hidden as i32 || e.occlude_level >= scene.max_level as i32 {
            continue;
        }
        let anchor = (e.tiles[0]..=e.tiles[1])
            .flat_map(|x| (e.tiles[2]..=e.tiles[3]).map(move |z| (x, z)))
            .find(|&(x, z)| x >= min_x && x < max_x && z >= min_z && z < max_z);
        let Some((x, z)) = anchor else { continue };
        if stamp_at(e.level as usize, x, z) == Some(stamp) {
            out.entities.push(id);
        }
    }
    // Floor tiles (`Scene.draw`'s per-level selection, :1147-1170): the
    // highest plane at or below the level whose tile draws at it owns it.
    let n = (2 * d + 2) as usize;
    out.tiles = vec![None; scene.max_level];
    for level in first_hidden..scene.max_level {
        let mut mask = vec![vec![false; n]; n];
        let mut any = false;
        for x in 0..=2 * d {
            for z in 0..=2 * d {
                let (tx, tz) = (ox + x, oz + z);
                if tx < 0 || tz < 0 || tx >= scene.max_x as i32 || tz >= scene.max_z as i32 {
                    continue;
                }
                let owner = (0..=level).rev().find(|&plane| {
                    scene
                        .tile(plane, tx as usize, tz as usize)
                        .is_some_and(|t| t.level as usize == level)
                });
                if let Some(plane) = owner {
                    if plane >= first_hidden && stamp_at(plane, tx, tz) == Some(stamp) {
                        mask[x as usize][z as usize] = true;
                        any = true;
                    }
                }
            }
        }
        if any {
            out.tiles[level] = Some(FloorSelection {
                whole: false,
                origin: window.origin,
                distance: d,
                mask,
            });
        }
    }
    out
}

/// `CLIENT910_MODERN_CHECK`: whether `hidden` is disjoint from what the plan
/// drew (a roof-hidden entity or tile is never in the plan), with a report.
#[must_use]
pub fn check(snapshot: &SceneSnapshot<'_>, hidden: &RoofHidden) -> (bool, String) {
    let Some(live) = snapshot.live_frame() else {
        return (true, "no live scene".into());
    };
    let plan = &live.draw.plan;
    let drawn_entities = hidden
        .entities
        .iter()
        .filter(|id| plan.opaque.contains(id) || plan.transparent.contains(id))
        .count();
    let mut drawn_tiles = 0;
    for (level, sel) in hidden.tiles.iter().enumerate() {
        let (Some(sel), Some(shown)) = (sel, plan.floors.get(level)) else {
            continue;
        };
        for (x, column) in sel.mask.iter().enumerate() {
            for (z, &h) in column.iter().enumerate() {
                let (tx, tz) = (sel.origin[0] + x as i32, sel.origin[1] + z as i32);
                if h && shown.visible(tx, tz) {
                    drawn_tiles += 1;
                }
            }
        }
    }
    (
        drawn_entities == 0 && drawn_tiles == 0,
        format!(
            "{} roof-hidden casters ({} also drawn), {} hidden floor tiles ({} also drawn)",
            hidden.entities.len(),
            drawn_entities,
            hidden.tile_count(),
            drawn_tiles
        ),
    )
}
