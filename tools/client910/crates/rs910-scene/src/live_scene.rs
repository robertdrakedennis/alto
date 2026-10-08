//! Live consumer of the scene draw and the client's roof producers.
use crate::{
    camera::SceneCamera,
    draw::{DrawEntity, DrawState},
    floor::{FloorGeometry, FloorHeights},
    occlusion::{Exclusion, Occlusion},
    roof::{RoofInput, RoofState, RoofWorld},
    scene::Scene,
    tileflags::SceneLevelTileFlags,
};

pub struct LiveScene {
    pub entities: Vec<DrawEntity>,
    pub static_entity_count: usize,
    /// Per static slot: how many times a loc change has put a model into it
    /// ([`Self::mark_slot_changed`]); part of the slot's
    /// [`crate::scene_snapshot::EntityKey`], so a backend's cached mesh of
    /// the slot's previous model is not reused.
    slot_changes: Vec<u32>,
    pub local_player: Option<[f32; 2]>,
    /// Applied removeRoofs2; setup is repeated when either mode or plane changes.
    pub roof_mode: i32,
    applied_roof_mode: i32,
    pub draw: DrawState,
    pub model_lights: crate::model_lights::ModelLights,
    pub dynamic: Option<crate::dynamic_scene::DynamicScene>,
    animation_epoch: Option<std::time::Instant>,
    heights: Vec<FloorHeights>,
    flags: SceneLevelTileFlags,
    occlusion: Occlusion,
    roof: RoofState,
    cycle: i32,
    /// The underwater scene's entities (indexed like the renderer's underwater
    /// models) and floor heights.
    underwater: Option<(Vec<DrawEntity>, Vec<FloorHeights>)>,
}
/// [`LiveScene::roof_removal`]: the scene draw's roof inputs.
#[derive(Clone, Copy, Debug)]
pub struct RoofRemoval<'a> {
    /// Per plane, x, z: the tile's roof stamp.
    pub stamps: &'a [Vec<Vec<i8>>],
    /// The stamp that hides a tile (`a[14]`).
    pub stamp: i8,
    /// The first plane the stamps can hide (`a[15]`).
    pub first_plane: usize,
}

impl LiveScene {
    /// Client cheat command 24: flip the scene's occlusion manager on or off.
    pub fn toggle_occlusion(&mut self) {
        self.occlusion.manager_enabled = !self.occlusion.manager_enabled;
    }
    /// A loc removal breaks its occluder out of the quads
    /// (see [`Occlusion::remove_occluder`]).
    pub fn remove_occluder(&mut self, scene: &mut Scene, kind: i32, level: i32, x: i32, z: i32) {
        self.occlusion
            .remove_occluder(scene, &self.heights, kind, level, x, z);
    }
    /// A runtime wall-loc marker (see [`Occlusion::set_level_occlude_map`]).
    pub fn set_level_occlude_map(&mut self, scene: &mut Scene, call: crate::scene::OccludeMapCall) {
        self.occlusion
            .set_level_occlude_map(scene, &self.heights, call);
    }
    /// Live occluder quads (all three occluder lists).
    pub fn occluder_count(&self) -> usize {
        self.occlusion.quads.len()
    }
    /// The per-level tile flags of this scene.
    pub fn tile_flags(&self) -> &SceneLevelTileFlags {
        &self.flags
    }
    /// Read only (renderer backends; the NXT interior shadow casters): the
    /// roof removal the last [`Self::update`] handed the scene draw; `None`
    /// without roof stamps.
    pub fn roof_removal(&self) -> Option<RoofRemoval<'_>> {
        Some(RoofRemoval {
            stamps: self.roof.stamps.as_deref()?,
            stamp: self.roof.draw_stamp(self.cycle),
            first_plane: usize::try_from(self.roof.setup_level + 1).ok()?,
        })
    }
    pub fn new(
        scene: &mut Scene,
        floors: &[Option<FloorGeometry>],
        flags: SceneLevelTileFlags,
        lights: &[crate::env::StaticLight],
    ) -> Option<Self> {
        let heights: Vec<_> = floors
            .iter()
            .map(|g| g.as_ref().map(|g| g.heights.clone()))
            .collect::<Option<_>>()?;
        let entities = crate::draw::entities(scene);
        Some(Self {
            static_entity_count: entities.len(),
            slot_changes: vec![0; entities.len()],
            local_player: None,
            roof_mode: 2,
            applied_roof_mode: -1,
            dynamic: None,
            animation_epoch: None,
            model_lights: crate::model_lights::ModelLights::new(scene, lights, entities.len()),
            entities,
            draw: DrawState::new(32),
            occlusion: Occlusion::new(scene, &heights),
            heights,
            flags,
            roof: RoofState::default(),
            cycle: 0,
            underwater: None,
        })
    }
    /// Install the underwater scene the scene draw plans before the normal one;
    /// `None` without an underwater scene.
    pub fn set_underwater(&mut self, underwater: Option<(Vec<DrawEntity>, Vec<FloorHeights>)>) {
        self.underwater = underwater;
    }
    #[must_use]
    pub fn has_underwater(&self) -> bool {
        self.underwater.is_some()
    }
    /// A loc change put a model (or none) into static slot `id`
    /// (loc removal or ground-loc add on the slot's entity).
    pub fn mark_slot_changed(&mut self, id: usize) {
        if let Some(n) = self.slot_changes.get_mut(id) {
            *n = n.wrapping_add(1);
        }
    }
    /// The loc changes static slot `id` has seen (0 for any other id).
    #[must_use]
    pub fn slot_changes(&self, id: usize) -> u32 {
        self.slot_changes.get(id).copied().unwrap_or(0)
    }
    pub fn install_temporary(&mut self, scene: &Scene) {
        self.entities.truncate(self.static_entity_count);
        crate::draw::append_temporary(scene, &mut self.entities);
        self.model_lights
            .reset_temporary(self.static_entity_count, self.entities.len());
    }
    pub fn enable_dynamic(
        &mut self,
        pack: &crate::cache::Pack,
        locs: &crate::config::LocStore,
        cache: crate::loctype::SharedModelCache,
    ) -> anyhow::Result<()> {
        rs910_core::profile::scope!("scene enable dynamic");
        self.enable_dynamic_with_variables(pack, locs, cache, None)
    }
    pub fn enable_dynamic_with_variables(
        &mut self,
        pack: &crate::cache::Pack,
        locs: &crate::config::LocStore,
        cache: crate::loctype::SharedModelCache,
        variables: Option<&crate::entities910::varps::Varps>,
    ) -> anyhow::Result<()> {
        self.enable_dynamic_with_preferences(
            pack,
            locs,
            cache,
            variables,
            &crate::rebuild::BuildPrefs::default(),
        )
    }
    pub fn enable_dynamic_with_preferences(
        &mut self,
        pack: &crate::cache::Pack,
        locs: &crate::config::LocStore,
        cache: crate::loctype::SharedModelCache,
        variables: Option<&crate::entities910::varps::Varps>,
        prefs: &crate::rebuild::BuildPrefs,
    ) -> anyhow::Result<()> {
        rs910_core::profile::scope!("scene enable dynamic");
        let seed = crate::debug_flags::flags()
            .animation_seed
            .unwrap_or_else(|| {
                crate::logic_clock::system_time()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64
            });
        self.dynamic = Some(crate::dynamic_scene::DynamicScene::new_with_preferences(
            pack,
            locs,
            &self.entities,
            seed,
            cache,
            variables,
            prefs,
        )?);
        Ok(())
    }
    ///  20 ms logic clock, independent of redraw/camera motion.
    pub fn animation_cycle(&mut self) -> i32 {
        if let Some(cycle) = crate::debug_flags::flags().animation_cycle {
            return cycle;
        }
        let start = *self
            .animation_epoch
            .get_or_insert_with(crate::logic_clock::now);
        (crate::logic_clock::now().duration_since(start).as_millis() / 20) as i32
    }
    pub fn update(
        &mut self,
        scene: &Scene,
        mut camera: SceneCamera,
        base: (i32, i32),
        level: usize,
        server_roof: [i32; 2],
    ) {
        // Roof rays: cam2's absolute look-at and eye before the scene-local
        // rebase.
        let cam2_abs = camera.cam2.as_ref().map(|c| {
            let t = camera.target;
            (
                [t[0] as f32 + c.lookat[0], t[2] as f32 + c.lookat[2]],
                [t[0] as f32 + c.eye[0], t[2] as f32 + c.eye[2]],
            )
        });
        camera.target[0] -= base.0 * 512;
        camera.target[2] -= base.1 * 512;
        let eye = camera.eye();
        self.cycle = self.cycle.wrapping_add(1) & i32::MAX;
        let world = RoofWorld {
            scene: Some(scene),
            flags: &self.flags,
            heights: &self.heights,
        };
        if self.roof.setup_level != level as i32 || self.applied_roof_mode != self.roof_mode {
            self.roof.setup(&world, self.roof_mode, level, self.cycle);
            self.applied_roof_mode = self.roof_mode;
        }
        // TODO(#gap-G1): offline orbit target stands in for localPlayerEntity.
        // Roof rays require an in-window camera.
        // Keep the free viewer's actual eye for projection/culling.
        let roof_eye = [
            eye[0].clamp(0, scene.max_x as i32 * 512 - 1),
            eye[1],
            eye[2].clamp(0, scene.max_z as i32 * 512 - 1),
        ];
        self.roof.update(
            &world,
            &RoofInput {
                cycle: self.cycle,
                level,
                camera_state: if cam2_abs.is_some() { 3 } else { 2 },
                camera: roof_eye,
                pitch: camera.pitch_int(),
                player: self.local_player.unwrap_or([
                    camera.target[0].clamp(0, scene.max_x as i32 * 512 - 1) as f32,
                    camera.target[2].clamp(0, scene.max_z as i32 * 512 - 1) as f32,
                ]),
                server: server_roof,
                base: [base.0, base.1],
                cam2_look: cam2_abs.map_or([f32::NAN; 2], |c| c.0),
                cam2_eye: cam2_abs.map_or([f32::NAN; 2], |c| c.1),
            },
        );
        let boxes: Vec<_> = self
            .roof
            .boxes
            .iter()
            .map(|&[max_height, min_x, max_x, max_z, min_z]| Exclusion {
                max_height,
                min_x,
                max_x,
                max_z,
                min_z,
            })
            .collect();
        let (near, far) = crate::camera::clip_distances(scene.max_x as i32);
        let (w, h) = camera.viewport;
        let view = crate::draw::DrawFrame {
            id: self.cycle,
            eye,
            yaw: camera.yaw_int(),
            pitch: camera.pitch_int(),
            surface: [w, h],
            viewport: [0, 0, w, h],
            near,
            far: (far as f32 * camera.far_scale) as i32,
            roof_stamp: self.roof.draw_stamp(self.cycle) as i32,
            roof_level: level as i32 + 1,
            cull: true,
            hide: 0,
            occlusion: 1,
        };
        let world = crate::draw::PlannerScene {
            scene,
            heights: &self.heights,
            entities: &self.entities,
        };
        self.draw.live_frame(
            world,
            view,
            self.roof.stamps.as_deref(),
            &mut self.occlusion,
            crate::draw::LiveInputs {
                boxes: &boxes,
                cam2: camera
                    .cam2
                    .as_ref()
                    .map(|c| (camera.view_entries(), c.projection())),
                underwater: self
                    .underwater
                    .as_ref()
                    .map(|(entities, heights)| (entities.as_slice(), heights.as_slice())),
            },
        );
        for &id in &self.draw.plan.dispatched {
            self.model_lights.collect(
                scene,
                &self.entities[id],
                [eye[0] >> scene.size, eye[2] >> scene.size],
            );
        }
    }
}

#[cfg(test)]
mod loc_change_tests {
    use super::*;
    use crate::loctype::shape;
    use std::collections::{BTreeSet, HashMap};

    fn quad_set(quads: &[crate::occlusion::Occluder]) -> BTreeSet<Vec<i32>> {
        quads
            .iter()
            .map(crate::occlusion::Occluder::words)
            .collect()
    }

    /// The occlusion half of a LOC_DEL / LOC_ADD_CHANGE on the offline
    /// Lumbridge scene, for every occluding straight wall the loader placed
    /// (wall loc placement): the removal clears the marker the loader set and
    /// breaks the wall's run out of the quads, and re-adding the wall (the
    /// level occlude map) gives back exactly the loader's quads.
    /// The loader's markers are the independent reference, so a mistake
    /// made the same way in `remove_loc_occluders` and `add_loc_occluders`
    /// fails here. The calls follow `client910`'s `scene_host` loc changes.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn lumbridge_straight_wall_del_and_add_follow_the_loader_markers() {
        let pack = crate::test_support::require_pack("client.mapsv2.js5");
        let materials = crate::texture::MaterialStore::load(&pack).unwrap();
        let store = crate::config::LocStore::load(&pack).unwrap();
        let flo = crate::flo::FloStore::load(&pack).unwrap();
        let tables = crate::maploader::FloTables::from_store(&flo);
        let mut world = crate::rebuild::rebuild_normal(
            &pack,
            &tables,
            &materials,
            Some(&store),
            3222,
            3222,
            &Default::default(),
        )
        .unwrap();
        let scene = world.scene_graph.as_mut().unwrap();
        let mut live = LiveScene::new(
            scene,
            &world.scene.normal,
            world.flags.clone(),
            &world.env.lights,
        )
        .unwrap();
        let original = live.occlusion.quads.clone();
        let original_set = quad_set(&original);

        // Every layer-0 wall's markers (both slots), to skip markers two
        // walls share: removing one of them clears the other's too.
        let mut walls = Vec::new();
        let mut owners = HashMap::<(i32, i32, i32, i32), usize>::new();
        for (&(level, layer, x, z), &(primary, second)) in &crate::locs::location_refs(scene) {
            if layer != 0 {
                continue;
            }
            for entity in std::iter::once(primary).chain(second) {
                let occupant = crate::dynamic_scene::slot_loc(scene, entity);
                let Some(loc) = store.get(occupant.loc_id) else {
                    continue;
                };
                let calls = crate::locs::add_loc_occluders(
                    loc,
                    occupant.shape,
                    occupant.angle,
                    level,
                    x,
                    z,
                );
                for c in &calls {
                    *owners.entry((c.kind, c.level, c.x, c.z)).or_default() += 1;
                }
                if entity == primary
                    && occupant.shape == shape::WALL_STRAIGHT
                    && loc.occlude == 1
                    && !live.flags.is_link_below(x, z)
                {
                    walls.push((level, x, z, occupant.angle, loc));
                }
            }
        }
        walls.sort_by_key(|&(level, x, z, angle, _)| (level, x, z, angle));

        let mut angles = [0; 4];
        for (level, x, z, angle, loc) in walls {
            let removed =
                crate::locs::remove_loc_occluders(loc, 0, shape::WALL_STRAIGHT, angle, x, z);
            let &[(kind, mx, mz)] = removed.as_slice() else {
                panic!("a straight wall clears one marker: {removed:?}");
            };
            if owners.get(&(kind, level, mx, mz)) != Some(&1) {
                continue;
            }
            let marker = |scene: &Scene| {
                scene
                    .tile(level as usize, mx as usize, mz as usize)
                    .map(|t| {
                        if kind == 1 {
                            t.occlude_x.0
                        } else {
                            t.occlude_z.0
                        }
                    })
            };
            let wall = (level, x, z, angle);
            assert!(
                marker(scene).is_some_and(|h| h != 0),
                "{wall:?}: LOC_DEL targets a marker the loader set ({kind}, {mx}, {mz})"
            );

            live.remove_occluder(scene, kind, level, mx, mz);
            assert_eq!(marker(scene), Some(0), "{wall:?}");
            let after = quad_set(&live.occlusion.quads);
            let gone: Vec<_> = original_set.difference(&after).collect();
            let new: Vec<_> = after.difference(&original_set).collect();
            // words(): [kind, level, minTileX, maxTileX, minTileZ, maxTileZ, ..]
            // with the max tile exclusive; kind 1 runs along z, kind 2 along x.
            let span = |q: &Vec<i32>| {
                if kind == 1 {
                    (q[4], q[5])
                } else {
                    (q[2], q[3])
                }
            };
            let at = if kind == 1 { mz } else { mx };
            assert!(
                gone.iter().any(|q| {
                    let (a, b) = span(q);
                    a <= at && at < b
                }),
                "{wall:?}: the quad over the wall goes"
            );
            let lo = gone.iter().map(|q| span(q).0).min().unwrap();
            let hi = gone.iter().map(|q| span(q).1).max().unwrap();
            for q in gone.iter().chain(&new) {
                assert_eq!(
                    (q[0], q[1]),
                    (kind, level),
                    "{wall:?}: other occluders stay"
                );
            }
            for q in &new {
                let (a, b) = span(q);
                assert!(
                    lo <= a && b <= hi && !(a <= at && at < b),
                    "{wall:?}: the rest of the run is re-coalesced around the gap: {q:?}"
                );
            }

            for call in
                crate::locs::add_loc_occluders(loc, shape::WALL_STRAIGHT, angle, level, x, z)
            {
                live.set_level_occlude_map(scene, call);
            }
            assert!(
                live.occlusion.quads == original,
                "{wall:?}: re-adding the wall restores the loader's quads"
            );
            angles[angle as usize] += 1;
        }
        assert!(
            angles.iter().all(|&n| n > 0),
            "every angle exercised: {angles:?}"
        );
    }
}
