//! The modern renderer's frame draw list (renderer plan M1): what one
//! [`SceneSnapshot`] asks to be drawn, in the order the faithful backends
//! draw it, as CPU data (no GPU types), so the structural tests can compare
//! it with the faithful backend's lists.
//!
//! Order (as the faithful GPU backend's `SceneMeshes::frame` walks
//! `live.draw.plan`): the plan's opaque
//! entities, every level's floor with that level's tile selection, then the
//! plan's transparent entities. Per entity (`SceneMeshes::mesh_for`): a
//! player is its spot
//! shadow, up to nine hint arrows and its body; a transient
//! temporary (NPC bodies, spot anims, projectiles) and a static or dynamic
//! loc are one model.
//!
//! Matrices are row-major 4x4 entries in scene-local fine units
//! (the snapshot's `floor_base` subtracted, like the software toolkit's
//! scene pass): a loc's scale-rotate-translate transform with its entity
//! position, a temporary's translation, a player's actor matrices.
//!
//! The draw list does not own or change anything: the NXT pass's
//! differences from the faithful look (lighting, materials, sky, HDR) are in
//! how it draws these items, never in which items it draws.

use crate::actor_matrix::Matrix;
use crate::draw::FloorSelection;
use crate::floor::FloorGeometry;
use crate::gpumodel::GpuModel;
use crate::scene::EntityRef;
use crate::scene_snapshot::{EntityKey, SceneSnapshot};

/// What an entity draw is (`SceneMeshes::mesh_for`'s slots).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A loc, dynamic loc or transient temporary: its one model.
    Model,
    /// A player's spot shadow (drawn without depth writes).
    SpotShadow,
    /// One of the local player's hint arrows (without depth writes).
    HintArrow,
    /// A player's body.
    Body,
}

/// One model draw of the frame.
pub struct EntityDraw<'a> {
    /// The `live.entities` slot (the plan's id).
    pub id: usize,
    pub kind: Kind,
    pub model: &'a GpuModel,
    /// Scene-local model matrix, row-major entries.
    pub matrix: [f32; 16],
    /// Depth writes: off for spot shadows and
    /// hint arrows.
    pub depth_write: bool,
    /// The cache key of a model that lives while the scene is installed
    /// (static and dynamic locs, [`SceneSnapshot::entity_key`]); `None` for
    /// models posed this frame (temporaries, players), which the renderer
    /// uploads per frame.
    pub key: Option<EntityKey>,
}

/// One level's floor draw.
pub struct FloorDraw<'a> {
    pub level: usize,
    pub geometry: &'a FloorGeometry,
    /// `Scene.draw`'s tile selection of this level this frame.
    pub selection: &'a FloorSelection,
}

/// One frame's draw list (see the module docs).
#[derive(Default)]
pub struct DrawList<'a> {
    pub opaque: Vec<EntityDraw<'a>>,
    pub floors: Vec<FloorDraw<'a>>,
    pub transparent: Vec<EntityDraw<'a>>,
}

/// `Matrix4x4.setToMatrix4x3` of a translation-only `Matrix4x3`.
fn translation([x, y, z]: [f32; 3]) -> [f32; 16] {
    [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., x, y, z, 1.]
}

impl<'a> DrawList<'a> {
    /// The draw list of `snapshot` (empty without a live scene, a scene
    /// graph, or under the scene blackout).
    #[must_use]
    pub fn build(snapshot: &SceneSnapshot<'a>) -> Self {
        let mut list = Self::default();
        if snapshot.blackout {
            return list;
        }
        let (Some(live), Some(scene)) = (snapshot.live_frame(), snapshot.scene) else {
            return list;
        };
        let plan = &live.draw.plan;
        for &id in &plan.opaque {
            entity(snapshot, live, scene, id, &mut list.opaque);
        }
        for (level, selection) in plan.floors.iter().enumerate() {
            if let Some(Some(geometry)) = snapshot.floors.get(level) {
                if geometry.vertex_count > 0 {
                    list.floors.push(FloorDraw {
                        level,
                        geometry,
                        selection,
                    });
                }
            }
        }
        for &id in &plan.transparent {
            entity(snapshot, live, scene, id, &mut list.transparent);
        }
        list
    }

    /// Every loc of the installed scene with a model, visible or not (the
    /// static and dynamic locs of `live.entities`, no temporaries or
    /// players), as draws in slot order: what the light-probe captures
    /// draw (renderer plan M6, `crate::frame::probes`).
    #[must_use]
    pub fn scene_locs(snapshot: &SceneSnapshot<'a>) -> Vec<EntityDraw<'a>> {
        let mut out = Vec::new();
        let (Some(live), Some(scene)) = (snapshot.live_frame(), snapshot.scene) else {
            return out;
        };
        for (id, e) in live.entities.iter().enumerate() {
            if !matches!(e.source, EntityRef::Temporary(_)) {
                entity(snapshot, live, scene, id, &mut out);
            }
        }
        out
    }

    /// The list's structure, for comparison with another backend's: the
    /// entity draws as `(id, kind)` and each floor's batches as `(level,
    /// material, index count)` over the selected tiles
    /// (`FloorBatch::build_indices`, what the faithful floor uploads for a
    /// selection, `FloorMesh::select_tiles`).
    #[must_use]
    pub fn summary(&self) -> Summary {
        let ids = |list: &[EntityDraw<'_>]| list.iter().map(|d| (d.id, d.kind)).collect();
        let mut floors = Vec::new();
        for floor in &self.floors {
            let g = floor.geometry;
            let tiles = floor.selection.tiles(g.tiles_x, g.tiles_z);
            for batch in &g.batches {
                let (indices, _, _) = batch.build_indices(g, &tiles);
                if !indices.is_empty() {
                    floors.push((floor.level, batch.material, indices.len()));
                }
            }
        }
        Summary {
            opaque: ids(&self.opaque),
            transparent: ids(&self.transparent),
            floors,
        }
    }
}

/// [`DrawList::summary`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub opaque: Vec<(usize, Kind)>,
    pub transparent: Vec<(usize, Kind)>,
    /// `(level, material, index count)` per non-empty floor batch.
    pub floors: Vec<(usize, i32, usize)>,
}

/// The draws of plan entity `id` (`SceneMeshes::mesh_for`'s order; also
/// the off-screen shadow casters' draws, `shadows::casters`; the
/// roof-hidden casters' draws, `shadows::interior`).
pub(crate) fn entity<'a>(
    snapshot: &SceneSnapshot<'a>,
    live: rs910_scene::scene_snapshot::owned::LiveFrame<'a>,
    scene: &'a crate::scene::Scene,
    id: usize,
    out: &mut Vec<EntityDraw<'a>>,
) {
    let Some(e) = live.entities.get(id) else {
        return;
    };
    if let EntityRef::Temporary(index) = e.source {
        let Some(t) = scene.temporary.get(index) else {
            return;
        };
        if t.transient {
            if let Some(model) = snapshot.model(e.source) {
                out.push(EntityDraw {
                    id,
                    kind: if t.spot_shadow {
                        Kind::SpotShadow
                    } else {
                        Kind::Model
                    },
                    model,
                    matrix: translation(t.position),
                    depth_write: !t.spot_shadow,
                    key: None,
                });
            }
            return;
        }
        // The player owner's draw of this player
        // (`fields[3]`, the player index the faithful owner keys by).
        let Some(entry) = snapshot
            .players
            .and_then(|p| p.player_draw(e.loc_id as usize))
        else {
            return;
        };
        let matrix = |m: &Matrix| m.entries();
        if let Some(shadow) = entry.shadow.as_ref().filter(|s| s.visible) {
            out.push(EntityDraw {
                id,
                kind: Kind::SpotShadow,
                model: shadow.model,
                matrix: matrix(shadow.matrix),
                depth_write: false,
                key: None,
            });
        }
        for arrow in entry.hint_arrows.iter().take(9).filter(|a| a.visible) {
            out.push(EntityDraw {
                id,
                kind: Kind::HintArrow,
                model: arrow.model,
                matrix: matrix(arrow.matrix),
                depth_write: false,
                key: None,
            });
        }
        if entry.visible {
            if let Some(model) = snapshot.model(e.source) {
                out.push(EntityDraw {
                    id,
                    kind: Kind::Body,
                    model,
                    matrix: matrix(entry.matrix),
                    depth_write: true,
                    key: None,
                });
            }
        }
        return;
    }
    let Some(model) = snapshot.model(e.source) else {
        return;
    };
    let mut pos = [e.x as f32, e.y as f32, e.z as f32];
    if let EntityRef::WallDecor(i) = e.source {
        // A static wall decoration is translated by its x and z offsets.
        pos[0] += scene.wall_decors[i].offset_x as f32;
        pos[2] += scene.wall_decors[i].offset_z as f32;
    }
    let matrix = match crate::scene::entity_srt(scene, e.source) {
        Some(s) => Matrix::srt(s.rot, s.scale, pos).entries(),
        None => translation(pos),
    };
    out.push(EntityDraw {
        id,
        kind: Kind::Model,
        model,
        matrix,
        depth_write: true,
        key: snapshot.entity_key(id),
    });
}
