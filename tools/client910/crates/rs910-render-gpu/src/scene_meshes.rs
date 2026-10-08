//! The faithful GPU toolkit's scene resource cache (`docs/renderer/modern-renderer.md`):
//! the uploaded
//! floors per level with their static-light and hard-shadow passes, the
//! scene-graph models in opaque and transparent draw order,
//! this frame's transient models and the per-frame material state. It moved
//! out of the shell (`client910::app::ViewerApp`, whose fields and methods
//! built these meshes) so a second backend keeps its own cache instead of
//! duplicating every call site.
//!
//! Keys: a static or dynamic scene entity's mesh is found by its
//! `live.entities` slot (`scene_mesh_ids[id]`: which list and where), a
//! dynamic loc's by its model revision as well (`dynamic_mesh_revisions`),
//! i.e. `rs910_scene::scene_snapshot::EntityKey`. Transient meshes are
//! uploaded every frame (`transient_mesh_ids`); a slot whose model keeps
//! last frame's structure rewrites last frame's mesh in place
//! (`FloorMesh::reuse_for_model`, programme Phase 6) instead of building one.
//!
//! Upload points follow the client's (behaviour-neutral: the buffers are
//! created in the same order with the same contents): the shell calls
//! [`SceneMeshes::upload_floors`], [`SceneMeshes::upload_floor_passes`] and
//! [`SceneMeshes::upload_scene`] when a scene installs, the slot and shadow
//! updates when locs change or animate, [`SceneMeshes::upload_transients`]
//! and [`SceneMeshes::select_floors`] in the redraw's CPU passes, and
//! [`SceneMeshes::frame`] draws the frame's
//! `rs910_scene::scene_snapshot::SceneSnapshot`.
use crate::floor_render::FloorMesh;

/// Scene levels (the shell's `NUM_LEVELS`).
const NUM_LEVELS: usize = 4;

/// [`SceneMeshes::frame_summary`]: the meshes [`SceneMeshes::frame`] draws,
/// by structure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameSummary {
    /// Per plan entity in draw order, the `mesh_for` slots it fills: 0 the
    /// spot shadow, 1-9 the hint arrows, 10 the body or model.
    pub opaque: Vec<(usize, usize)>,
    pub transparent: Vec<(usize, usize)>,
    /// `(level, material, index count)` of every floor batch that draws
    /// (over the level's current tile selection).
    pub floors: Vec<(usize, i32, u32)>,
}

/// The inputs of a scene install: the resources and the level geometry,
/// the region base, and the underwater floor and models.
#[derive(Clone, Copy)]
pub struct FloorUpload<'a> {
    pub pack: &'a crate::cache::Pack,
    pub materials: &'a crate::texture::MaterialStore,
    pub floors: &'a [Option<crate::floor::FloorGeometry>],
    pub floor_base: (i32, i32),
    pub underwater_floor: Option<&'a crate::floor::FloorGeometry>,
    pub underwater_models: &'a [crate::rebuild::UnderwaterModel],
}

/// One dynamic loc's mesh refresh: its entity `id`, its current model
/// `revision` and the resources to rebuild the mesh from.
#[derive(Clone, Copy)]
pub struct DynamicRefresh<'a> {
    pub pack: &'a crate::cache::Pack,
    pub materials: &'a crate::texture::MaterialStore,
    pub scene: &'a crate::scene::Scene,
    pub entity: &'a crate::draw::DrawEntity,
    pub id: usize,
    pub revision: u64,
    pub floor_base: (i32, i32),
}

/// The inputs of one frame's material walk
/// ([`SceneMeshes::prepare_frame_materials`]).
pub struct FrameMaterials<'a> {
    pub live: &'a crate::live_scene::LiveScene,
    pub players: Option<&'a mut crate::player_renderer::PlayersRenderer>,
    pub resources: Option<(&'a crate::cache::Pack, &'a crate::texture::MaterialStore)>,
    pub underwater_floor: Option<&'a crate::floor::FloorGeometry>,
    pub lights: &'a [Vec<crate::floorlight::BakedLight>],
    pub camera: &'a crate::camera::SceneCamera,
    pub floor_base: (i32, i32),
    pub env: &'a crate::env::EnvFrame,
}

/// See the module docs. Field names are the shell's (`ViewerApp`) they
/// replace.
pub struct SceneMeshes {
    /// Uploaded faithful floors per level.
    pub floor_meshes: Vec<Option<FloorMesh>>,
    /// Uploaded static lights per level.
    pub light_meshes: Vec<Vec<crate::floorpass::LightMesh>>,
    /// Uploaded hard-shadow blocks per level.
    pub shadow_meshes: Vec<Option<crate::floorpass::ShadowMesh>>,
    /// Uploaded scene models, opaque draw order.
    pub scene_opaque_meshes: Vec<FloorMesh>,
    /// Uploaded scene models, transparent draw order.
    pub scene_transparent_meshes: Vec<FloorMesh>,
    /// Per `live.entities` slot: `(transparent, index)` into the lists above.
    pub scene_mesh_ids: Vec<Option<(bool, usize)>>,
    pub transient_opaque_meshes: Vec<FloorMesh>,
    pub transient_transparent_meshes: Vec<FloorMesh>,
    pub transient_mesh_ids: Vec<Option<(bool, usize)>>,
    /// Last frame's transient meshes with their entity slots, in slot
    /// order, for reuse by this frame's [`Self::upload_transients`]
    /// (dropped there when unused).
    transient_pool: Vec<(usize, FloorMesh)>,
    /// The dynamic loc model revision each slot's mesh was built from.
    pub dynamic_mesh_revisions: Vec<u64>,
    /// The tile selection each floor mesh holds (the last plan's).
    pub selected_floors: Vec<crate::draw::FloorSelection>,
    /// The level-0 selection still to apply to the underwater floor.
    pub underwater_selection: Option<crate::draw::FloorSelection>,
    /// The material state walked across the frame's draws.
    pub material_state: crate::material::MaterialState,
    /// A scene model upload failed (the map transaction must not complete).
    pub scene_upload_failed: bool,
}

impl Default for SceneMeshes {
    fn default() -> Self {
        Self {
            floor_meshes: vec![None, None, None, None],
            light_meshes: Vec::new(),
            shadow_meshes: Vec::new(),
            scene_opaque_meshes: Vec::new(),
            scene_transparent_meshes: Vec::new(),
            scene_mesh_ids: Vec::new(),
            transient_opaque_meshes: Vec::new(),
            transient_transparent_meshes: Vec::new(),
            transient_mesh_ids: Vec::new(),
            transient_pool: Vec::new(),
            dynamic_mesh_revisions: Vec::new(),
            selected_floors: Vec::new(),
            underwater_selection: None,
            material_state: crate::material::MaterialState::default(),
            scene_upload_failed: false,
        }
    }
}

impl SceneMeshes {
    /// A new scene: forget the previous scene's selections, models and
    /// floor passes (the floors are replaced by [`Self::upload_floors`]).
    pub fn reset_scene(&mut self) {
        self.selected_floors.clear();
        self.scene_opaque_meshes.clear();
        self.scene_transparent_meshes.clear();
        self.scene_mesh_ids.clear();
        self.light_meshes.clear();
        self.shadow_meshes.clear();
    }

    /// Upload the faithful floors (all levels) through
    /// `Renderer::build_floor_mesh`, the underwater floor
    /// (the level-0 underwater geometry) and the underwater scene's locs.
    /// Returns whether the underwater locs uploaded.
    pub fn upload_floors(
        &mut self,
        (renderer, gpu): crate::render::Faithful<'_>,
        install: &FloorUpload<'_>,
    ) -> anyhow::Result<()> {
        let FloorUpload {
            pack,
            materials,
            floors,
            floor_base,
            underwater_floor,
            underwater_models,
        } = *install;
        let mut meshes: Vec<Option<FloorMesh>> = Vec::with_capacity(NUM_LEVELS);
        for (level, geometry) in floors.iter().enumerate() {
            let Some(geometry) = geometry else {
                meshes.push(None);
                continue;
            };
            let origin = [
                (floor_base.0 * 512) as f32,
                0.0,
                (floor_base.1 * 512) as f32,
            ];
            match renderer.build_floor_mesh(
                gpu,
                pack,
                materials,
                geometry,
                origin,
                &format!("floor l{level}"),
            ) {
                Ok(mesh) => {
                    log::info!(
                        "[client910] floor l{level}: {} verts, {} batches uploaded (depth {}, water detail {}, water programs {:?})",
                        mesh.vertex_count,
                        mesh.batch_count(),
                        geometry.has_depth,
                        geometry.water_detail,
                        mesh.water_program_counts()
                    );
                    meshes.push(Some(mesh));
                }
                Err(err) => {
                    log::warn!("[client910] floor l{level} upload failed: {err:#}");
                    meshes.push(None);
                }
            }
        }
        self.floor_meshes = meshes;
        // The underwater floor.
        let underwater = underwater_floor.and_then(|geometry| {
            let origin = [
                (floor_base.0 * 512) as f32,
                0.0,
                (floor_base.1 * 512) as f32,
            ];
            match renderer.build_floor_mesh(gpu, pack, materials, geometry, origin, "floor underwater") {
                Ok(mesh) => {
                    log::info!(
                        "[client910] underwater floor: {} verts, {} batches uploaded (water programs {:?})",
                        mesh.vertex_count,
                        mesh.batch_count(),
                        mesh.water_program_counts()
                    );
                    Some(mesh)
                }
                Err(err) => {
                    log::warn!("[client910] underwater floor upload failed: {err:#}");
                    None
                }
            }
        });
        *renderer.underwater_floor_mut() = underwater;
        // The underwater scene's locs.
        let uploaded =
            renderer.set_underwater_models(gpu, pack, materials, underwater_models, floor_base);
        if let Err(err) = &uploaded {
            log::warn!("[client910] underwater loc upload failed: {err:#}");
        } else if !underwater_models.is_empty() {
            log::info!(
                "[client910] underwater locs: {} models uploaded",
                underwater_models.len()
            );
        }
        uploaded
    }

    /// The static lights and hard-shadow blocks of every level.
    pub fn upload_floor_passes(
        &mut self,
        (renderer, gpu): crate::render::FaithfulRef<'_>,
        floors: &[Option<crate::floor::FloorGeometry>],
        lights: &[Vec<crate::floorlight::BakedLight>],
        floor_base: (i32, i32),
    ) {
        let origin = [
            (floor_base.0 * 512) as f32,
            0.0,
            (floor_base.1 * 512) as f32,
        ];
        let mut light_meshes = Vec::with_capacity(NUM_LEVELS);
        let mut shadow_meshes = Vec::with_capacity(NUM_LEVELS);
        for level in 0..NUM_LEVELS {
            let lights = lights.get(level).map(Vec::as_slice).unwrap_or(&[]);
            light_meshes.push(renderer.build_light_meshes(
                gpu,
                lights,
                origin,
                &format!("lights l{level}"),
            ));
            let shadows = floors
                .get(level)
                .and_then(|g| g.as_ref())
                .and_then(|geometry| {
                    crate::floor::shadow_mask_view(geometry).map(|mask| {
                        renderer.build_shadow_mesh(
                            gpu,
                            geometry,
                            &mask,
                            origin,
                            &format!("shadows l{level}"),
                        )
                    })
                });
            shadow_meshes.push(shadows);
        }
        log::info!(
            "[client910] floor passes: lights {:?}, shadow blocks {:?}",
            light_meshes.iter().map(Vec::len).collect::<Vec<_>>(),
            shadow_meshes
                .iter()
                .map(|s| s.as_ref().map_or(0, |s| s.block_count()))
                .collect::<Vec<_>>()
        );
        self.light_meshes = light_meshes;
        self.shadow_meshes = shadow_meshes;
    }

    /// The start of a scene-model upload: nothing uploaded yet, and a
    /// missing input leaves the upload failed.
    pub fn begin_scene_upload(&mut self) {
        self.scene_upload_failed = true;
        self.scene_opaque_meshes.clear();
        self.scene_transparent_meshes.clear();
    }

    /// Upload every static scene-graph model (opaque and transparent lists)
    /// through `Renderer::build_model_mesh`. The client prepends to those
    /// lists, so the traversal order is the reverse of the push order kept in
    /// `scene.rs`.
    pub fn upload_scene(
        &mut self,
        (renderer, gpu): crate::render::Faithful<'_>,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        scene: &crate::scene::Scene,
        live: Option<&crate::live_scene::LiveScene>,
        floor_base: (i32, i32),
    ) {
        let base = [
            (floor_base.0 * 512) as f32,
            0.0,
            (floor_base.1 * 512) as f32,
        ];
        let mut failures = 0_usize;
        let mut associations = Vec::new();
        let mut build =
            |refs: &[crate::scene::EntityRef], transparent: bool, out: &mut Vec<FloorMesh>| {
                for e in refs.iter().rev() {
                    let (model, pos, label) = match *e {
                        crate::scene::EntityRef::Temporary(_) => continue,
                        crate::scene::EntityRef::Scenery(i) => {
                            let s = &scene.scenery[i];
                            (s.model.as_ref(), [s.x, s.y, s.z], "scenery")
                        }
                        crate::scene::EntityRef::Wall(i) => {
                            let w = &scene.walls[i];
                            (w.model.as_ref(), [w.x, w.y, w.z], "wall")
                        }
                        crate::scene::EntityRef::WallDecor(i) => {
                            // A wall decor draws translated by its offsets.
                            let d = &scene.wall_decors[i];
                            (
                                d.model.as_ref(),
                                [d.x + d.offset_x, d.y, d.z + d.offset_z],
                                "wall decor",
                            )
                        }
                        crate::scene::EntityRef::GroundDecor(i) => {
                            let g = &scene.ground_decors[i];
                            (g.model.as_ref(), [g.x, g.y, g.z], "ground decor")
                        }
                    };
                    let Some(model) = model else { continue };
                    let model = crate::scene::srt_model(model, crate::scene::entity_srt(scene, *e));
                    let origin = [
                        base[0] + pos[0] as f32,
                        base[1] + pos[1] as f32,
                        base[2] + pos[2] as f32,
                    ];
                    match renderer.build_model_mesh(gpu, pack, materials, &model, origin, label) {
                        Ok(mesh) => {
                            associations.push((*e, transparent, out.len()));
                            out.push(mesh);
                        }
                        Err(err) => {
                            failures += 1;
                            if failures <= 5 {
                                log::warn!("[client910] scene model upload failed: {err:#}");
                            }
                        }
                    }
                }
            };
        let mut opaque = Vec::new();
        let mut transparent = Vec::new();
        build(&scene.opaque, false, &mut opaque);
        build(&scene.transparent, true, &mut transparent);
        build(&scene.pending, false, &mut opaque);
        self.scene_mesh_ids = live
            .map(|live| {
                live.entities
                    .iter()
                    .map(|e| {
                        associations
                            .iter()
                            .find(|(source, _, _)| *source == e.source)
                            .map(|&(_, t, i)| (t, i))
                    })
                    .collect()
            })
            .unwrap_or_default();
        log::info!(
            "[client910] scene: {} opaque + {} transparent models uploaded ({} failures)",
            opaque.len(),
            transparent.len(),
            failures
        );
        self.dynamic_mesh_revisions = vec![u64::MAX; self.scene_mesh_ids.len()];
        self.scene_upload_failed = failures != 0;
        self.scene_opaque_meshes = opaque;
        self.scene_transparent_meshes = transparent;
    }

    /// The mesh half of the dynamic loc refresh
    /// (`ViewerApp::refresh_dynamic_scene`): entity `refresh.id`'s current
    /// model (at dynamic `refresh.revision`) replaces its mesh's streams in
    /// place, or a new mesh is built into its slot (appended to the opaque
    /// list for an entity without one).
    pub fn refresh_dynamic_mesh(
        &mut self,
        (renderer, gpu): crate::render::Faithful<'_>,
        refresh: &DynamicRefresh<'_>,
    ) -> anyhow::Result<()> {
        let DynamicRefresh {
            pack,
            materials,
            scene,
            entity,
            id,
            revision,
            floor_base,
        } = *refresh;
        let Some(model) = crate::dynamic_scene::model(scene, entity.source) else {
            return Ok(());
        };
        if self.dynamic_mesh_revisions[id] == revision {
            return Ok(());
        }
        let model = crate::scene::srt_model(model, crate::scene::entity_srt(scene, entity.source));
        let model = &*model;
        let slot = self.scene_mesh_ids[id];
        let reused = if let Some((transparent, index)) = slot {
            let mesh = if transparent {
                &mut self.scene_transparent_meshes[index]
            } else {
                &mut self.scene_opaque_meshes[index]
            };
            renderer.update_model_mesh(gpu, mesh, materials, model)?
        } else {
            false
        };
        if !reused {
            let mut origin = [
                (floor_base.0 * 512 + entity.x) as f32,
                entity.y as f32,
                (floor_base.1 * 512 + entity.z) as f32,
            ];
            if let crate::scene::EntityRef::WallDecor(i) = entity.source {
                origin[0] += scene.wall_decors[i].offset_x as f32;
                origin[2] += scene.wall_decors[i].offset_z as f32;
            }
            let mesh = renderer.build_model_mesh(
                gpu,
                pack,
                materials,
                model,
                origin,
                "dynamic scenery",
            )?;
            if let Some((transparent, index)) = slot {
                if transparent {
                    self.scene_transparent_meshes[index] = mesh;
                } else {
                    self.scene_opaque_meshes[index] = mesh;
                }
            } else {
                self.scene_mesh_ids[id] = Some((false, self.scene_opaque_meshes.len()));
                self.scene_opaque_meshes.push(mesh);
            }
        }
        self.dynamic_mesh_revisions[id] = revision;
        Ok(())
    }

    /// Mark entity `id`'s mesh stale: a restored or replaced dynamic loc
    /// restarts its model revisions.
    pub fn invalidate_dynamic_mesh(&mut self, id: usize) {
        if let Some(r) = self.dynamic_mesh_revisions.get_mut(id) {
            *r = u64::MAX;
        }
    }

    /// The dynamic locs' shadow changes at the floor pass (floors draw after
    /// the opaque entities): re-upload each level's dirty blocks and
    /// drain them. Transparent loc shadow changes stay in the CPU masks until
    /// the next floor pass.
    pub fn update_dynamic_shadows(
        &mut self,
        (renderer, gpu): crate::render::FaithfulRef<'_>,
        floors: &[Option<crate::floor::FloorGeometry>],
        dirty_shadows: &mut [std::collections::HashSet<(usize, usize)>],
    ) {
        for (level, mesh) in self.shadow_meshes.iter_mut().enumerate() {
            if let (Some(mesh), Some(geometry)) = (mesh, floors[level].as_ref()) {
                renderer.update_shadow_mesh(gpu, mesh, geometry, &dirty_shadows[level]);
            }
            dirty_shadows[level].clear();
        }
    }

    /// Re-upload the touched shadow blocks.
    pub fn update_shadow_blocks(
        &mut self,
        (renderer, gpu): crate::render::FaithfulRef<'_>,
        floors: &[Option<crate::floor::FloorGeometry>],
        dirty: &[std::collections::HashSet<(usize, usize)>],
    ) {
        for (level, mesh) in self.shadow_meshes.iter_mut().enumerate() {
            if let (Some(mesh), Some(geometry), Some(blocks)) = (
                mesh,
                floors.get(level).and_then(Option::as_ref),
                dirty.get(level),
            ) {
                if !blocks.is_empty() {
                    renderer.update_shadow_mesh(gpu, mesh, geometry, blocks);
                }
            }
        }
    }

    /// A static scene slot lost its model: nothing draws for entity `id`.
    pub fn clear_slot(&mut self, id: usize) {
        if let Some(slot) = self.scene_mesh_ids.get_mut(id) {
            *slot = None;
        }
    }

    /// The mesh half of a static slot install (a loc change): entity `id`'s new model at
    /// `origin`, into its old slot when the list (opaque or transparent)
    /// is unchanged, else appended to the list the model now belongs to.
    pub fn install_slot_mesh(
        &mut self,
        (renderer, gpu): crate::render::Faithful<'_>,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        id: usize,
        model: &crate::gpumodel::GpuModel,
        origin: [f32; 3],
    ) -> anyhow::Result<()> {
        let mesh = renderer.build_model_mesh(
            gpu,
            pack,
            materials,
            model,
            origin,
            "location replacement",
        )?;
        let transparent = model.has_transparency || model.has_particles;
        let slot = self.scene_mesh_ids.get(id).copied().flatten();
        if let Some((old_transparent, index)) = slot.filter(|(old, _)| *old == transparent) {
            if old_transparent {
                self.scene_transparent_meshes[index] = mesh;
            } else {
                self.scene_opaque_meshes[index] = mesh;
            }
        } else if transparent {
            let index = self.scene_transparent_meshes.len();
            self.scene_transparent_meshes.push(mesh);
            if let Some(slot) = self.scene_mesh_ids.get_mut(id) {
                *slot = Some((true, index));
            }
        } else {
            let index = self.scene_opaque_meshes.len();
            self.scene_opaque_meshes.push(mesh);
            if let Some(slot) = self.scene_mesh_ids.get_mut(id) {
                *slot = Some((false, index));
            }
        }
        Ok(())
    }

    /// Retire the previous frame's transient meshes: nothing draws them any
    /// more; they wait by entity slot for [`Self::upload_transients`] to
    /// reuse or drop.
    pub fn clear_transients(&mut self) {
        let mut opaque: Vec<Option<FloorMesh>> = std::mem::take(&mut self.transient_opaque_meshes)
            .into_iter()
            .map(Some)
            .collect();
        let mut transparent: Vec<Option<FloorMesh>> =
            std::mem::take(&mut self.transient_transparent_meshes)
                .into_iter()
                .map(Some)
                .collect();
        self.transient_pool.clear();
        for (id, slot) in self.transient_mesh_ids.iter().enumerate() {
            let mesh = slot.and_then(|(is_transparent, index)| {
                if is_transparent {
                    transparent.get_mut(index).and_then(Option::take)
                } else {
                    opaque.get_mut(index).and_then(Option::take)
                }
            });
            if let Some(mesh) = mesh {
                self.transient_pool.push((id, mesh));
            }
        }
    }

    /// This frame's transient scene entities (`scene.temporary` marked
    /// transient: projectiles, spot anims, ground items, NPC bodies, loc
    /// replacements on empty slots) as meshes, keyed by entity slot.
    pub fn upload_transients(
        &mut self,
        (renderer, gpu): crate::render::Faithful<'_>,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        live: &crate::live_scene::LiveScene,
        scene: &crate::scene::Scene,
        floor_base: (i32, i32),
    ) -> anyhow::Result<()> {
        self.transient_mesh_ids.clear();
        self.transient_mesh_ids.resize(live.entities.len(), None);
        // Last frame's mesh of the same slot, when this frame's model fits it
        // (`FloorMesh::reuse_for_model`); the rest drop at the end. Both
        // walk the slots in order.
        let mut pool = std::mem::take(&mut self.transient_pool)
            .into_iter()
            .peekable();
        for (id, entity) in live.entities.iter().enumerate() {
            let crate::scene::EntityRef::Temporary(index) = entity.source else {
                continue;
            };
            let temporary = &scene.temporary[index];
            if !temporary.transient {
                continue;
            }
            let Some(model) = temporary.model.as_ref() else {
                continue;
            };
            let origin = [
                (floor_base.0 * 512) as f32 + temporary.position[0],
                temporary.position[1],
                (floor_base.1 * 512) as f32 + temporary.position[2],
            ];
            while pool.next_if(|(slot, _)| *slot < id).is_some() {}
            let reused = match pool.next_if(|(slot, _)| *slot == id).map(|(_, mesh)| mesh) {
                Some(mut mesh) => renderer
                    .reuse_model_mesh(gpu, &mut mesh, materials, model, origin)?
                    .then_some(mesh),
                None => None,
            };
            let mut mesh = match reused {
                Some(mesh) => mesh,
                None => {
                    renderer.build_model_mesh(gpu, pack, materials, model, origin, "transient")?
                }
            };
            mesh.set_depth_write(!temporary.spot_shadow);
            if temporary.transparent {
                let slot = self.transient_transparent_meshes.len();
                self.transient_transparent_meshes.push(mesh);
                self.transient_mesh_ids[id] = Some((true, slot));
            } else {
                let slot = self.transient_opaque_meshes.len();
                self.transient_opaque_meshes.push(mesh);
                self.transient_mesh_ids[id] = Some((false, slot));
            }
        }
        Ok(())
    }

    /// The per-level tile selection of this frame's plan (`floors`,
    /// `live.draw.plan.floors`) onto each changed floor mesh and its light
    /// and shadow passes. The underwater floor's tiles are selected with the
    /// level-0 visibility of the frame.
    pub fn select_floors(
        &mut self,
        (renderer, gpu): crate::render::FaithfulRef<'_>,
        floors: &[Option<crate::floor::FloorGeometry>],
        selections: &[crate::draw::FloorSelection],
    ) {
        for (level, selection) in selections.iter().enumerate() {
            if self.selected_floors.get(level) == Some(selection) {
                continue;
            }
            // The underwater floor's tiles use the level-0 visibility of the
            // frame.
            if level == 0 {
                self.underwater_selection = Some(selection.clone());
            }
            if let (Some(geometry), Some(mesh)) = (
                floors.get(level).and_then(Option::as_ref),
                self.floor_meshes.get_mut(level).and_then(Option::as_mut),
            ) {
                renderer.select_floor_tiles(
                    gpu,
                    mesh,
                    geometry,
                    self.light_meshes
                        .get_mut(level)
                        .map(Vec::as_mut_slice)
                        .unwrap_or(&mut []),
                    self.shadow_meshes.get_mut(level).and_then(Option::as_mut),
                    selection,
                );
            }
        }
        self.selected_floors = selections.to_vec();
    }

    /// Put the frame's own tile selection back after a minimap base render
    /// (the minimap base rebuild) replaced it with the plan's.
    pub fn reselect_floors(
        &mut self,
        (renderer, gpu): crate::render::FaithfulRef<'_>,
        floors: &[Option<crate::floor::FloorGeometry>],
    ) {
        for (l, selection) in self.selected_floors.iter().enumerate() {
            if let (Some(geometry), Some(mesh)) = (
                floors.get(l).and_then(Option::as_ref),
                self.floor_meshes.get_mut(l).and_then(Option::as_mut),
            ) {
                renderer.select_floor_tiles(
                    gpu,
                    mesh,
                    geometry,
                    self.light_meshes
                        .get_mut(l)
                        .map(Vec::as_mut_slice)
                        .unwrap_or(&mut []),
                    self.shadow_meshes.get_mut(l).and_then(Option::as_mut),
                    selection,
                );
            }
        }
    }

    /// The static lights' flicker values (`ModelLights`) of this frame.
    pub fn update_light_intensities(
        &self,
        (renderer, gpu): crate::render::FaithfulRef<'_>,
        values: &[f32],
    ) {
        renderer.update_light_intensities(gpu, &self.light_meshes, values);
    }

    /// The frame's material walk (the material state across the frame's
    /// draws): the environment sampler, the underwater floor's tile
    /// selection and lists, then every opaque entity, each level's floor
    /// with its light and shadow passes, and every transparent entity in
    /// plan order. Player bodies take their actor frame first, their spot
    /// shadow and hint arrows with depth writes off.
    pub fn prepare_frame_materials(
        &mut self,
        (renderer, gpu): crate::render::Faithful<'_>,
        inputs: FrameMaterials<'_>,
    ) {
        let FrameMaterials {
            live,
            players,
            resources,
            underwater_floor,
            lights,
            camera,
            floor_base,
            env,
        } = inputs;
        if let Some((pack, materials)) = resources {
            if let Err(error) = renderer.set_material_environment(
                gpu,
                pack,
                materials,
                env.sampler,
                [(floor_base.0 * 512) as f32, 0., (floor_base.1 * 512) as f32],
            ) {
                crate::logging::warn_repeated!("[client910] environment sampler: {error:#}");
            }
        }
        let millis = renderer.material_time();
        // The underwater floor's batches come first.
        if let (Some(selection), Some(geometry)) =
            (self.underwater_selection.take(), underwater_floor)
        {
            renderer.select_underwater_tiles(gpu, geometry, &selection);
        }
        // The culled, sorted underwater lists.
        renderer.set_underwater_order(live.has_underwater().then(|| {
            (
                live.draw.plan.underwater_opaque.clone(),
                live.draw.plan.underwater_transparent.clone(),
            )
        }));
        renderer.prepare_underwater_materials(gpu, &mut self.material_state, millis);
        let mut players = players;
        let transient_mesh_ids = &self.transient_mesh_ids;
        let scene_mesh_ids = &self.scene_mesh_ids;
        let transient_opaque_meshes = &mut self.transient_opaque_meshes;
        let transient_transparent_meshes = &mut self.transient_transparent_meshes;
        let scene_opaque_meshes = &mut self.scene_opaque_meshes;
        let scene_transparent_meshes = &mut self.scene_transparent_meshes;
        let renderer = &*renderer;
        let mut models = |ids: &[usize], state: &mut crate::material::MaterialState| {
            for &id in ids {
                if let crate::scene::EntityRef::Temporary(_) = live.entities[id].source {
                    if live.entities[id].loc_id < 0 {
                        if let Some((transparent, index)) =
                            transient_mesh_ids.get(id).copied().flatten()
                        {
                            let mesh = if transparent {
                                transient_transparent_meshes.get_mut(index)
                            } else {
                                transient_opaque_meshes.get_mut(index)
                            };
                            if let Some(mesh) = mesh {
                                renderer.prepare_materials(
                                    gpu,
                                    mesh,
                                    state,
                                    millis,
                                    &live.model_lights.parameters(&live.entities[id]),
                                    live.model_lights.selected(id).len(),
                                );
                            }
                        }
                        continue;
                    }
                    let player = live.entities[id].loc_id as usize;
                    if let Some(entry) = players.as_mut().and_then(|r| r.meshes.get_mut(&player)) {
                        // Shadow, then hint arrows, both with depth writes
                        // off.
                        for shadow in entry
                            .shadow
                            .iter_mut()
                            .chain(entry.hint_arrows.iter_mut())
                            .filter(|s| s.visible)
                        {
                            let Some(mesh) = shadow.mesh.as_mut() else {
                                continue;
                            };
                            state.depth_primary = false;
                            renderer.set_actor_frame(
                                gpu,
                                mesh,
                                camera,
                                floor_base,
                                env,
                                &shadow.matrix,
                            );
                            renderer.prepare_materials(
                                gpu,
                                mesh,
                                state,
                                millis,
                                &live
                                    .model_lights
                                    .actor_parameters(&live.entities[id], &shadow.matrix),
                                live.model_lights.selected(id).len(),
                            );
                            state.depth_primary = true;
                        }
                        if !entry.visible {
                            continue;
                        }
                        let Some(mesh) = entry.mesh.as_mut() else {
                            continue;
                        };
                        renderer.set_actor_frame(gpu, mesh, camera, floor_base, env, &entry.matrix);
                        renderer.prepare_materials(
                            gpu,
                            mesh,
                            state,
                            millis,
                            &live
                                .model_lights
                                .actor_parameters(&live.entities[id], &entry.matrix),
                            live.model_lights.selected(id).len(),
                        );
                    }
                    continue;
                }
                let Some((transparent, index)) = scene_mesh_ids.get(id).copied().flatten() else {
                    continue;
                };
                let mesh = if transparent {
                    scene_transparent_meshes.get_mut(index)
                } else {
                    scene_opaque_meshes.get_mut(index)
                };
                if let Some(mesh) = mesh {
                    renderer.prepare_materials(
                        gpu,
                        mesh,
                        state,
                        millis,
                        &live.model_lights.parameters(&live.entities[id]),
                        live.model_lights.selected(id).len(),
                    );
                }
            }
        };
        models(&live.draw.plan.opaque, &mut self.material_state);
        for (level, mesh) in self.floor_meshes.iter_mut().enumerate() {
            let Some(mesh) = mesh else {
                continue;
            };
            if mesh.vertex_count == 0 {
                continue;
            }
            renderer.prepare_materials(
                gpu,
                mesh,
                &mut self.material_state,
                millis,
                &[[0.; 4]; 8],
                0,
            );
            if lights.get(level).is_some_and(|l| !l.is_empty()) {
                self.material_state.blend_mode(128);
                self.material_state.depth_secondary(false);
            }
            if self.shadow_meshes.get(level).is_some_and(Option::is_some) {
                self.material_state.blend_mode(1);
                self.material_state.depth_primary = true;
            }
        }
        models(&live.draw.plan.transparent, &mut self.material_state);
    }

    /// The meshes entity `id` draws, in draw order: its spot shadow, up to
    /// nine hint arrows, then the body (players), or the one model mesh.
    fn mesh_for<'m>(
        &'m self,
        snapshot: &crate::scene_snapshot::SceneSnapshot<'_>,
        players: Option<&'m crate::player_renderer::PlayersRenderer>,
        id: usize,
    ) -> [Option<&'m FloorMesh>; 11] {
        if let Some(e) = snapshot.live.and_then(|l| l.entities.get(id)) {
            if let crate::scene::EntityRef::Temporary(index) = e.source {
                if snapshot
                    .scene
                    .is_some_and(|scene| scene.temporary[index].transient)
                {
                    let mut out = [None; 11];
                    out[10] = self.transient_mesh_ids.get(id).copied().flatten().and_then(
                        |(transparent, index)| {
                            if transparent {
                                self.transient_transparent_meshes.get(index)
                            } else {
                                self.transient_opaque_meshes.get(index)
                            }
                        },
                    );
                    return out;
                }
                let Some(entry) = players.and_then(|r| r.meshes.get(&(e.loc_id as usize))) else {
                    return [None; 11];
                };
                let mut out = [None; 11];
                out[0] = entry
                    .shadow
                    .as_ref()
                    .filter(|s| s.visible)
                    .and_then(|s| s.mesh.as_ref());
                for (slot, arrow) in entry.hint_arrows.iter().take(9).enumerate() {
                    out[slot + 1] = arrow.mesh.as_ref().filter(|_| arrow.visible);
                }
                out[10] = entry.mesh.as_ref().filter(|_| entry.visible);
                return out;
            }
        }
        let mut out = [None; 11];
        out[10] =
            self.scene_mesh_ids
                .get(id)
                .copied()
                .flatten()
                .and_then(|(transparent, index)| {
                    if transparent {
                        self.scene_transparent_meshes.get(index)
                    } else {
                        self.scene_opaque_meshes.get(index)
                    }
                });
        out
    }

    /// The structure of the lists [`Self::frame`] draws for `snapshot`
    /// (renderer plan M1: what the NXT backend's draw list is checked
    /// against, `CLIENT910_MODERN_CHECK`); see [`FrameSummary`].
    #[must_use]
    pub fn frame_summary(
        &self,
        snapshot: &crate::scene_snapshot::SceneSnapshot<'_>,
        players: Option<&crate::player_renderer::PlayersRenderer>,
    ) -> FrameSummary {
        let slots = |ids: &[usize]| -> Vec<(usize, usize)> {
            ids.iter()
                .flat_map(|&id| {
                    self.mesh_for(snapshot, players, id)
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.is_some())
                        .map(|(slot, _)| (id, slot))
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let (opaque, transparent) = snapshot.live.map_or((Vec::new(), Vec::new()), |live| {
            (
                slots(&live.draw.plan.opaque),
                slots(&live.draw.plan.transparent),
            )
        });
        let floors = self
            .floor_meshes
            .iter()
            .enumerate()
            .filter_map(|(level, mesh)| mesh.as_ref().map(|mesh| (level, mesh)))
            .filter(|(_, mesh)| mesh.vertex_count != 0)
            .flat_map(|(level, mesh)| {
                mesh.batch_index_counts()
                    .into_iter()
                    .filter(|&(_, count)| count > 0)
                    .map(move |(material, count)| (level, material, count))
            })
            .collect();
        FrameSummary {
            opaque,
            transparent,
            floors,
        }
    }

    /// Draw the scene on the faithful GPU toolkit: every level's floor
    /// (each level draws with per-tile holes) with its static lights and
    /// hard shadows, the plan's opaque and transparent entity meshes (per
    /// entity: spot shadow, up to nine hint arrows, then the body), the
    /// particle owners' segments and the model billboards
    /// then `Renderer::frame_with_levels`.
    pub fn frame(
        &self,
        renderer: &mut crate::render::Renderer,
        gpu: &mut crate::gpu_device::Device,
        snapshot: &crate::scene_snapshot::SceneSnapshot<'_>,
        players: Option<&crate::player_renderer::PlayersRenderer>,
        camera: &crate::render::OrbitCamera,
    ) -> anyhow::Result<()> {
        // Every level's faithful floor (each level draws with per-tile
        // holes), with its static lights and hard shadows.
        let floors: Vec<crate::floorpass::FloorLevelDraw<'_>> = self
            .floor_meshes
            .iter()
            .enumerate()
            .filter_map(|(level, mesh)| {
                mesh.as_ref().map(|floor| crate::floorpass::FloorLevelDraw {
                    floor,
                    lights: self
                        .light_meshes
                        .get(level)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                    shadows: self.shadow_meshes.get(level).and_then(|s| s.as_ref()),
                })
            })
            .collect();
        // Per entity: spot shadow, up to nine hint arrows, then the body.
        let mesh_for = |id: &usize| self.mesh_for(snapshot, players, *id);
        let (scene_opaque, scene_transparent): (Vec<_>, Vec<_>) = if let Some(live) = snapshot.live
        {
            (
                live.draw
                    .plan
                    .opaque
                    .iter()
                    .flat_map(mesh_for)
                    .flatten()
                    .collect(),
                live.draw
                    .plan
                    .transparent
                    .iter()
                    .flat_map(mesh_for)
                    .flatten()
                    .collect(),
            )
        } else {
            (
                self.scene_opaque_meshes.iter().collect(),
                self.scene_transparent_meshes.iter().collect(),
            )
        };
        // Particle owners' draws follow their entity's meshes in the lists.
        if let Some(live) = snapshot.live {
            let ends = |ids: &[usize]| -> Vec<usize> {
                let mut count = 0;
                ids.iter()
                    .map(|id| {
                        count += mesh_for(id).iter().flatten().count();
                        count
                    })
                    .collect()
            };
            let opaque_ends = ends(&live.draw.plan.opaque);
            let transparent_ends = ends(&live.draw.plan.transparent);
            renderer.resolve_particle_segments([&opaque_ends, &transparent_ends]);
        }
        // Every listed model's billboards.
        if let (Some(pack), Some(materials)) = (snapshot.pack, snapshot.materials) {
            if let Err(error) = renderer.prepare_billboards(
                gpu,
                pack,
                materials,
                &snapshot.camera,
                snapshot.env.fog.range,
                [&scene_opaque, &scene_transparent],
            ) {
                crate::logging::warn_repeated!("[client910] billboard upload: {error:#}");
            }
        }
        renderer.frame_with_levels(
            gpu,
            camera,
            snapshot.env,
            &floors,
            &scene_opaque,
            &scene_transparent,
        )
    }
}
