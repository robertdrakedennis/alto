//! Live adapter that builds, draws and updates the particles of player
//! bodies. Body particle anchors feed the player's particle system (merged
//! with its spot models by the app owner). The actor spot models themselves
//! are placed and drawn by the app's transient entity owner beside the body.
use crate::{
    actor_matrix::Matrix,
    animation_assets::AnimationAssets,
    billboard::BillboardStore,
    cache::Pack,
    floor_render::FloorMesh,
    game_runtime::Game,
    live_scene::LiveScene,
    particle::EmitterStore,
    player_body::{self, Resources},
    player_model::{Inputs, Models},
    scene::{EntityRef, Scene},
    texture::MaterialStore,
};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
pub struct Entry {
    pub mesh: Option<FloorMesh>,
    pub matrix: Matrix,
    pub visible: bool,
    pub shadow: Option<Shadow>,
    /// The local player's hint-arrow models, drawn after the shadow with
    /// depth writes off.
    pub hint_arrows: Vec<Shadow>,
    /// Particle anchors of the body model in scene-local fine units (the
    /// model's anchors through the draw matrix).
    pub particles: crate::particle::Binding,
}
/// The inputs of one frame's [`PlayersRenderer::refresh`].
pub struct PlayerRefresh<'a> {
    /// CPU poses and write-back run for every backend; only faithful draws need these meshes.
    pub faithful_uploads: bool,
    pub game: &'a mut Game,
    pub options: &'a crate::client_options::ClientOptions,
    pub live: &'a mut LiveScene,
    pub scene: &'a mut Scene,
    pub materials: &'a MaterialStore,
    pub scene_cycle: i32,
    pub pick_frame: &'a mut crate::player_picking::Frame,
    pub hint_arrows: &'a [Option<crate::minimap::HintArrow>],
    pub npc_store: Option<&'a crate::config::NpcStore>,
}
pub struct Shadow {
    pub mesh: Option<FloorMesh>,
    pub matrix: Matrix,
    pub visible: bool,
    /// The model this frame (the other backends draw it,
    /// `player_draw::ShadowDraw`).
    pub model: crate::gpumodel::GpuModel,
}
pub struct PlayersRenderer {
    pack: Pack,
    models: Models,
    shadows: crate::player_shadow::Shadows,
    assets: AnimationAssets,
    billboards: BillboardStore,
    emitters: EmitterStore,
    pub meshes: BTreeMap<usize, Entry>,
    hint_arrow_models: HintArrowModels,
}
/// The other backends' view of this frame's players: body matrix and
/// visibility, spot shadow, hint arrows.
impl crate::player_draw::PlayerDraws for PlayersRenderer {
    fn player_draw(&self, player: usize) -> Option<crate::player_draw::PlayerDraw<'_>> {
        let entry = self.meshes.get(&player)?;
        fn draw(shadow: &Shadow) -> crate::player_draw::ShadowDraw<'_> {
            crate::player_draw::ShadowDraw {
                model: &shadow.model,
                matrix: &shadow.matrix,
                visible: shadow.visible,
            }
        }
        Some(crate::player_draw::PlayerDraw {
            matrix: &entry.matrix,
            visible: entry.visible,
            shadow: entry.shadow.as_ref().map(draw),
            hint_arrows: entry.hint_arrows.iter().map(draw).collect(),
        })
    }
}
/// A small LRU cache (4 entries) of hint-arrow models.
#[derive(Default)]
struct HintArrowModels {
    models: BTreeMap<i32, crate::gpumodel::GpuModel>,
    order: std::collections::VecDeque<i32>,
}
impl HintArrowModels {
    /// The cached 2055-flag model lit at 64/768, copied, turned to the
    /// target (a Y rotation that leaves normals), ground-tilted and raised
    /// like the spot shadow.
    fn get(
        &mut self,
        r: &Resources,
        detail: i32,
        id: i32,
        angle: i32,
        ground: [i32; 3],
    ) -> Result<Option<crate::gpumodel::GpuModel>> {
        if let std::collections::btree_map::Entry::Vacant(slot) = self.models.entry(id) {
            let Some(mut raw) = u32::try_from(id)
                .ok()
                .and_then(|id| crate::modelunlit::ModelUnlit::load(r.pack, id).ok())
            else {
                return Ok(None);
            };
            if raw.version < 13 {
                raw.scale_by_power_of_two(2);
            }
            let model = crate::gpumodel::GpuModel::new(
                &crate::gpumodel::ModelStores {
                    materials: r.materials,
                    billboards: r.billboards,
                    emitters: r.emitters,
                },
                &raw,
                crate::gpumodel::BuildParams {
                    flags: 2055,
                    ambient: 64,
                    contrast: 768,
                    detail,
                },
            )?;
            slot.insert(model);
        }
        self.order.retain(|&k| k != id);
        self.order.push_back(id);
        while self.order.len() > 4 {
            if let Some(evicted) = self.order.pop_front() {
                self.models.remove(&evicted);
            }
        }
        let mut model = self.models[&id].clone();
        if angle != 0 {
            model.rotate_y_keep_normals(angle);
        }
        if ground[0] != 0 {
            model.rotate_x(ground[0]);
        }
        if ground[1] != 0 {
            model.rotate_z(ground[1]);
        }
        if ground[2] != 0 {
            model.translate(0, ground[2], 0);
        }
        Ok(Some(model))
    }
}
impl PlayersRenderer {
    pub fn new(pack: &Pack) -> Result<Self> {
        Ok(Self {
            pack: pack.clone(),
            models: Models::load(pack, 0x37)?,
            shadows: Default::default(),
            assets: AnimationAssets::load(pack)?,
            billboards: BillboardStore::load(pack)?,
            emitters: EmitterStore::load(pack)?,
            meshes: BTreeMap::new(),
            hint_arrow_models: Default::default(),
        })
    }

    /// Apply the model detail flags to body and spot-shadow caches.
    /// Returns false when no cache invalidation was needed.
    pub fn set_model_detail(&mut self, detail: i32) -> bool {
        let changed = self.models.set_detail(detail);
        if changed {
            // The hint-arrow model cache resets with the detail.
            self.hint_arrow_models = Default::default();
        }
        changed | self.shadows.set_detail(detail)
    }
    pub fn refresh(
        &mut self,
        (renderer, gpu): crate::render::Faithful<'_>,
        inputs: PlayerRefresh<'_>,
    ) -> Result<()> {
        let PlayerRefresh {
            faithful_uploads,
            game,
            options,
            live,
            scene,
            materials,
            scene_cycle,
            pick_frame,
            hint_arrows,
            npc_store,
        } = inputs;
        self.meshes.retain(|id, _| {
            game.runtime
                .feed
                .state
                .players
                .players
                .get(*id)
                .is_some_and(Option::is_some)
        });
        let defaults = &game.inputs.appearance.defaults;
        let varps = game.runtime.feed.state.varps.as_ref();
        let bits = &game.inputs.bits;
        // The body model resolves the multi-NPC through the local player's game state.
        let vars = |bit: bool, id: i32| -> Option<i32> {
            if bit {
                let definition = bits.get(id, false).ok()?;
                varps?.get_bit(&definition).ok()
            } else {
                varps?.get(id).ok()
            }
        };
        let r = Resources {
            pack: &self.pack,
            types: Inputs {
                items: &game.inputs.appearance.types.items,
                bases: &game.inputs.bas,
                wear: &defaults.wear,
                recolour: defaults
                    .graphics
                    .recolour
                    .as_ref()
                    .context("player colour palette")?,
                retexture: defaults
                    .graphics
                    .retexture
                    .as_ref()
                    .context("player texture palette")?,
            },
            materials,
            billboards: &self.billboards,
            emitters: &self.emitters,
            npcs: npc_store.map(|store| crate::player_body::NpcTypes { store, vars: &vars }),
        };
        for (draw, ids) in [
            (false, live.draw.plan.culled_updates.clone()),
            (true, live.draw.plan.dispatched.clone()),
        ] {
            for id in ids {
                let EntityRef::Temporary(index) = live.entities[id].source else {
                    continue;
                };
                // Location replacements promoted into the static layer keep a
                // non-transient temporary for tile references only
                // (`apply_pending_location_models`); they are not players.
                if scene.temporary[index].transient || scene.temporary[index].location_key.is_some()
                {
                    continue;
                }
                let player = scene.temporary[index].player;
                let arrow_targets = if draw && player == game.runtime.map.local {
                    hint_targets(game, player, hint_arrows)
                } else {
                    Vec::new()
                };
                if crate::render_debug_flags::flags().hint_trace && !arrow_targets.is_empty() {
                    log::info!("[client910] hint arrow targets {arrow_targets:?}");
                }
                let e = game.runtime.feed.state.players.players[player]
                    .as_mut()
                    .context("temporary player disappeared")?;
                let use_idle = e.actor.scene.use_idle;
                let Some(mut body) = player_body::build(
                    &mut self.models,
                    &mut self.assets,
                    &r,
                    e,
                    game.runtime.terrain.as_ref(),
                    player_body::BodyRequest {
                        cycle: game.cycle,
                        flags: if draw { 2048 } else { 0 },
                        use_idle,
                    },
                )?
                else {
                    continue;
                };
                e.actor.scene.min_y = body.min_y;
                e.actor.scene.height = body.height;
                e.actor.scene.ground = body.ground;
                if !draw {
                    continue;
                }
                let shadow_enabled = options.live().character_shadows
                    && (e.appearance.bas == -1
                        || game
                            .inputs
                            .bas
                            .get(&e.appearance.bas)
                            .is_some_and(|b| b.casts_shadow));
                let mut shadow = if shadow_enabled {
                    let g = &defaults.graphics.scalars;
                    let shape = if g.spotshadowtexture >= 0 {
                        crate::player_shadow::Shape::Texture {
                            material: g.spotshadowtexture as i16,
                            alpha: g.spotshadowtexture_alpha as i8,
                        }
                    } else {
                        crate::player_shadow::Shape::Rings {
                            size: 1,
                            colours: [0, 0],
                            alpha: [160, 240],
                        }
                    };
                    Some(self.shadows.build(
                        (&r).into(),
                        &mut self.assets,
                        &mut body.model,
                        shape,
                        body.ground,
                        crate::player_shadow::animation(e),
                    )?)
                } else {
                    None
                };
                let decor = scene
                    .tile(
                        e.level as usize,
                        (e.fine_x as usize) >> 9,
                        (e.fine_z as usize) >> 9,
                    )
                    .and_then(|t| t.ground_decoration)
                    .map(|i| scene.ground_decors[i].decor_height);
                let s = &mut e.actor.scene;
                let delta = if let Some(h) = decor {
                    s.decoration_offset.wrapping_sub(h)
                } else {
                    s.decoration_offset
                };
                s.decoration_offset = (s.decoration_offset as f32 - delta as f32 / 10.) as i32;
                let matrix = Matrix::actor(
                    e.actor.rotation,
                    [e.fine_x, e.motion.y, e.fine_z],
                    (-5i32).wrapping_sub(s.decoration_offset) as f32,
                );
                let entry = self.meshes.entry(player).or_insert_with(|| Entry {
                    mesh: None,
                    matrix,
                    visible: true,
                    shadow: None,
                    hint_arrows: Vec::new(),
                    particles: Default::default(),
                });
                entry.matrix = matrix;
                if faithful_uploads {
                    let reused = match entry.mesh.as_mut() {
                        Some(mesh) => {
                            renderer.update_model_mesh(gpu, mesh, materials, &body.model)?
                        }
                        None => false,
                    };
                    if !reused {
                        let mesh = renderer.build_model_mesh(
                            gpu,
                            &self.pack,
                            materials,
                            &body.model,
                            [0.; 3],
                            "player body",
                        )?;
                        *entry = Entry {
                            mesh: Some(mesh),
                            matrix,
                            visible: true,
                            shadow: None,
                            hint_arrows: Vec::new(),
                            particles: Default::default(),
                        };
                    }
                }
                s.transparent = shadow.is_some() || body.model.has_transparency;
                s.draw_cycle = scene_cycle;
                live.entities[id].precise_cylinder =
                    crate::actor_render::cylinder(&mut body.model, &matrix);
                let entry = self.meshes.get_mut(&player).unwrap();
                entry.visible = body.model.unique_count != 0
                    && crate::draw::entity_model_visible(
                        &live.entities[id],
                        &live.draw.plan.model_planes,
                    );
                // Bind the body model's particle anchors through the draw
                // matrix.
                let mut particles = crate::particle::Binding {
                    key: crate::particle::keys::player(player),
                    level: e.level,
                    ..Default::default()
                };
                particles.add_model(
                    &body.model,
                    &matrix,
                    crate::particle::Rotation::IDENTITY,
                    crate::particle::keys::BODY,
                );
                entry.particles = particles;
                if let Some(model) = shadow.as_mut() {
                    let matrix = Matrix::actor(
                        e.actor.rotation,
                        [e.fine_x, e.motion.y, e.fine_z],
                        (-20i32).wrapping_sub(s.decoration_offset) as f32,
                    );
                    let visible = crate::actor_render::cylinder(model, &matrix).is_some_and(|c| {
                        crate::draw::precise_cylinder_visible(c, &live.draw.plan.model_planes)
                    });
                    let reused = if let Some(old) = entry.shadow.as_mut() {
                        old.matrix = matrix;
                        old.visible = visible;
                        old.model = model.clone();
                        if faithful_uploads {
                            match old.mesh.as_mut() {
                                Some(mesh) => {
                                    renderer.update_model_mesh(gpu, mesh, materials, model)?
                                }
                                None => false,
                            }
                        } else {
                            true
                        }
                    } else {
                        false
                    };
                    if !reused {
                        let mesh = if faithful_uploads {
                            let mut mesh = renderer.build_model_mesh(
                                gpu,
                                &self.pack,
                                materials,
                                model,
                                [0.; 3],
                                "player shadow",
                            )?;
                            mesh.set_depth_write(false);
                            Some(mesh)
                        } else {
                            None
                        };
                        entry.shadow = Some(Shadow {
                            mesh,
                            matrix,
                            visible,
                            model: model.clone(),
                        });
                    }
                } else {
                    entry.shadow = None;
                }
                // The local player's hint arrows share the shadow's draw
                // matrix (-20 - decoration offset).
                let arrow_matrix = Matrix::actor(
                    e.actor.rotation,
                    [e.fine_x, e.motion.y, e.fine_z],
                    (-20i32).wrapping_sub(s.decoration_offset) as f32,
                );
                let angle = e.angle & 16383;
                let mut arrows = Vec::new();
                for ([dx, dz], model, range) in arrow_targets {
                    // drawHintArrow :455-467.
                    let distance = dx * dx + dz * dz;
                    if distance < 262_144 || distance > range {
                        continue;
                    }
                    let turn = ((dx as f64).atan2(dz as f64) * 2607.5945876176133
                        - f64::from(angle)) as i32
                        & 0x3FFF;
                    if let Some(model) = self.hint_arrow_models.get(
                        &r,
                        self.models.detail,
                        model,
                        turn,
                        body.ground,
                    )? {
                        arrows.push(model);
                    }
                }
                if crate::render_debug_flags::flags().hint_trace && !arrows.is_empty() {
                    log::info!("[client910] hint arrow models {}", arrows.len());
                }
                let entry = self.meshes.get_mut(&player).unwrap();
                entry.hint_arrows.truncate(arrows.len());
                for (slot, model) in arrows.iter_mut().enumerate() {
                    let visible =
                        crate::actor_render::cylinder(model, &arrow_matrix).is_some_and(|c| {
                            crate::draw::precise_cylinder_visible(c, &live.draw.plan.model_planes)
                        });
                    let reused = if let Some(old) = entry.hint_arrows.get_mut(slot) {
                        old.matrix = arrow_matrix;
                        old.visible = visible;
                        old.model = model.clone();
                        if faithful_uploads {
                            match old.mesh.as_mut() {
                                Some(mesh) => {
                                    renderer.update_model_mesh(gpu, mesh, materials, model)?
                                }
                                None => false,
                            }
                        } else {
                            true
                        }
                    } else {
                        false
                    };
                    if !reused {
                        let mesh = if faithful_uploads {
                            let mut mesh = renderer.build_model_mesh(
                                gpu,
                                &self.pack,
                                materials,
                                model,
                                [0.; 3],
                                "hint arrow",
                            )?;
                            mesh.set_depth_write(false);
                            Some(mesh)
                        } else {
                            None
                        };
                        let arrow = Shadow {
                            mesh,
                            matrix: arrow_matrix,
                            visible,
                            model: model.clone(),
                        };
                        if slot < entry.hint_arrows.len() {
                            entry.hint_arrows[slot] = arrow;
                        } else {
                            entry.hint_arrows.push(arrow);
                        }
                    }
                }
                let raw = crate::player_picking::bounds(&mut body.model);
                let pick_matrix =
                    Matrix::actor(e.actor.rotation, [e.fine_x, e.motion.y, e.fine_z], 0.);
                let position = [
                    e.fine_x,
                    (e.motion.y as i32).wrapping_add(live.entities[id].overlay_height >> 1) as f32,
                    e.fine_z,
                ];
                let depth = crate::animation_matrix::transform(
                    &pick_frame.view,
                    position[0],
                    position[1],
                    position[2],
                )[2] as i32;
                let mut capture = |model: &mut crate::gpumodel::GpuModel,
                                   draw_matrix: &Matrix,
                                   visible: bool| {
                    if !visible {
                        return;
                    }
                    let bounds = crate::player_picking::bounds(model);
                    let capsule = crate::scene_player_pick::screen_bounds(
                        bounds,
                        model.horizontal_radius(),
                        crate::camera::multiply(&draw_matrix.entries(), &pick_frame.vp),
                        pick_frame.projection,
                        pick_frame.screen,
                    );
                    pick_frame
                        .picks
                        .push(crate::scene_player_pick::PickablePlayer {
                            id: crate::scene_player_pick::PlayerPickId {
                                pid: player as i32,
                                generation: game.runtime.feed.state.players.generations[player],
                            },
                            bounds: crate::scene_player_pick::pick_bounds(raw, 0),
                            screen_bounds: Some(capsule),
                            projected_depth: depth,
                            matrix: crate::camera::multiply(&pick_matrix.entries(), &pick_frame.vp),
                            active: true,
                        });
                };
                if let (Some(model), Some(draw)) = (shadow.as_mut(), entry.shadow.as_ref()) {
                    capture(model, &draw.matrix, draw.visible);
                }
                capture(&mut body.model, &entry.matrix, entry.visible);
                scene.temporary[index].model = Some(body.model);
            }
        }
        let visible = |id: &usize| {
            if let EntityRef::Temporary(index) = live.entities[*id].source {
                if scene.temporary[index].transient {
                    return crate::dynamic_scene::model(scene, live.entities[*id].source).is_some();
                }
                return scene.temporary[index].model.is_some()
                    && self
                        .meshes
                        .get(&scene.temporary[index].player)
                        .is_some_and(|e| {
                            e.visible
                                || e.shadow.as_ref().is_some_and(|s| s.visible)
                                || e.hint_arrows.iter().any(|a| a.visible)
                        });
            }
            crate::dynamic_scene::model(scene, live.entities[*id].source).is_some()
                && crate::draw::entity_model_visible(
                    &live.entities[*id],
                    &live.draw.plan.model_planes,
                )
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

    /// Picking rebuilds the animated body with flag131072 using the
    /// last draw's projection and capsule, and the current unshifted transform.
    pub fn refresh_picks(
        &mut self,
        game: &mut Game,
        materials: &MaterialStore,
        frame: &mut crate::player_picking::Frame,
        mouse: [i32; 2],
        npc_store: Option<&crate::config::NpcStore>,
    ) -> Result<()> {
        let defaults = &game.inputs.appearance.defaults;
        let varps = game.runtime.feed.state.varps.as_ref();
        let bits = &game.inputs.bits;
        // The body model resolves the multi-NPC through the local player's game state.
        let vars = |bit: bool, id: i32| -> Option<i32> {
            if bit {
                let definition = bits.get(id, false).ok()?;
                varps?.get_bit(&definition).ok()
            } else {
                varps?.get(id).ok()
            }
        };
        let r = Resources {
            pack: &self.pack,
            types: Inputs {
                items: &game.inputs.appearance.types.items,
                bases: &game.inputs.bas,
                wear: &defaults.wear,
                recolour: defaults
                    .graphics
                    .recolour
                    .as_ref()
                    .context("player colour palette")?,
                retexture: defaults
                    .graphics
                    .retexture
                    .as_ref()
                    .context("player texture palette")?,
            },
            materials,
            billboards: &self.billboards,
            emitters: &self.emitters,
            npcs: npc_store.map(|store| crate::player_body::NpcTypes { store, vars: &vars }),
        };
        let mut current = BTreeMap::new();
        for pick in &mut frame.picks {
            pick.active = false;
            if !pick
                .screen_bounds
                .is_some_and(|b| crate::scene_player_pick::capsule_hit(b, mouse, [0, 0]))
            {
                continue;
            }
            let id = pick.id.pid as usize;
            if game.runtime.feed.state.players.generations.get(id) != Some(&pick.id.generation) {
                continue;
            }
            let Some(e) = game
                .runtime
                .feed
                .state
                .players
                .players
                .get_mut(id)
                .and_then(Option::as_mut)
            else {
                continue;
            };
            if e.appearance.visibility == Some(1) {
                continue;
            }
            if let std::collections::btree_map::Entry::Vacant(slot) = current.entry(id) {
                let use_idle = e.actor.scene.use_idle;
                let value = player_body::build(
                    &mut self.models,
                    &mut self.assets,
                    &r,
                    e,
                    game.runtime.terrain.as_ref(),
                    player_body::BodyRequest {
                        cycle: game.cycle,
                        flags: 131072,
                        use_idle,
                    },
                )?
                .map(|mut body| {
                    crate::scene_player_pick::pick_bounds(
                        crate::player_picking::bounds(&mut body.model),
                        0,
                    )
                });
                slot.insert(value);
            }
            if let Some(bounds) = current[&id] {
                pick.bounds = bounds;
                let matrix = Matrix::actor(e.actor.rotation, [e.fine_x, e.motion.y, e.fine_z], 0.);
                pick.matrix = crate::camera::multiply(&matrix.entries(), &frame.vp);
                pick.active = true;
            }
        }
        Ok(())
    }
}

/// The hint-arrow targets, from the last hint slot down: the fine offset
/// from the local player, the arrow model and the squared range
/// (`92160000` for NPC/player targets, `(height << 9)^2` for tiles).
fn hint_targets(
    game: &Game,
    local: usize,
    arrows: &[Option<crate::minimap::HintArrow>],
) -> Vec<([i64; 2], i32, i64)> {
    let state = &game.runtime.feed.state;
    let Some(me) = state.players.players.get(local).and_then(Option::as_ref) else {
        return Vec::new();
    };
    let offset = |x: f32, z: f32| [(x - me.fine_x) as i32 as i64, (z - me.fine_z) as i32 as i64];
    let mut out = Vec::new();
    for arrow in arrows.iter().rev().flatten() {
        if arrow.model == -1 {
            continue;
        }
        match arrow.hint_type {
            1 => {
                if let Some(npc) = arrow.npc_index.and_then(|i| state.npcs.entities.get(&i)) {
                    out.push((
                        offset(npc.path.fine_x, npc.path.fine_z),
                        arrow.model,
                        92_160_000,
                    ));
                }
            }
            2 => {
                if let Some([x, z]) = arrow.fine {
                    let range = i64::from(arrow.distance_tiles << 9);
                    out.push((
                        [
                            i64::from(x - me.fine_x as i32),
                            i64::from(z - me.fine_z as i32),
                        ],
                        arrow.model,
                        range * range,
                    ));
                }
            }
            10 => {
                if let Some(p) = arrow
                    .player_index
                    .and_then(|i| state.players.players.get(i))
                    .and_then(Option::as_ref)
                {
                    out.push((offset(p.fine_x, p.fine_z), arrow.model, 92_160_000));
                }
            }
            _ => {}
        }
    }
    out
}
