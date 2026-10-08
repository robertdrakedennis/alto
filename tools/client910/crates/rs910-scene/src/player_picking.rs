//! Bridge from the last scene draw to the menu's per-cycle pick tests of players, NPCs, locs
//! and objects.
use crate::{
    camera,
    gpumodel::GpuModel,
    scene_player_pick::{ModelBounds, PickablePlayer, ScreenBounds},
};

#[derive(Clone)]
pub struct Frame {
    pub picks: Vec<PickablePlayer>,
    /// NPC body picks share the same projection/capsule math. The `pid` field
    /// is the live NPC slot for these entries; generation is unused.
    pub npc_picks: Vec<PickablePlayer>,
    /// Drawn, active location entities; inactive pickables are released.
    pub loc_picks: Vec<LocPick>,
    /// Drawn ground object stacks, one entry per stack.
    pub obj_picks: Vec<ObjPick>,
    /// The pickable list in test order: projected depth descending, later draws first on equal
    /// depth.
    pub order: Vec<PickRef>,
    pub view: [f32; 16],
    pub vp: [f32; 16],
    pub projection: [f32; 16],
    pub viewport: [i32; 4],
    pub screen: [f32; 4],
    pub base: [i32; 2],
    pub terrain_generation: u64,
    /// The height of the scene locations bound to open active-loc subinterfaces, keyed
    /// `(level, layer, x, z)` in scene tiles.
    pub loc_heights: std::collections::BTreeMap<(i32, i32, i32, i32), i32>,
    /// The height of the object stacks bound to open active-obj subinterfaces, keyed
    /// `(level, x, z)`.
    pub obj_heights: std::collections::BTreeMap<(i32, i32, i32), i32>,
}
impl Frame {
    pub fn new(
        camera: &camera::SceneCamera,
        base: [i32; 2],
        terrain_generation: u64,
        viewport: [i32; 4],
    ) -> Self {
        let mut local = camera.clone();
        local.target[0] -= base[0];
        local.target[2] -= base[1];
        let view = local.view_entries();
        let mut projection = local.projection();
        // The projection used for picking stays unflipped; only the shader copy is flipped.
        camera::glx_flip_y(&mut projection);
        let screen = [
            viewport[0] as f32 + viewport[2] as f32 / 2.,
            viewport[1] as f32 + viewport[3] as f32 / 2.,
            viewport[2] as f32 / 2.,
            viewport[3] as f32 / 2.,
        ];
        Self {
            picks: vec![],
            npc_picks: vec![],
            loc_picks: vec![],
            obj_picks: vec![],
            order: vec![],
            view,
            vp: camera::multiply(&view, &projection),
            projection,
            viewport,
            screen,
            base,
            terrain_generation,
            loc_heights: Default::default(),
            obj_heights: Default::default(),
        }
    }
    pub fn matches(&self, input: &crate::cam2_scene::SceneInput<'_>, viewport: [i32; 4]) -> bool {
        self.base == input.base
            && self.terrain_generation == input.terrain_generation
            && self.viewport == viewport
    }
}
/// One entry of the ordered pickable list; indices address the frame vectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickRef {
    Player(usize),
    Npc(usize),
    Loc(usize),
    Obj(usize),
}

/// The loc a zone request put in a static scene slot. The slot's own scene
/// entity stays the map's loc (it is what a restore brings back), so the pick
/// list reads what the slot holds now from here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotOccupant {
    pub id: i32,
    pub shape: i32,
    pub angle: i32,
}

/// The changed static slots by their scene entity.
pub type SlotOccupants = std::collections::HashMap<crate::scene::EntityRef, SlotOccupant>;

/// A drawn static or dynamic location pickable.
#[derive(Clone, Debug)]
pub struct LocPick {
    /// Scene owner of the drawn model, re-read by the per-cycle hit test.
    pub source: crate::scene::EntityRef,
    /// Plane the entity is on.
    pub level: i32,
    /// Loc id, shape and angle.
    pub id: i32,
    pub shape: i32,
    pub angle: i32,
    /// Minimum scene tile of a primary-layer entity, else the position shifted right by 9.
    pub tile: [i32; 2],
    /// The capsule written by the draw (model or clickbox capsule).
    pub capsule: ScreenBounds,
    /// The entity-matrix translation used by `pick`; wall decorations draw with their offset but
    /// pick without it.
    pub pick_origin: [f32; 3],
    /// The loc type's clickbox.
    pub clickbox: Option<[i32; 6]>,
    /// The rotation/scale/translation transform of the entity
    /// ([`crate::scene::entity_srt`], or a changed loc's
    /// [`crate::scene::LocSrtPick::srt`]), part of its draw and pick matrix.
    pub srt: Option<crate::map::LocSrt>,
    /// The drawn model of a transient loc (one the redraw added to the scene),
    /// kept with the pick: the transient entities leave the scene before the
    /// logic cycles that test the mouse against this pick again.
    pub transient_model: Option<TransientPickModel>,
    /// Latest hit test result for the current mouse.
    pub hit: bool,
}

/// The model a transient loc is picked through, with the bounds it had when it
/// was drawn.
#[derive(Clone, Debug)]
pub struct TransientPickModel {
    pub model: std::sync::Arc<GpuModel>,
    pub bounds: ModelBounds,
}

/// One drawn ground object stack: a single pickable per stack whose pick tests the primary,
/// secondary, then tertiary model with the box-only model test, so each drawn model's
/// draw-time box suffices. `picks` holds `(rank, box)` in that test order.
#[derive(Clone, Debug)]
pub struct ObjPick {
    pub level: i32,
    pub tile: [i32; 2],
    pub picks: Vec<(usize, PickablePlayer)>,
}

fn translation_mvp(origin: [f32; 3], vp: &[f32; 16]) -> [f32; 16] {
    let matrix = crate::actor_matrix::Matrix::actor([0.0, 0.0, 0.0, 1.0], origin, 0.0);
    camera::multiply(&matrix.entries(), vp)
}

/// The entity matrix at `origin` times the view-projection: a loc's rotation/scale transform
/// is part of the matrix its draw and `pick` use.
fn entity_mvp(origin: [f32; 3], srt: Option<crate::map::LocSrt>, vp: &[f32; 16]) -> [f32; 16] {
    match srt {
        Some(s) => {
            let matrix = crate::actor_matrix::Matrix::srt(s.rot, s.scale, origin);
            camera::multiply(&matrix.entries(), vp)
        }
        None => translation_mvp(origin, vp),
    }
}

impl Frame {
    /// Collect the pickables of the last planned draw: every drawn, active location and object rank
    /// records the capsule its draw wrote, then all pickables (players and
    /// NPCs captured by their owners) are ordered by projected depth, later
    /// draws first on equal depth. Only entries with an enabled capsule are
    /// inserted.
    ///
    /// Static scene locs keep untransformed models; a changed loc's model
    /// carries its rotation/scale/translation baked into the vertices
    /// (`GpuModel::apply_srt`) and keeps the untransformed bounds
    /// ([`crate::scene::LocSrtPick`]). Both pick the way they draw: the untransformed
    /// bounds/clickbox through the SRT entity matrix (`entity_mvp`). A ground stack's models
    /// are drawn as one transient per object; they join one `ObjPick` at the first model's
    /// place in the pick order. A static slot a zone request changed picks as the loc it holds
    /// now (`occupants`), with that loc's clickbox and active flag.
    pub fn collect_scene(
        &mut self,
        scene: &mut crate::scene::Scene,
        live: &crate::live_scene::LiveScene,
        locs: Option<&crate::config::LocStore>,
        occupants: &SlotOccupants,
    ) {
        use crate::scene::{EntityRef, TemporaryPick};
        self.loc_picks.clear();
        self.obj_picks.clear();
        self.order.clear();
        let plan = &live.draw.plan;
        let drawn: std::collections::HashSet<usize> = plan
            .opaque
            .iter()
            .chain(&plan.transparent)
            .copied()
            .collect();
        let clickbox = |id: i32| {
            locs.and_then(|store| u32::try_from(id).ok().and_then(|id| store.get(id)))
                .and_then(|loc| loc.clickbox)
        };
        let loc_active = |id: i32| {
            locs.and_then(|store| {
                let loc = u32::try_from(id).ok().and_then(|id| store.get(id))?;
                Some(loc.active_for(store.allow_members.get()) != 0)
            })
            .unwrap_or(false)
        };
        let mut entries: Vec<(i32, usize, PickRef)> = Vec::new();
        for (seq, &id) in plan.dispatched.iter().enumerate() {
            let depth = plan.dispatched_depth.get(seq).copied().unwrap_or(0);
            let source = live.entities[id].source;
            let visible = drawn.contains(&id);
            // (level, id, shape, angle, tile, pick origin, draw origin, active)
            let located = match source {
                EntityRef::Temporary(index) => {
                    let t = &scene.temporary[index];
                    if let Some(npc) = t.npc_index {
                        if visible {
                            for (k, pick) in self.npc_picks.iter().enumerate() {
                                if pick.id.pid == npc as i32 {
                                    entries.push((depth, seq, PickRef::Npc(k)));
                                }
                            }
                        }
                        continue;
                    }
                    match t.pick {
                        Some(TemporaryPick::Obj { tile, rank }) => {
                            if !visible {
                                continue;
                            }
                            let (level, position) = (t.level, t.position);
                            let Some(model) = scene.temporary[index].model.as_mut() else {
                                continue;
                            };
                            let raw = bounds(model);
                            let mvp = translation_mvp(position, &self.vp);
                            let capsule = crate::scene_player_pick::screen_bounds(
                                raw,
                                model.horizontal_radius(),
                                mvp,
                                self.projection,
                                self.screen,
                            );
                            if !capsule.enabled {
                                continue;
                            }
                            let pick = PickablePlayer {
                                id: crate::scene_player_pick::PlayerPickId {
                                    pid: -1,
                                    generation: 0,
                                },
                                bounds: crate::scene_player_pick::pick_bounds(raw, 0),
                                screen_bounds: Some(capsule),
                                projected_depth: depth,
                                matrix: mvp,
                                active: true,
                            };
                            if let Some(stack) = self
                                .obj_picks
                                .iter_mut()
                                .find(|o| o.level == level && o.tile == tile)
                            {
                                stack.picks.push((rank, pick));
                                stack.picks.sort_by_key(|&(rank, _)| rank);
                                continue;
                            }
                            entries.push((depth, seq, PickRef::Obj(self.obj_picks.len())));
                            self.obj_picks.push(ObjPick {
                                level,
                                tile,
                                picks: vec![(rank, pick)],
                            });
                            continue;
                        }
                        Some(TemporaryPick::Loc {
                            id,
                            shape,
                            angle,
                            tile,
                            srt,
                        }) => (
                            t.level,
                            id,
                            shape,
                            angle,
                            tile,
                            t.position,
                            t.position,
                            loc_active(id),
                            srt,
                        ),
                        None => {
                            if !t.transient && t.location_key.is_none() && t.player != usize::MAX {
                                for (k, pick) in self.picks.iter().enumerate() {
                                    if pick.id.pid == t.player as i32 {
                                        entries.push((depth, seq, PickRef::Player(k)));
                                    }
                                }
                            }
                            continue;
                        }
                    }
                }
                EntityRef::Scenery(i) => {
                    let e = &scene.scenery[i];
                    let origin = [e.x as f32, e.y as f32, e.z as f32];
                    (
                        e.level,
                        e.loc_id as i32,
                        e.shape,
                        e.angle,
                        [e.min_tx, e.min_tz],
                        origin,
                        origin,
                        e.active,
                        None,
                    )
                }
                EntityRef::Wall(i) => {
                    let e = &scene.walls[i];
                    let origin = [e.x as f32, e.y as f32, e.z as f32];
                    (
                        e.level,
                        e.loc_id as i32,
                        e.shape,
                        e.angle,
                        [e.x >> 9, e.z >> 9],
                        origin,
                        origin,
                        e.active,
                        None,
                    )
                }
                EntityRef::WallDecor(i) => {
                    let e = &scene.wall_decors[i];
                    (
                        e.level,
                        e.loc_id as i32,
                        e.shape,
                        e.angle,
                        [e.x >> 9, e.z >> 9],
                        [e.x as f32, e.y as f32, e.z as f32],
                        [
                            (e.x + e.offset_x) as f32,
                            e.y as f32,
                            (e.z + e.offset_z) as f32,
                        ],
                        e.active,
                        None,
                    )
                }
                EntityRef::GroundDecor(i) => {
                    let e = &scene.ground_decors[i];
                    let origin = [e.x as f32, e.y as f32, e.z as f32];
                    (
                        e.level,
                        e.loc_id as i32,
                        22,
                        e.angle,
                        [e.x >> 9, e.z >> 9],
                        origin,
                        origin,
                        e.active,
                        None,
                    )
                }
            };
            let (level, loc, shape, angle, tile, pick_origin, draw_origin, active, srt) = located;
            // A changed static slot is the loc the server put there.
            let (loc, shape, angle, active) = match occupants.get(&source) {
                Some(o) => (o.id, o.shape, o.angle, loc_active(o.id)),
                None => (loc, shape, angle, active),
            };
            // Inactive pickables are released.
            if !visible || !active {
                continue;
            }
            // The SRT entity matrix. A static loc's translation is already in its position;
            // a changed loc's position is the tile, so its translation joins
            // the origin here.
            let (srt, changed) = match srt {
                Some(p) => (Some(p.srt), Some(p)),
                None => (crate::scene::entity_srt(scene, source), None),
            };
            let offset = changed.map_or([0.0; 3], |p| p.srt.trans);
            let pick_origin: [f32; 3] = std::array::from_fn(|i| pick_origin[i] + offset[i]);
            let draw_origin: [f32; 3] = std::array::from_fn(|i| draw_origin[i] + offset[i]);
            let draw_mvp = entity_mvp(draw_origin, srt, &self.vp);
            let clickbox = clickbox(loc);
            let capsule = if let Some(c) = clickbox {
                // The clickbox cuboid replaces the model bounds for the draw-time capsule.
                crate::scene_player_pick::cuboid_screen_bounds(
                    c.map(|v| v as f32),
                    draw_mvp,
                    self.projection,
                    self.screen,
                )
            } else if let Some(changed) = changed {
                // The baked model's untransformed bounds.
                let [x0, y0, z0, x1, y1, z1] = changed.bounds;
                crate::scene_player_pick::screen_bounds(
                    ModelBounds {
                        min: [x0, y0, z0],
                        max: [x1, y1, z1],
                    },
                    changed.horizontal_radius,
                    draw_mvp,
                    self.projection,
                    self.screen,
                )
            } else {
                let Some(model) = crate::dynamic_scene::model_mut(scene, source).as_mut() else {
                    continue;
                };
                crate::scene_player_pick::screen_bounds(
                    bounds(model),
                    model.horizontal_radius(),
                    draw_mvp,
                    self.projection,
                    self.screen,
                )
            };
            if !capsule.enabled {
                continue;
            }
            // A transient loc without a clickbox is picked through its model.
            let transient_model = match (source, clickbox) {
                (EntityRef::Temporary(index), None) => scene
                    .temporary
                    .get_mut(index)
                    .and_then(|t| t.model.as_mut())
                    .map(|model| TransientPickModel {
                        bounds: bounds(model),
                        model: std::sync::Arc::new(model.clone()),
                    }),
                _ => None,
            };
            entries.push((depth, seq, PickRef::Loc(self.loc_picks.len())));
            self.loc_picks.push(LocPick {
                source,
                level,
                id: loc,
                shape,
                angle,
                tile,
                capsule,
                pick_origin,
                clickbox,
                srt,
                transient_model,
                hit: false,
            });
        }
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        self.order = entries.into_iter().map(|entry| entry.2).collect();
    }

    /// The height the loc/obj overlay-height commands read for each active binding. `locs` are `(level, layer, x, z)`
    /// scene-tile keys, `objs` are `(level, x, z)`.
    pub fn collect_active_heights(
        &mut self,
        scene: &mut crate::scene::Scene,
        locs: &[(i32, i32, i32, i32)],
        objs: &[(i32, i32, i32)],
    ) {
        self.loc_heights.clear();
        self.obj_heights.clear();
        for &key in locs {
            if let Some(height) = loc_height(scene, key) {
                self.loc_heights.insert(key, height);
            }
        }
        for &key in objs {
            self.obj_heights.insert(key, obj_height(scene, key));
        }
    }

    /// Hit test every location against the current mouse: the draw capsule, then the clickbox
    /// cuboid pick, otherwise the model pick (not box-only, no shift) on the drawn model.
    pub fn refresh_locs(&mut self, scene: &mut crate::scene::Scene, mouse: [i32; 2]) {
        let (vp, screen) = (self.vp, self.screen);
        for pick in &mut self.loc_picks {
            pick.hit = false;
            if !crate::scene_player_pick::capsule_hit(pick.capsule, mouse, [0, 0]) {
                continue;
            }
            let mvp = entity_mvp(pick.pick_origin, pick.srt, &vp);
            pick.hit = match pick.clickbox {
                Some(c) => {
                    crate::scene_player_pick::cuboid_pick(c.map(|v| v as f32), &mvp, screen, mouse)
                }
                None => match pick.source {
                    // A transient loc's model is stored baked (`LocSrtPick`):
                    // the untransformed model through the SRT matrix is the
                    // baked one at the tile origin. The model is the one the
                    // redraw drew, kept with the pick.
                    crate::scene::EntityRef::Temporary(_) => {
                        let trans = pick.srt.map_or([0.0; 3], |s| s.trans);
                        let origin: [f32; 3] =
                            std::array::from_fn(|i| pick.pick_origin[i] - trans[i]);
                        pick.transient_model.as_ref().is_some_and(|kept| {
                            crate::scene_player_pick::model_pick_within(
                                &kept.model,
                                kept.bounds,
                                &translation_mvp(origin, &vp),
                                screen,
                                mouse,
                                false,
                                0,
                            )
                        })
                    }
                    source => crate::dynamic_scene::model_mut(scene, source)
                        .as_mut()
                        .is_some_and(|model| {
                            crate::scene_player_pick::model_pick(
                                model, &mvp, screen, mouse, false, 0,
                            )
                        }),
                },
            };
        }
    }
}

pub fn bounds(model: &mut GpuModel) -> ModelBounds {
    ModelBounds {
        min: [model.min_x(), model.min_y(), model.min_z()],
        max: [model.max_x(), model.max_y(), model.max_z()],
    }
}

/// A loc's height is the negated overlay height: the model's minimum y, or 0 without a model.
/// A changed loc uses the retained replacement's model the same way.
fn loc_height(scene: &mut crate::scene::Scene, key: (i32, i32, i32, i32)) -> Option<i32> {
    use crate::scene::{EntityRef, PrimaryRef};
    let (level, layer, x, z) = key;
    let min_y = |model: &mut Option<GpuModel>| model.as_mut().map_or(0, GpuModel::min_y);
    if let Some(t) = scene
        .temporary
        .iter_mut()
        .find(|t| t.location_key == Some(key))
    {
        // A baked changed loc keeps its untransformed bounds.
        if let Some(crate::scene::TemporaryPick::Loc { srt: Some(srt), .. }) = t.pick {
            return Some(-srt.bounds[1]);
        }
        return Some(-min_y(&mut t.model));
    }
    if !(0..4).contains(&level) || x < 0 || z < 0 {
        return None;
    }
    let tile = scene.tile(level as usize, x as usize, z as usize)?;
    // Layer 0 wall, 1 wall decoration, 2 scenery entity, 3 ground decoration.
    let source = match layer {
        0 => EntityRef::Wall(tile.wall?),
        1 => EntityRef::WallDecor(tile.wall_decoration?),
        2 => tile.entities.iter().find_map(|e| match e {
            PrimaryRef::Scenery(i) => Some(EntityRef::Scenery(*i)),
            PrimaryRef::Temporary(_) => None,
        })?,
        3 => EntityRef::GroundDecor(tile.ground_decoration?),
        _ => return None,
    };
    Some(-min_y(crate::dynamic_scene::model_mut(scene, source)))
}

/// An object stack's height is its lift plus 10 (its overlay height is -10). The lift is the
/// lowest overlay height of the tile's raised primary entities, or the ground decoration's
/// height when that is higher.
fn obj_height(scene: &mut crate::scene::Scene, (level, x, z): (i32, i32, i32)) -> i32 {
    use crate::scene::PrimaryRef;
    let mut lift = 0;
    if (0..4).contains(&level) && x >= 0 && z >= 0 {
        if let Some(tile) = scene.tile(level as usize, x as usize, z as usize) {
            let raised: Vec<usize> = tile
                .entities
                .iter()
                .filter_map(|e| match e {
                    PrimaryRef::Scenery(i) => Some(*i),
                    PrimaryRef::Temporary(_) => None,
                })
                .collect();
            let decor = tile.ground_decoration;
            for i in raised {
                let e = &mut scene.scenery[i];
                if !e.raised {
                    continue;
                }
                let overlay = e.model.as_mut().map_or(0, GpuModel::min_y);
                if overlay < lift {
                    lift = overlay;
                }
            }
            if let Some(decor) = decor.and_then(|d| scene.ground_decors.get(d)) {
                if decor.decor_height > -lift {
                    lift = -decor.decor_height;
                }
            }
        }
    }
    lift + 10
}

#[cfg(test)]
mod active_height_tests {
    use super::*;

    fn decor(height: i32) -> crate::scene::GroundDecorEntity {
        crate::scene::GroundDecorEntity {
            level: 0,
            occlude_level: 0,
            x: 256,
            y: 0,
            z: 256,
            decor_height: height,
            loc_id: 1,
            angle: 0,
            active: true,
            use_merged_normals: false,
            has_hard_shadow: false,
            srt: None,
            dynamic: false,
            model: None,
        }
    }

    #[test]
    fn obj_height_rides_the_ground_decoration() {
        // Stack height is lift + 10; a decoration of height 30 lifts the stack to lift = -30.
        let mut scene = crate::scene::Scene::new(9, 4, 4, 4);
        scene.create_tile(0, 1, 1);
        assert_eq!(obj_height(&mut scene, (0, 1, 1)), 10);
        scene.add_ground_decoration(0, 1, 1, decor(30));
        assert_eq!(obj_height(&mut scene, (0, 1, 1)), -20);
        // Off-map and empty tiles keep the unraised height.
        assert_eq!(obj_height(&mut scene, (0, 3, 3)), 10);
        assert_eq!(obj_height(&mut scene, (0, -1, 0)), 10);
    }

    #[test]
    fn loc_height_resolves_the_bound_layer() {
        // Layer 3 is the ground decoration; without a model its overlay height is 0.
        let mut scene = crate::scene::Scene::new(9, 4, 4, 4);
        scene.create_tile(0, 2, 2);
        assert_eq!(loc_height(&mut scene, (0, 3, 2, 2)), None);
        scene.add_ground_decoration(0, 2, 2, decor(0));
        assert_eq!(loc_height(&mut scene, (0, 3, 2, 2)), Some(0));
        // The wall layer of the same tile holds nothing.
        assert_eq!(loc_height(&mut scene, (0, 0, 2, 2)), None);
    }
}
