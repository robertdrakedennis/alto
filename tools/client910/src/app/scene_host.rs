//! `ViewerApp::scene` (code-quality programme Phase 4.4).
use super::*;

/// The installed scene (the scene rebuild and the scene graph): the floors and
/// their faithful GPU meshes, the placed scene graph and live scene,
/// static lights, the underwater level, loc-change slots, the build
/// preferences it was built with and the packets waiting for it.
pub(super) struct SceneHost {
    pub(super) applied_preferences: Option<crate::rebuild::BuildPrefs>,
    /// The current player level: the live scene's draw/roof level.
    pub(super) focus_level: u8,
    /// Floor configs for preference rebuilds (`None` without a pack).
    pub(super) flo: Option<FloStore>,
    /// Faithful floors per level (CPU; uploaded in `resumed`).
    pub(super) floors: Vec<Option<crate::floor::FloorGeometry>>,
    /// Materials for the floor textures.
    pub(super) material_store: Option<crate::texture::MaterialStore>,
    /// Pack root for floor texture loads (`None`: no pack-backed scene).
    pub(super) pack_root: Option<PathBuf>,
    /// `sceneBaseTile` of `floors`.
    pub(super) floor_base: (i32, i32),
    /// The faithful GPU toolkit's scene resources (uploaded floors, floor
    /// passes, scene and transient models, material state; renderer plan
    /// A1).
    pub(super) meshes: crate::scene_meshes::SceneMeshes,
    /// The underwater level height map 0, uploaded into the renderer's
    /// environment passes.
    pub(super) underwater_floor: Option<crate::floor::FloorGeometry>,
    /// The underwater scene's loc models, uploaded with the floors.
    pub(super) underwater_models: Vec<crate::rebuild::UnderwaterModel>,
    /// The placed scene graph (phase C).
    pub(super) graph: Option<crate::scene::Scene>,
    pub(super) live: Option<crate::live_scene::LiveScene>,
    /// Baked static lights per level (CPU side).
    pub(super) lights: Vec<Vec<crate::floorlight::BakedLight>>,
    /// Per loc-change slot, what the replacement last applied
    /// (`apply_location_changes`); reset with the scene.
    pub(super) loc_changes: HashMap<(i32, i32, i32, i32), LocChangeState>,
    /// Loc lookup slots of the installed scene, built on
    /// the first loc change.
    pub(super) loc_slots: Option<HashMap<(i32, i32, i32, i32), LocSlot>>,
    /// Point-light packets that arrived before the live scene owner existed.
    pub(super) pending_point_lights: Vec<crate::session::UiEvent>,
    /// Hint-arrow packets that arrived before the live map base was installed.
    pub(super) pending_hint_arrows: Vec<Vec<u8>>,
    /// The scene has no GPU behind it (a headless run): loc changes write the
    /// CPU scene (slot models, shadows, occluders) and leave the meshes alone.
    pub(super) meshless: bool,
}

/// A loc-change request target as the replacement applies it
/// (new id, shape, angle and transform; `id < 0` removes).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LocChangeSig {
    pub(super) id: i32,
    pub(super) shape: i32,
    pub(super) angle: i32,
    pub(super) transform: Option<[f32; 10]>,
}

/// The occupants the retained loc-change requests give the scene (the
/// replacement of the new id, as the minimap reads them). A `remove`
/// request restores the map's own loc, which the scene graph still holds; a
/// later request for a slot replaces an earlier one, as in
/// [`ViewerApp::apply_location_changes`].
pub(super) fn minimap_loc_overrides(
    requests: &[crate::protocol910::zone_state::Location],
) -> crate::minimap::LocOverrides {
    let mut slots = HashMap::new();
    for r in requests {
        let occupant = (!r.remove).then(|| u32::try_from(r.id).ok().map(|id| (id, r.shape)));
        slots.insert((r.level, r.layer, r.x, r.z), occupant);
    }
    slots
        .into_iter()
        .filter_map(|(key, occupant)| occupant.map(|o| (key, o)))
        .collect()
}

/// A loc slot's zone key: `(level, layer, x, z)`.
pub(super) type SlotKey = (i32, i32, i32, i32);

/// The loc each changed static slot holds now, for the pick list: the slot's
/// own scene entity is still the map's loc. A slot emptied by a removal holds
/// nothing (its model is gone, so nothing draws or picks).
pub(super) fn slot_occupants(
    changes: &HashMap<SlotKey, LocChangeState>,
    slots: Option<&HashMap<SlotKey, LocSlot>>,
) -> crate::player_picking::SlotOccupants {
    let Some(slots) = slots else {
        return Default::default();
    };
    changes
        .iter()
        .filter_map(|(key, state)| {
            let applied = state.applied.as_ref().filter(|sig| sig.id >= 0)?;
            let (_, source) = slots.get(key)?.primary;
            Some((
                source,
                crate::player_picking::SlotOccupant {
                    id: applied.id,
                    shape: applied.shape,
                    angle: applied.angle,
                },
            ))
        })
        .collect()
}

/// One `(level, layer, x, z)` slot as the scene last applied it.
#[derive(Debug, Default)]
pub(super) struct LocChangeState {
    /// `None`: the map's own loc (never changed, or restored).
    pub(super) applied: Option<LocChangeSig>,
    /// The static slot's own models (primary, second wall/decor) while
    /// another occupant is applied.
    pub(super) original_models: Vec<Option<crate::gpumodel::GpuModel>>,
    /// The applied replacement's stamped shadow `(shadow, occludeLevel, x, z)`.
    pub(super) shadow: Option<(crate::hardshadow::HardShadow, usize, i32, i32)>,
    /// The applied replacement is a `DynamicLoc` living in the slot's
    /// dynamic entity (its model, animation and shadow are `DynamicScene`'s).
    pub(super) dynamic_owner: bool,
    /// The static replacement model last installed into this slot.
    pub(super) installed_model: Option<(LocChangeSig, Option<i64>)>,
}

/// The static scene entities found at a loc slot:
/// `(live entity id, scene entity)` of the located loc and of the second
/// wall / wall decoration the removal drops with it.
#[derive(Clone, Copy, Debug)]
pub(super) struct LocSlot {
    pub(super) primary: (usize, crate::scene::EntityRef),
    pub(super) secondary: Option<(usize, crate::scene::EntityRef)>,
}

impl LocSlot {
    pub(super) fn entities(self) -> impl Iterator<Item = (usize, crate::scene::EntityRef)> {
        std::iter::once(self.primary).chain(self.secondary)
    }
}

/// [`crate::locs::location_refs`] resolved to live entity ids.
pub(super) fn location_slots(
    scene: &crate::scene::Scene,
    live: &crate::live_scene::LiveScene,
) -> HashMap<(i32, i32, i32, i32), LocSlot> {
    let ids: HashMap<crate::scene::EntityRef, usize> = live
        .entities
        .iter()
        .take(live.static_entity_count)
        .enumerate()
        .map(|(id, e)| (e.source, id))
        .collect();
    crate::locs::location_refs(scene)
        .into_iter()
        .filter_map(|(key, (primary, secondary))| {
            Some((
                key,
                LocSlot {
                    primary: (*ids.get(&primary)?, primary),
                    secondary: secondary.and_then(|s| Some((*ids.get(&s)?, s))),
                },
            ))
        })
        .collect()
}

/// The static-slot mesh owners a loc change writes.
pub(super) struct SlotMeshes<'a> {
    /// `None` for a meshless scene: the slot's CPU state is written, no mesh.
    pub(super) renderer: Option<&'a mut ActiveToolkit>,
    pub(super) pack: &'a Pack,
    pub(super) materials: &'a crate::texture::MaterialStore,
    pub(super) floor_base: (i32, i32),
    pub(super) meshes: &'a mut crate::scene_meshes::SceneMeshes,
}

impl SlotMeshes<'_> {
    /// Put `model` (or nothing) into a static scene slot: the entity's draw
    /// fields (overlay height, transparency, bounds, cylinder) and its mesh.
    pub(super) fn install(
        &mut self,
        scene: &mut crate::scene::Scene,
        entity: &mut crate::draw::DrawEntity,
        id: usize,
        model: Option<crate::gpumodel::GpuModel>,
    ) -> anyhow::Result<()> {
        let source = entity.source;
        let Some(mut model) = model else {
            *crate::dynamic_scene::model_mut(scene, source) = None;
            entity.overlay_height = 0;
            entity.transparent = false;
            entity.bounds = None;
            entity.cylinder = None;
            self.meshes.clear_slot(id);
            return Ok(());
        };
        model.radius();
        entity.overlay_height = model.min_y();
        entity.transparent = model.has_transparency || model.has_particles;
        entity.bounds = Some([
            entity.x + model.min_x(),
            entity.y + model.min_y(),
            entity.z + model.min_z(),
            model.max_x() - model.min_x(),
            model.max_y() - model.min_y(),
            model.max_z() - model.min_z(),
        ]);
        entity.cylinder = if model.unique_count == 0 {
            None
        } else {
            Some([
                entity.x,
                entity.y,
                entity.z,
                model.min_y(),
                model.max_y(),
                model.horizontal_radius(),
            ])
        };
        let pos = match source {
            crate::scene::EntityRef::Scenery(i) => {
                let e = &scene.scenery[i];
                [e.x, e.y, e.z]
            }
            crate::scene::EntityRef::Wall(i) => {
                let e = &scene.walls[i];
                [e.x, e.y, e.z]
            }
            crate::scene::EntityRef::WallDecor(i) => {
                let e = &scene.wall_decors[i];
                [e.x + e.offset_x, e.y, e.z + e.offset_z]
            }
            crate::scene::EntityRef::GroundDecor(i) => {
                let e = &scene.ground_decors[i];
                [e.x, e.y, e.z]
            }
            crate::scene::EntityRef::Temporary(_) => return Ok(()),
        };
        *crate::dynamic_scene::model_mut(scene, source) = Some(model.clone());
        let Some(renderer) = self.renderer.as_deref_mut() else {
            return Ok(());
        };
        if renderer.kind() == crate::active_toolkit::RendererKind::Modern
            && !renderer.toolkit0()
            && !rs910_render_modern::modern_debug_flags::flags().check
        {
            return Ok(());
        }
        let origin = [
            (self.floor_base.0 * 512 + pos[0]) as f32,
            pos[1] as f32,
            (self.floor_base.1 * 512 + pos[2]) as f32,
        ];
        self.meshes.install_slot_mesh(
            renderer.faithful(),
            self.pack,
            self.materials,
            id,
            &model,
            origin,
        )
    }
}

impl ViewerApp {
    /// Upload the faithful floors (all levels) through `Renderer::build_floor_mesh`.
    pub(super) fn upload_floors(&mut self) {
        rs910_core::profile::scope!("scene upload floors");
        self.scene.meshes.reset_scene();
        let faithful = self.faithful_scene_required();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let (Some(materials), Some(_)) = (
            self.scene.material_store.as_ref(),
            self.scene.pack_root.as_ref(),
        ) else {
            self.scene.meshes.floor_meshes = vec![None, None, None, None];
            return;
        };
        let pack = self.pack.clone();
        let uploaded = if faithful {
            self.scene.meshes.upload_floors(
                renderer.faithful(),
                &rs910_render_gpu::scene_meshes::FloorUpload {
                    pack: &pack,
                    materials,
                    floors: &self.scene.floors,
                    floor_base: self.scene.floor_base,
                    underwater_floor: self.scene.underwater_floor.as_ref(),
                    underwater_models: &self.scene.underwater_models,
                },
            )
        } else {
            self.scene.meshes.floor_meshes = (0..self.scene.floors.len()).map(|_| None).collect();
            Ok(())
        };
        // The scene draw plans the underwater lists each frame.
        if let Some(live) = self.scene.live.as_mut() {
            live.set_underwater(uploaded.ok().and(self.scene.underwater_floor.as_ref()).map(
                |floor| {
                    (
                        self.scene
                            .underwater_models
                            .iter()
                            .map(|m| m.entity.clone())
                            .collect(),
                        vec![floor.heights.clone()],
                    )
                },
            ));
        }
        // The minimap rebuild (on a toolkit delete or a world rebuild) and
        // the forced ground level for the new window.
        self.minimap.rebuild();
        if let Some(geometry) = self.scene.floors.iter().flatten().next() {
            self.minimap
                .init_force_ground_level(geometry.tiles_x, geometry.tiles_z);
        }
        // The static lights and hard-shadow blocks of every level.
        if faithful {
            self.scene.meshes.upload_floor_passes(
                renderer.faithful_ref(),
                &self.scene.floors,
                &self.scene.lights,
                self.scene.floor_base,
            );
        }
        self.upload_scene();
    }

    /// Upload every static scene-graph model (`Scene.opaqueEntities` /
    /// `transparentEntities`; `SceneMeshes::upload_scene`).
    pub(super) fn upload_scene(&mut self) {
        rs910_core::profile::scope!("scene upload models");
        self.scene.meshes.begin_scene_upload();
        if !self.faithful_scene_required() {
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let (Some(materials), Some(_), Some(scene)) = (
            self.scene.material_store.as_ref(),
            self.scene.pack_root.as_ref(),
            self.scene.graph.as_ref(),
        ) else {
            return;
        };
        let pack = self.pack.clone();
        self.scene.meshes.upload_scene(
            renderer.faithful(),
            &pack,
            materials,
            scene,
            self.scene.live.as_ref(),
            self.scene.floor_base,
        );
    }

    pub(super) fn refresh_dynamic_scene(
        &mut self,
        sun: &crate::floor::SunLighting,
    ) -> anyhow::Result<()> {
        let faithful = self.faithful_scene_required();
        let mut renderer = self.renderer.as_mut();
        if renderer.is_none() && !self.scene.meshless {
            return Ok(());
        }
        let (Some(live), Some(scene), Some(materials)) = (
            self.scene.live.as_mut(),
            self.scene.graph.as_mut(),
            self.scene.material_store.as_ref(),
        ) else {
            return Ok(());
        };
        let cycle = if self.core.session.as_ref().is_some_and(|s| s.game.is_some()) {
            self.core.cycle
        } else {
            live.animation_cycle()
        };
        let Some(dynamic) = live.dynamic.as_mut() else {
            return Ok(());
        };
        dynamic.set_hard_shadow_raster(faithful);
        let phases = [
            live.draw.plan.culled_updates.clone(),
            live.draw.plan.dispatch_opaque.clone(),
            live.draw.plan.dispatch_transparent.clone(),
        ];
        let game = self.core.session.game_mut();
        // Multi-locs read the cutscene var domain while `sceneState == 0`.
        dynamic.var_overrides = game
            .as_ref()
            .filter(|g| g.cutscene.scene_state == 0)
            .map(|g| g.cutscene.var_overrides.clone())
            .unwrap_or_default();
        let variables = game.and_then(|g| g.runtime.feed.state.varps.as_mut());
        dynamic.with_variables(variables, |dynamic| {
            for (phase, ids) in phases.iter().enumerate() {
                for &id in ids {
                    if !dynamic.contains(id) {
                        continue;
                    }
                    let entity = &mut live.entities[id];
                    dynamic.refresh(
                        id,
                        phase != 0,
                        cycle,
                        entity,
                        crate::dynamic_scene::RefreshWorld {
                            scene,
                            floors: &mut self.scene.floors,
                            materials,
                            sun,
                        },
                    )?;
                    if phase == 0 || !faithful {
                        continue;
                    }
                    let Some(renderer) = renderer.as_deref_mut() else {
                        continue;
                    };
                    let revision = dynamic.revision(id);
                    self.scene.meshes.refresh_dynamic_mesh(
                        renderer.faithful(),
                        &rs910_render_gpu::scene_meshes::DynamicRefresh {
                            pack: &dynamic.pack,
                            materials,
                            scene,
                            entity,
                            id,
                            revision,
                            floor_base: self.scene.floor_base,
                        },
                    )?;
                }
                if let (true, 1, Some(renderer)) = (faithful, phase, renderer.as_deref()) {
                    // The floors are drawn now. Transparent loc shadow changes remain
                    // in CPU masks and become visible at the next floor pass.
                    self.scene.meshes.update_dynamic_shadows(
                        renderer.faithful_ref(),
                        &self.scene.floors,
                        &mut dynamic.dirty_shadows,
                    );
                }
            }
            Ok(())
        })?;
        let planes = &live.draw.plan.model_planes;
        let visible = |id: &usize| {
            crate::dynamic_scene::model(scene, live.entities[*id].source).is_some()
                && crate::draw::entity_model_visible(&live.entities[*id], planes)
        };
        live.draw.plan.opaque = live
            .draw
            .plan
            .dispatch_opaque
            .iter()
            .copied()
            .filter(visible)
            .collect();
        live.draw.plan.transparent = live
            .draw
            .plan
            .dispatch_transparent
            .iter()
            .copied()
            .filter(visible)
            .collect();
        Ok(())
    }

    /// Consume retained loc animation requests at the actual scene
    /// animation owner. DynamicScene activates matching static entities in
    /// place, so the existing mesh refresh path remains authoritative.
    pub(super) fn apply_pending_loc_animations(&mut self) {
        if self
            .scene
            .live
            .as_ref()
            .and_then(|live| live.dynamic.as_ref())
            .is_none()
        {
            return;
        }
        let requests = self
            .core
            .session
            .game_mut()
            .map(|game| game.take_loc_animations())
            .unwrap_or_default();
        if requests.is_empty() {
            return;
        }
        let cycle = self.core.cycle;
        let unresolved = {
            let Some(live) = self.scene.live.as_mut() else {
                return;
            };
            let Some(dynamic) = live.dynamic.as_mut() else {
                return;
            };
            let mut unresolved = Vec::new();
            for request in requests {
                match dynamic.start_animation_at(
                    &live.entities,
                    crate::dynamic_scene::LocTarget {
                        level: request.level,
                        tile: [request.x, request.z],
                        layer: request.layer,
                        shape: request.shape,
                        angle: request.angle,
                    },
                    crate::dynamic_scene::AnimationStart {
                        cycle,
                        sequence: request.sequence,
                        delay: request.delay,
                    },
                ) {
                    Ok(0) => unresolved.push(request),
                    Ok(_) => {}
                    Err(error) => {
                        crate::logging::warn_repeated!("[client910] loc animation: {error:#}")
                    }
                }
            }
            unresolved
        };
        if !unresolved.is_empty() {
            if let Some(game) = self.core.session.game_mut() {
                game.pending_loc_animations.extend(unresolved);
            }
        }
    }

    /// Applies the retained loc-change requests: each request whose target
    /// changed since the scene last applied it removes the slot's previous
    /// occupant — its hard shadow leaves the floors, its occluder is cleared,
    /// a dynamic loc stops — and then adds the new one: a static replacement
    /// stamps its own shadow and occlusion markers. A `remove` request
    /// restores the map's own loc. The client keeps no collision map
    /// (pathing is server-side). Each replacement ends with a minimap
    /// refresh at the local player's level, which rescans the changed scene
    /// for map-element locs.
    pub(super) fn apply_location_changes(
        &mut self,
        sun: &crate::floor::SunLighting,
    ) -> anyhow::Result<()> {
        type Key = (i32, i32, i32, i32);
        let (targets, prefs, replaced) = self
            .core
            .session
            .game()
            .map(|game| {
                let mut targets: HashMap<Key, Option<LocChangeSig>> = HashMap::new();
                for r in &game.runtime.feed.state.zones.locations {
                    targets.insert(
                        (r.level, r.layer, r.x, r.z),
                        (!r.remove).then_some(LocChangeSig {
                            id: r.id,
                            shape: r.shape,
                            angle: r.angle,
                            transform: r.transform,
                        }),
                    );
                }
                let prefs = crate::rebuild::BuildPrefs::from_options(
                    &game.ui_variables.queries.preferences.options,
                );
                let replaced = minimap_loc_overrides(&game.runtime.feed.state.zones.locations);
                (targets, Some(prefs), replaced)
            })
            .unwrap_or_default();
        let changed: Vec<Key> = targets
            .iter()
            .filter(|(key, target)| {
                self.scene
                    .loc_changes
                    .get(*key)
                    .map_or(target.is_some(), |state| state.applied != **target)
            })
            .map(|(key, _)| *key)
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        // A loc replacement ends with a minimap refresh. The minimap reads the scene graph through the
        // requests' occupants, which already hold these changes, so the
        // refresh needs none of the GPU slot work below.
        self.refresh_minimap_locs(&replaced);
        let Some(prefs) = prefs else {
            return Ok(());
        };
        let renderer = self.renderer.as_mut();
        if renderer.is_none() && !self.scene.meshless {
            return Ok(());
        }
        let (Some(scene), Some(live), Some(materials), Some(_)) = (
            self.scene.graph.as_mut(),
            self.scene.live.as_mut(),
            self.scene.material_store.as_ref(),
            self.scene.pack_root.as_ref(),
        ) else {
            return Ok(());
        };
        let pack = self.pack.clone();
        if self.entities.loc_store.is_none() {
            self.entities.loc_store = Some(crate::config::LocStore::load(&pack)?);
        }
        let store = self.entities.loc_store.as_ref().expect("loaded");
        let slots = self
            .scene
            .loc_slots
            .get_or_insert_with(|| location_slots(scene, live));
        let mut dirty: Vec<std::collections::HashSet<(usize, usize)>> =
            (0..self.scene.floors.len())
                .map(|_| Default::default())
                .collect();
        let mut ctx = SlotMeshes {
            renderer,
            pack: &pack,
            materials,
            floor_base: self.scene.floor_base,
            meshes: &mut self.scene.meshes,
        };
        for key in changed {
            let (level, layer, x, z) = key;
            let target = targets[&key].clone();
            let state = self.scene.loc_changes.entry(key).or_default();
            let slot = slots.get(&key).copied();
            // Diagnostic: per-level byte sums of the shadow masks.
            let texels = |floors: &[Option<crate::floor::FloorGeometry>]| -> Vec<u64> {
                floors
                    .iter()
                    .map(|f| {
                        f.as_ref()
                            .and_then(|f| f.hard_shadows.as_ref())
                            .map_or(0, |m| m.mask.iter().map(|&b| u64::from(b)).sum())
                    })
                    .collect()
            };
            let before = (
                texels(&self.scene.floors),
                live.occluder_count(),
                state.applied.clone(),
            );
            // Remove the current occupant at (level, layer, x, z).
            match (&state.applied, slot) {
                (None, Some(slot)) => {
                    let (id, source) = slot.primary;
                    let entity = &live.entities[id];
                    let occupant = crate::dynamic_scene::slot_loc(scene, source);
                    if entity.dynamic {
                        if let Some(dynamic) = live.dynamic.as_mut() {
                            dynamic.remove_entity(id, entity, &mut self.scene.floors, sun);
                            for (d, level_dirty) in
                                dirty.iter_mut().zip(dynamic.dirty_shadows.iter_mut())
                            {
                                d.extend(level_dirty.drain());
                            }
                        }
                    } else if occupant.has_hard_shadow {
                        // The entity's shadow is re-derived from its model.
                        if let Some(shadow) = crate::dynamic_scene::model_mut(scene, source)
                            .as_mut()
                            .and_then(|m| m.hard_shadow(sun))
                        {
                            crate::hardshadow::stamp_shadow(
                                &mut self.scene.floors,
                                &shadow,
                                occupant.occlude_level,
                                occupant.x,
                                occupant.z,
                                sun,
                                Some(&mut dirty),
                            );
                        }
                    }
                    if let Some(loc) = store.get(occupant.loc_id) {
                        for (kind, ox, oz) in crate::locs::remove_loc_occluders(
                            loc,
                            layer,
                            occupant.shape,
                            occupant.angle,
                            x,
                            z,
                        ) {
                            live.remove_occluder(scene, kind, level, ox, oz);
                        }
                    }
                    // Removing a wall or dynamic wall (and the decor,
                    // entity and ground forms): keep the models for a later
                    // restore, then empty the slots.
                    for (n, (id, source)) in slot.entities().enumerate() {
                        let model = crate::dynamic_scene::model_mut(scene, source).take();
                        if state.original_models.len() <= n {
                            state.original_models.push(model);
                        }
                        ctx.install(scene, &mut live.entities[id], id, None)?;
                        live.mark_slot_changed(id);
                    }
                }
                (Some(prev), _) if prev.id >= 0 => {
                    if let (true, Some(slot), Some(dynamic)) =
                        (state.dynamic_owner, slot, live.dynamic.as_mut())
                    {
                        let id = slot.primary.0;
                        dynamic.remove_entity(id, &live.entities[id], &mut self.scene.floors, sun);
                        for (d, level_dirty) in
                            dirty.iter_mut().zip(dynamic.dirty_shadows.iter_mut())
                        {
                            d.extend(level_dirty.drain());
                        }
                        state.dynamic_owner = false;
                    }
                    if let Some((shadow, occlude_level, sx, sz)) = state.shadow.take() {
                        crate::hardshadow::stamp_shadow(
                            &mut self.scene.floors,
                            &shadow,
                            occlude_level,
                            sx,
                            sz,
                            sun,
                            Some(&mut dirty),
                        );
                    }
                    if let Some(loc) = u32::try_from(prev.id).ok().and_then(|id| store.get(id)) {
                        for (kind, ox, oz) in crate::locs::remove_loc_occluders(
                            loc, layer, prev.shape, prev.angle, x, z,
                        ) {
                            live.remove_occluder(scene, kind, level, ox, oz);
                        }
                    }
                }
                _ => {}
            }
            // Add the target loc.
            match (&target, slot) {
                (None, Some(slot)) => {
                    let (id, source) = slot.primary;
                    for (n, (id, _)) in slot.entities().enumerate() {
                        let model = state.original_models.get_mut(n).and_then(Option::take);
                        ctx.install(scene, &mut live.entities[id], id, model)?;
                        live.mark_slot_changed(id);
                    }
                    state.original_models.clear();
                    let occupant = crate::dynamic_scene::slot_loc(scene, source);
                    let entity = &mut live.entities[id];
                    entity.loc_id = occupant.loc_id as i32;
                    entity.shape = occupant.shape;
                    entity.angle = occupant.angle;
                    let entity = &live.entities[id];
                    if entity.dynamic {
                        if let Some(dynamic) = live.dynamic.as_mut() {
                            dynamic.restore_entity(id, entity)?;
                            ctx.meshes.invalidate_dynamic_mesh(id);
                        }
                    } else if occupant.has_hard_shadow {
                        if let Some(shadow) = crate::dynamic_scene::model_mut(scene, source)
                            .as_mut()
                            .and_then(|m| m.hard_shadow(sun))
                        {
                            crate::hardshadow::apply_shadow(
                                &mut self.scene.floors,
                                &shadow,
                                occupant.occlude_level,
                                occupant.x,
                                occupant.z,
                                sun,
                                Some(&mut dirty),
                            );
                        }
                    }
                    if let Some(loc) = store.get(occupant.loc_id) {
                        for call in crate::locs::add_loc_occluders(
                            loc,
                            occupant.shape,
                            occupant.angle,
                            level,
                            x,
                            z,
                        ) {
                            live.set_level_occlude_map(scene, call);
                        }
                    }
                }
                (Some(next), _) if next.id >= 0 => {
                    let Some(loc) = u32::try_from(next.id).ok().and_then(|id| store.get(id)) else {
                        state.applied = target;
                        continue;
                    };
                    // The entity sits at the footprint centre on the
                    // bridge-adjusted level.
                    let (w, l) = if next.angle == 1 || next.angle == 3 {
                        (i32::from(loc.length), i32::from(loc.width))
                    } else {
                        (i32::from(loc.width), i32::from(loc.length))
                    };
                    let mut fx = (x << 9) + (w << 8);
                    let mut fz = (z << 9) + (l << 8);
                    if let Some(t) = next.transform {
                        fx += t[4] as i32;
                        fz += t[6] as i32;
                    }
                    let occlude_level = if level < 3 && live.tile_flags().is_link_below(x, z) {
                        level + 1
                    } else {
                        level
                    } as usize;
                    let is_static = (!loc.has_anim
                        || (loc.disable_anim_low_detail && prefs.anim_detail == 0))
                        && !loc.has_multiloc
                        && !loc.force_dynamic
                        && !loc.always_dynamic;
                    // Static entities with a hard shadow.
                    if is_static && loc.hardshadow && prefs.scenery_shadows != 0 {
                        let shadow = scene
                            .temporary
                            .iter_mut()
                            .find(|t| t.location_key == Some(key))
                            .and_then(|t| t.model.as_mut())
                            .and_then(|m| m.hard_shadow(sun));
                        if let Some(shadow) = shadow {
                            crate::hardshadow::apply_shadow(
                                &mut self.scene.floors,
                                &shadow,
                                occlude_level,
                                fx,
                                fz,
                                sun,
                                Some(&mut dirty),
                            );
                            state.shadow = Some((shadow, occlude_level, fx, fz));
                        }
                    }
                    // A dynamic replacement on a dynamic slot becomes that
                    // slot's new dynamic loc, which draws, animates and shadows
                    // itself.
                    if let (false, Some(slot), Some(dynamic)) =
                        (is_static, slot, live.dynamic.as_mut())
                    {
                        let id = slot.primary.0;
                        if live.entities[id].dynamic {
                            let entity = &mut live.entities[id];
                            entity.loc_id = next.id;
                            entity.shape = next.shape;
                            entity.angle = next.angle;
                            dynamic.restore_entity(id, entity)?;
                            // A fresh DynamicLoc restarts its model revisions.
                            ctx.meshes.invalidate_dynamic_mesh(id);
                            state.dynamic_owner = true;
                        }
                    }
                    for call in
                        crate::locs::add_loc_occluders(loc, next.shape, next.angle, level, x, z)
                    {
                        live.set_level_occlude_map(scene, call);
                    }
                }
                _ => {}
            }
            log::info!(
                "[client910] loc change {key:?} at logic cycle {} slot {:?}: {:?} -> {:?}; shadow mask sums {:?} -> {:?}, occluders {} -> {}",
                self.core.cycle,
                slot.map(|s| {
                    let dynamic = live.entities[s.primary.0].dynamic;
                    (crate::dynamic_scene::slot_loc(scene, s.primary.1).loc_id, if dynamic { "dynamic" } else { "static" })
                }),
                before.2.as_ref().map(|s| (s.id, s.shape, s.angle)),
                target.as_ref().map(|s| (s.id, s.shape, s.angle)),
                before.0,
                texels(&self.scene.floors),
                before.1,
                live.occluder_count(),
            );
            state.applied = target;
            state.installed_model = None;
        }
        // Re-upload the touched shadow blocks.
        if let Some(renderer) = ctx.renderer.as_deref() {
            ctx.meshes
                .update_shadow_blocks(renderer.faithful_ref(), &self.scene.floors, &dirty);
        }
        Ok(())
    }

    /// The minimap refresh at the local player's level: the local player's
    /// level and route
    /// head, whatever level the changed loc is on.
    pub(super) fn refresh_minimap_locs(&mut self, replaced: &crate::minimap::LocOverrides) {
        let Some(game) = self.core.session.game() else {
            return;
        };
        let (Some(scene), Some(terrain)) =
            (self.scene.graph.as_ref(), game.runtime.terrain.as_ref())
        else {
            return;
        };
        let Some(player) = game.runtime.feed.state.players.players[game.runtime.map.local].as_ref()
        else {
            return;
        };
        self.minimap.refresh(
            scene,
            terrain,
            player.level,
            Some([player.x[0], player.z[0]]),
            replaced,
        );
    }

    /// A loc replacement mutates the owning scene layer
    /// before the replacement is drawn. The transient builder tags each
    /// replacement with its zone key; promote that model into the matching
    /// static entity and mesh slot so the normal draw, occlusion and picking
    /// owners see the replacement as persistent scene state. A replacement
    /// on an empty slot stays with the transient owner. Shadows, occluders
    /// and the restore of the map's own loc are [`Self::apply_location_changes`].
    pub(super) fn apply_pending_location_models(
        &mut self,
        sun: &crate::floor::SunLighting,
    ) -> anyhow::Result<()> {
        self.apply_location_changes(sun)?;
        let states = self
            .core
            .session
            .game()
            .map(|game| {
                let zones = &game.runtime.feed.state.zones;
                let mut states = std::collections::HashMap::new();
                for request in zones.locations.iter().chain(&zones.customisations) {
                    // A remove request restores the map's own loc
                    // (apply_location_changes); it leaves the slot alone here.
                    if request.remove {
                        continue;
                    }
                    states.insert(
                        (request.level, request.layer, request.x, request.z),
                        request.id >= 0,
                    );
                }
                states
            })
            .unwrap_or_default();
        if states.is_empty() {
            return Ok(());
        }
        let signatures: HashMap<SlotKey, (LocChangeSig, Option<i64>)> = self
            .core
            .session
            .game()
            .map(|game| {
                game.runtime
                    .feed
                    .state
                    .zones
                    .locations
                    .iter()
                    .chain(&game.runtime.feed.state.zones.customisations)
                    .filter(|request| !request.remove)
                    .map(|request| {
                        (
                            (request.level, request.layer, request.x, request.z),
                            (
                                LocChangeSig {
                                    id: request.id,
                                    shape: request.shape,
                                    angle: request.angle,
                                    transform: request.transform,
                                },
                                request.custom.as_ref().map(|custom| custom.salt),
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let anim_detail = self.core.session.game().map_or(1, |game| {
            crate::rebuild::BuildPrefs::from_options(&game.ui_variables.queries.preferences.options)
                .anim_detail
        });
        let static_model = |key: &SlotKey| {
            signatures.get(key).is_some_and(|(signature, _)| {
                self.entities
                    .loc_store
                    .as_ref()
                    .and_then(|store| {
                        u32::try_from(signature.id)
                            .ok()
                            .and_then(|id| store.get(id))
                    })
                    .is_some_and(|loc| {
                        (!loc.has_anim || (loc.disable_anim_low_detail && anim_detail == 0))
                            && !loc.has_multiloc
                            && !loc.force_dynamic
                            && !loc.always_dynamic
                    })
            })
        };
        let renderer = self.renderer.as_mut();
        if renderer.is_none() && !self.scene.meshless {
            return Ok(());
        }
        let (Some(scene), Some(live), Some(materials), Some(_)) = (
            self.scene.graph.as_mut(),
            self.scene.live.as_mut(),
            self.scene.material_store.as_ref(),
            self.scene.pack_root.as_ref(),
        ) else {
            return Ok(());
        };
        let slots = self
            .scene
            .loc_slots
            .get_or_insert_with(|| location_slots(scene, live));
        let mut replacements = std::collections::HashMap::new();
        for temporary in &mut scene.temporary {
            let Some(key) = temporary.location_key else {
                continue;
            };
            if states.get(&key) != Some(&true) || !slots.contains_key(&key) {
                continue;
            }
            if self
                .scene
                .loc_changes
                .get(&key)
                .is_some_and(|c| c.dynamic_owner)
            {
                // DynamicScene owns this slot's model; the tagged temporary
                // is not drawn.
                temporary.transient = false;
                continue;
            }
            temporary.transient = false;
            if static_model(&key)
                && self
                    .scene
                    .loc_changes
                    .get(&key)
                    .is_some_and(|state| state.installed_model.as_ref() == signatures.get(&key))
            {
                continue;
            }
            let Some(model) = temporary.model.clone() else {
                continue;
            };
            // The model is now owned by the matching static layer. Keep the
            // tagged temporary in the scene vector for tile references, but
            // route its draw lookup away from the transient mesh owner.
            replacements.insert(key, model);
        }
        let pack = self.pack.clone();
        let mut ctx = SlotMeshes {
            renderer,
            pack: &pack,
            materials,
            floor_base: self.scene.floor_base,
            meshes: &mut self.scene.meshes,
        };
        for (key, enabled) in &states {
            let Some(slot) = slots.get(key) else {
                continue;
            };
            if self
                .scene
                .loc_changes
                .get(key)
                .is_some_and(|c| c.dynamic_owner)
            {
                continue;
            }
            let cached = !enabled || static_model(key);
            if cached
                && self
                    .scene
                    .loc_changes
                    .get(key)
                    .is_some_and(|state| state.installed_model.as_ref() == signatures.get(key))
            {
                continue;
            }
            if *enabled && !replacements.contains_key(key) {
                continue;
            }
            for (n, (id, _)) in slot.entities().enumerate() {
                // Removing a dynamic wall: a second wall leaves with the first.
                let model = if *enabled && n == 0 {
                    let Some(model) = replacements.get(key).cloned() else {
                        continue;
                    };
                    Some(model)
                } else {
                    None
                };
                ctx.install(scene, &mut live.entities[id], id, model)?;
                live.mark_slot_changed(id);
            }
            if cached {
                self.scene
                    .loc_changes
                    .entry(*key)
                    .or_default()
                    .installed_model = signatures.get(key).cloned();
            }
        }
        Ok(())
    }

    pub(super) fn prepare_frame_materials(&mut self, env: &crate::env::EnvFrame) {
        let faithful = self.faithful_scene_required();
        let (Some(renderer), Some(live)) = (self.renderer.as_mut(), self.scene.live.as_mut())
        else {
            return;
        };
        let flickering = self
            .core
            .session
            .game()
            .is_none_or(|g| g.ui_variables.queries.preferences.options.live().flickering);
        live.model_lights.animate(self.core.cycle, flickering);
        if !faithful {
            return;
        }
        self.scene
            .meshes
            .update_light_intensities(renderer.faithful_ref(), &live.model_lights.intensities);
        if crate::debug_flags::flags().light_trace {
            log::info!(
                "[lights] cycle={} flickering={} floor_meshes={} values={:?}",
                self.core.cycle,
                flickering,
                self.scene
                    .meshes
                    .light_meshes
                    .iter()
                    .map(Vec::len)
                    .sum::<usize>(),
                live.model_lights.intensities
            );
        }
        let resources = self
            .scene
            .pack_root
            .as_ref()
            .and(self.scene.material_store.as_ref())
            .map(|materials| (&self.pack, materials));
        let camera = self.view.camera.scene_camera(renderer.scene_size());
        self.scene.meshes.prepare_frame_materials(
            renderer.faithful(),
            rs910_render_gpu::scene_meshes::FrameMaterials {
                live,
                players: self.entities.players.as_mut(),
                resources,
                underwater_floor: self.scene.underwater_floor.as_ref(),
                lights: &self.scene.lights,
                camera: &camera,
                floor_base: self.scene.floor_base,
                env,
            },
        );
    }

    pub(super) fn apply_point_light_updates(&mut self, mut updates: Vec<crate::session::UiEvent>) {
        if self.scene.live.is_none() {
            self.scene.pending_point_lights.append(&mut updates);
            return;
        }
        updates.splice(0..0, std::mem::take(&mut self.scene.pending_point_lights));
        let cycle = self.core.cycle;
        let Some(live) = self.scene.live.as_mut() else {
            return;
        };
        for event in updates {
            match event {
                crate::session::UiEvent::PointLightColour {
                    duration,
                    colour,
                    id,
                } => {
                    live.model_lights.point_light_colour(
                        i32::from(id),
                        i32::from(duration),
                        colour,
                        cycle,
                    );
                }
                crate::session::UiEvent::PointLightIntensity {
                    duration,
                    intensity,
                    id,
                } => {
                    live.model_lights.point_light_intensity(
                        i32::from(id),
                        i32::from(duration),
                        intensity,
                        cycle,
                    );
                }
                _ => {}
            }
        }
    }

    pub(super) fn apply_hint_arrow_updates(&mut self, updates: Vec<Vec<u8>>) {
        let base = self.core.session.as_ref().and_then(|session| {
            session
                .game
                .as_ref()
                .map(|game| [game.runtime.map.base_x, game.runtime.map.base_z])
        });
        apply_hint_arrows(
            &mut self.minimap,
            &mut self.scene.pending_hint_arrows,
            base,
            updates,
        );
    }

    /// Rebuild the current scene from its existing map identity. Preference
    /// changes never reinstall player state or manufacture a map transition.
    ///
    /// This is the world rebuild plus the next mainloop's scene rebuild for a
    /// map whose squares are already loaded: in game, state 18 enters 3, the
    /// redraw flushes the LOADING box, the scene builds, and the rebuild
    /// returns to 18 queueing `MAP_BUILD_COMPLETE`.
    pub(super) fn apply_scene_preferences(&mut self) -> anyhow::Result<()> {
        let Some(session) = self.core.session.as_mut() else {
            return Ok(());
        };
        let Some(game) = session.game.as_mut() else {
            return Ok(());
        };
        if game.runtime.map_request.is_some() || !game.runtime.feed.state.initialized {
            return Ok(());
        }
        let prefs = crate::rebuild::BuildPrefs::from_options(
            &game.ui_variables.queries.preferences.options,
        );
        let effects = &game.ui_variables.queries.preferences.pending_effects;
        let rebuild = effects.iter().any(|e| {
            matches!(
                e,
                crate::ui_preferences::PreferenceEffect::SceneRebuild
                    | crate::ui_preferences::PreferenceEffect::ResetModelCaches
            )
        });
        if self.scene.applied_preferences == Some(prefs) && !rebuild {
            game.ui_variables.queries.preferences.drain_effects();
            return Ok(());
        }
        let rebuild_in_game = session.machine.state == crate::login_state::GAME;
        if rebuild_in_game {
            self.set_client_state(crate::login_state::REBUILD_GAME);
            let text =
                loading_host::rebuild_message_text(&self.core.session.as_ref().unwrap().rebuild);
            self.present_message_box(&text);
        }
        // The rebuild start time.
        let started = crate::logic_clock::monotonic_millis();
        let result = self.rebuild_installed_scene(prefs);
        if rebuild_in_game {
            if let Some(session) = self.core.session.as_mut().filter(|_| result.is_ok()) {
                if session.io.world.stream.is_some() {
                    let elapsed =
                        crate::logic_clock::monotonic_millis().wrapping_sub(started) as i32;
                    session
                        .io
                        .world
                        .pending_writes
                        .extend(crate::net::encode_map_build_complete(elapsed));
                }
            }
            self.leave_rebuild_state();
        }
        result
    }

    /// The scene build of [`Self::apply_scene_preferences`] over the installed
    /// map identity: the instanced-region landscape and loc readers for an instanced
    /// region, the normal readers otherwise.
    pub(super) fn rebuild_installed_scene(
        &mut self,
        prefs: crate::rebuild::BuildPrefs,
    ) -> anyhow::Result<()> {
        let Some(session) = self.core.session.as_mut() else {
            return Ok(());
        };
        let Some(game) = session.game.as_mut() else {
            return Ok(());
        };
        let connection = match session.io.world.stream.as_ref() {
            Some(stream) if session.io.world.pending_writes.is_empty() => {
                Some(crate::loading_connection::LoadingConnection::start(stream)?)
            }
            _ => None,
        };
        self.scene
            .pack_root
            .as_ref()
            .context("scene pack missing")?;
        let pack = self.pack.clone();
        let world = game
            .runtime
            .feed
            .state
            .world
            .as_ref()
            .context("current world missing")?;
        let tables = crate::maploader::FloTables::from_store(
            self.scene
                .flo
                .as_ref()
                .context("floor definitions missing")?,
        );
        let locs = config::LocStore::load(&pack)?;
        // The asynchronously rebuilt world's loc type list.
        locs.allow_members.set(game.allow_members);
        let materials = self
            .scene
            .material_store
            .as_ref()
            .context("materials missing")?;
        let mut result = if let Some(layout) = &game.runtime.installed_region {
            crate::rebuild::rebuild_region_world(
                &pack, &tables, materials, &locs, world, layout, &prefs,
            )?
        } else {
            crate::rebuild::rebuild_world(&pack, &tables, materials, &locs, world, &prefs)?
        };
        let graph = result
            .scene_graph
            .as_mut()
            .context("rebuilt scene missing")?;
        let mut live = crate::live_scene::LiveScene::new(
            graph,
            &result.scene.normal,
            result.flags.clone(),
            &result.env.lights,
        )
        .context("rebuilt scene floors missing")?;
        live.enable_dynamic_with_preferences(
            &pack,
            &locs,
            result.model_cache.clone(),
            game.runtime.feed.state.varps.as_ref(),
            &prefs,
        )?;
        game.capture_scene(graph);
        self.core.audio.pending_loc_sounds = Some(std::mem::take(&mut result.loc_sounds));
        self.scene.graph = result.scene_graph;
        self.scene.loc_changes.clear();
        self.scene.loc_slots = None;
        if let Some(old) = self.scene.live.as_ref() {
            live.model_lights
                .inherit_primary(&old.model_lights, self.core.cycle);
        }
        self.scene.live = Some(live);
        self.scene.underwater_floor = result.scene.underwater.first().cloned().flatten();
        self.scene.underwater_models = result.underwater_models;
        self.scene.floors = result.scene.normal;
        self.core.environment.env = Some(result.env);
        self.scene.lights = result.lights;
        if let Some(players) = &mut self.entities.players {
            players.set_model_detail(prefs.model_detail());
        }
        self.upload_floors();
        anyhow::ensure!(
            !self.faithful_scene_required()
                || (!self.scene.meshes.scene_upload_failed
                    && self.scene.meshes.floor_meshes.len() == self.scene.floors.len()
                    && self
                        .scene
                        .floors
                        .iter()
                        .zip(&self.scene.meshes.floor_meshes)
                        .all(|(floor, mesh)| floor.is_none() || mesh.is_some())),
            "settings scene resource upload failed"
        );
        if let Some(connection) = connection {
            connection.finish()?;
        }
        self.scene.applied_preferences = Some(prefs);
        self.clock.logic.reset();
        if let Some(game) = self.core.session.game_mut() {
            game.ui_variables.queries.preferences.drain_effects();
        }
        Ok(())
    }

    /// Rebuilds the base sprite when the player's level changed, and the
    /// minimap and compass draw inputs of this redraw.
    pub(super) fn minimap_frames(
        &mut self,
        env: &crate::env::EnvFrame,
    ) -> (
        Option<crate::minimap::Frame>,
        Option<crate::minimap::CompassFrame>,
    ) {
        let Some(session) = self.core.session.as_ref() else {
            return (None, None);
        };
        let Some(game) = session.game.as_ref() else {
            return (None, None);
        };
        if game.runtime.map_request.is_some() || !game.runtime.feed.state.initialized {
            return (None, None);
        }
        let Some(player) = game.runtime.feed.state.players.players[game.runtime.map.local].as_ref()
        else {
            return (None, None);
        };
        let level = player.level;
        let base = [game.runtime.map.base_x, game.runtime.map.base_z];
        if self.minimap.pack.is_none() && self.scene.pack_root.is_some() {
            self.minimap.install(&self.pack);
        }
        {
            let ui = &session.ui;
            self.minimap.logged_in_members = ui.engine.account.logged_in_members;
        }
        // The minimap reads the world's loc type list, whose members flag is
        // set at login.
        if let Some(locs) = &self.minimap.locs {
            locs.allow_members.set(game.allow_members);
        }
        // The area's map elements for this base.
        self.minimap.ensure_world_map(base, game.runtime.map.width);
        // Level change: the cached minimap level differs from the local player level and a scene exists.
        if self.minimap.cached_level != level {
            if let (Some(scene), Some(terrain), Some(renderer)) = (
                self.scene.graph.as_ref(),
                game.runtime.terrain.as_ref(),
                self.renderer.as_mut(),
            ) {
                // The minimap base rebuild waits for every visible
                // map-scene icon before replacing the cached base sprite.
                if !self.minimap.are_loc_icons_ready(
                    scene,
                    terrain,
                    level,
                    Some([player.x[0], player.z[0]]),
                ) {
                    return (None, None);
                }
                // The minimap base rebuild reads the route waypoint x/z[0] >> 3.
                let plan =
                    self.minimap
                        .base_plan(scene, terrain, level, Some([player.x[0], player.z[0]]));
                // the loc icon queue and the map element queue.
                self.minimap.refresh(
                    scene,
                    terrain,
                    level,
                    Some([player.x[0], player.z[0]]),
                    &minimap_loc_overrides(&game.runtime.feed.state.zones.locations),
                );
                self.minimap.queue_map_elements(
                    base,
                    terrain.width as i32,
                    terrain.height as i32,
                    level,
                );
                if let Some(old) = self.minimap.base.take() {
                    renderer.release_minimap_base(old.texture);
                }
                match renderer.render_minimap_base(
                    &plan,
                    &self.scene.floors,
                    &mut self.scene.meshes.floor_meshes,
                    self.scene.floor_base,
                    terrain.height as i32,
                    env,
                ) {
                    Ok(id) => {
                        self.minimap.base = Some(crate::minimap::Base {
                            texture: id,
                            size: plan.size,
                        });
                        self.minimap.cached_level = level;
                    }
                    Err(error) => {
                        crate::logging::warn_repeated!("[client910] minimap base: {error:#}")
                    }
                }
                // The frame's own tile selection replaced by the plan's: restore it.
                self.scene
                    .meshes
                    .reselect_floors(renderer.faithful_ref(), &self.scene.floors);
            }
        }
        let Some(terrain) = game.runtime.terrain.as_ref() else {
            return (None, None);
        };
        // The yaw source by camera state.
        let (cam2_yaw, orbit_yaw) = match &session.ui {
            ui if ui.engine.camera.cam2.camera_state == 3 => {
                (Some(ui.engine.camera.cam2.yaw()), game.camera.yaw)
            }
            _ => (None, game.camera.yaw),
        };
        let anticheat = self.minimap.anticheat_angle;
        let (map_angle, compass_yaw, overlay_angle) = match cam2_yaw {
            Some(yaw) => {
                let units = (f64::from(yaw) * 2607.5945876176133) as i32;
                (
                    (-units + anticheat) & 0x3FFF,
                    -units,
                    (units + anticheat) & 0x3FFF,
                )
            }
            None => (
                (anticheat + -(orbit_yaw as i32)) & 0x3FFF,
                -(orbit_yaw as i32),
                (anticheat + orbit_yaw as i32) & 0x3FFF,
            ),
        };
        // entity state for the overlays.
        let state = &game.runtime.feed.state;
        let social = Some(&session.ui.engine.social);
        let npcs: Vec<crate::minimap::NpcDot> = state
            .npcs
            .slots
            .iter()
            .filter_map(|slot| state.npcs.entities.get(slot).map(|n| (*slot, n)))
            .filter(|(_, n)| n.type_id != -1)
            .map(|(index, n)| crate::minimap::NpcDot {
                index,
                type_id: n.type_id,
                level: n.path.level,
                fine: [n.path.fine_x as i32, n.path.fine_z as i32],
            })
            .collect();
        let players: Vec<crate::minimap::PlayerDot> = state
            .players
            .high_indices
            .iter()
            .filter(|&&i| i != game.runtime.map.local)
            .filter_map(|&i| {
                state
                    .players
                    .players
                    .get(i)
                    .and_then(Option::as_ref)
                    .map(|p| (i, p))
            })
            .map(|(index, p)| crate::minimap::PlayerDot {
                index,
                has_model: p.appearance.model.is_some(),
                hidden: p.appearance.visibility.is_some_and(|v| v != 0),
                level: p.level,
                fine: [p.fine_x as i32, p.fine_z as i32],
                transmog_npc: p.appearance.model.as_ref().map_or(-1, |m| m.npc),
                partner: p.partner,
                suppress_partner: p.suppress_partner,
                team: p.appearance.team,
                friend: p.appearance.name.as_ref().is_some_and(|name| {
                    social.is_some_and(|social| {
                        social.friends.iter().any(|friend| {
                            friend.display_name.eq_ignore_ascii_case(name)
                                || friend.previous_name.eq_ignore_ascii_case(name)
                        })
                    })
                }),
                clan: p.appearance.name.as_ref().is_some_and(|name| {
                    social.is_some_and(|social| {
                        social.affined_channel.as_ref().is_some_and(|channel| {
                            channel
                                .users
                                .iter()
                                .any(|user| user.name.eq_ignore_ascii_case(name))
                        }) || social.listened_channel.as_ref().is_some_and(|channel| {
                            channel
                                .users
                                .iter()
                                .any(|user| user.name.eq_ignore_ascii_case(name))
                        })
                    })
                }),
            })
            .collect();
        let read = |bit: bool, id: i32| -> anyhow::Result<i32> {
            let varps = state.varps.as_ref().context("local player varps")?;
            if bit {
                let def = game
                    .inputs
                    .bits
                    .get(id, false)
                    .map_err(|e| anyhow::anyhow!("varbit {id}: {e:?}"))?;
                varps
                    .get_bit(&def)
                    .map_err(|e| anyhow::anyhow!("varbit {id}: {e:?}"))
            } else {
                varps
                    .get(id)
                    .map_err(|e| anyhow::anyhow!("varp {id}: {e:?}"))
            }
        };
        if crate::toolkit_debug_flags::flags().minimap_dump.is_some() {
            let types: Vec<i32> = npcs.iter().map(|n| n.type_id).collect();
            log::info!(
                "[minimap] input npcs {} (slots {}) {:?} players {} stacks {} level {} base {:?}",
                npcs.len(),
                state.npcs.slots.len(),
                &types[..types.len().min(10)],
                players.len(),
                state.zones.objects.key_order.len(),
                level,
                base
            );
        }
        let overlays = self.minimap.overlays(&crate::minimap::OverlayInput {
            base,
            player: [player.fine_x as i32, player.fine_z as i32],
            level,
            cycle: game.cycle,
            local_size: player.size,
            local_team: player.appearance.team,
            stacks: &state.zones.objects.key_order,
            npcs: &npcs,
            players: &players,
            read: &read,
            flag: self.core.minimap,
        });
        let frame = crate::minimap::Frame {
            base: self.minimap.base.clone(),
            toggle: self.core.minimap.toggle,
            player: [player.fine_x as i32, player.fine_z as i32],
            size_z: terrain.height as i32,
            angle: map_angle,
            scale: 4096 - self.minimap.zoom * 16,
            // No owner sets an invisible local player.
            player_hidden: false,
            overlay_angle,
            zoom: self.minimap.zoom,
            camera_state_4: false,
            overlays,
        };
        let compass = self
            .minimap
            .compass
            .clone()
            .map(|sprite| crate::minimap::CompassFrame {
                sprite,
                toggle: self.core.minimap.toggle,
                angle: (anticheat * 2 + compass_yaw) & 0x3FFF,
            });
        (Some(frame), compass)
    }

    /// Swap the scene assets and GPU meshes to the reloaded world and
    /// retarget the camera to the new spawn.
    pub(super) fn apply_reloaded_assets(
        &mut self,
        assets: WorldAssets,
        event: &crate::session::RebuildEvent,
    ) {
        rs910_core::profile::scope!("scene install");
        let ground = assets.spawn_ground();
        let (spawn_x, spawn_z) = (assets.spawn_x, assets.spawn_z);
        self.scene.focus_level = assets.focus_level;
        self.scene.flo = assets.flo;
        self.scene.floors = assets.floors;
        self.scene.underwater_floor = assets.underwater_floor;
        self.scene.underwater_models = assets.underwater_models;
        self.scene.material_store = assets.material_store;
        self.scene.pack_root = assets.pack_root;
        self.scene.floor_base = assets.floor_base;
        self.scene.graph = assets.scene_graph;
        self.scene.loc_changes.clear();
        self.scene.loc_slots = None;
        let mut live = assets.live_scene;
        if let (Some(new), Some(old)) = (live.as_mut(), self.scene.live.as_ref()) {
            new.model_lights
                .inherit_primary(&old.model_lights, self.core.cycle);
        }
        self.scene.live = live;
        self.core.environment.env = assets.env;
        self.scene.lights = assets.lights;
        self.core.audio.pending_loc_sounds = assets.loc_sounds;
        self.scene.meshes.selected_floors.clear();
        self.upload_floors();
        if !self.view.follow {
            self.view.camera.target = glam::Vec3::new(spawn_x as f32, ground, spawn_z as f32);
        }
        if let Some(session) = self.core.session.as_mut() {
            session.groups = event.groups.clone();
        }
        log::info!(
            "[client910] live reload ok: spawn {spawn_x},{spawn_z} groups {:?}",
            event.groups
        );
    }
}
