//! `ViewerApp::entities` (code-quality programme Phase 4.4).
use super::*;
use rs910_core::soft_cache::SoftMap;

/// See [`EntityRenderer::loc_models`].
type LocModelKey = (u32, i32, i32, i32, Option<i64>, Option<[i32; 3]>, i32);

mod cache;
use cache::{
    GroundStacks, NpcDefinition, NpcDefinitionRevision, NpcDefinitions, TransientVisibility,
};

/// A dynamic loc the server added: the loc, shape and angle its animation
/// belongs to, and the animation as the dynamic scene plays a placed one.
#[derive(Default)]
pub(super) struct AddedLocAnimation {
    placed: Option<(u32, i32, i32)>,
    playback: crate::dynamic_loc::DynamicLoc,
}

/// The scene entities' draw state: the per-frame model caches (effect,
/// hint, ground, loc and NPC type models, obj stack
/// radii), the loc store they read, the players renderer
/// (player and NPC bodies and picking) and the animation,
/// billboard and emitter stores.
pub(super) struct EntityRenderer {
    pub(super) effect_models: SoftMap<i32, crate::gpumodel::GpuModel>,
    pub(super) hint_models: SoftMap<i32, crate::gpumodel::GpuModel>,
    pub(super) ground_models: SoftMap<(u32, i32), crate::gpumodel::GpuModel>,
    /// The largest model radius of each stack's previous draw, per stack
    /// `(level, local x, local z)`, which sizes the next draw's ground tilt.
    pub(super) obj_stack_radius: HashMap<(i32, i32, i32), i32>,
    pub(super) ground_stacks: GroundStacks,
    pub(super) npc_definitions: NpcDefinitions,
    /// The models of locs the server added: `(loc, shape, angle, detail,
    /// customisation salt, footprint centre of a loc that follows the
    /// terrain, animation flags)`. A static loc's finished model has no
    /// animation flags; an animated loc's base model has them and no centre.
    pub(super) loc_models: SoftMap<LocModelKey, crate::gpumodel::GpuModel>,
    /// The animation of each dynamic loc the server added, by its zone key
    /// `(level, layer, x, z)`, and the random source of their picks.
    pub(super) added_loc_animations: HashMap<(i32, i32, i32, i32), AddedLocAnimation>,
    pub(super) added_loc_random: crate::animation_playback::AnimationRandom,
    pub(super) loc_store: Option<crate::config::LocStore>,
    /// The NPC model cache for scene NPCs: `(type, detail,
    /// body customisation salt)`.
    /// The cached bodies and shadows of scene NPCs.
    pub(super) npcs: crate::npc_draw::Cache,
    /// What picking needs of each drawn NPC, by NPC slot, this frame.
    pub(super) npc_picks:
        HashMap<usize, (crate::npc_draw::Placement, crate::npc_draw::PickOptions)>,
    pub(super) players: Option<crate::player_renderer::PlayersRenderer>,
    pub(super) animation_assets: Option<crate::animation_assets::AnimationAssets>,
    pub(super) billboards: Option<crate::billboard::BillboardStore>,
    pub(super) emitters: Option<crate::particle::EmitterStore>,
    /// The last ordinary frame's cover marker pairing, which the cutscene
    /// frames draw again.
    pub(super) cover_marker_memory: crate::cover_marker::Memory,
}

pub(super) fn obj_stack_next(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    *seed
}

pub(super) fn obj_stack_seed(level: i32, world_x: i32, world_z: i32, item: u32) -> u32 {
    let mut seed = (level as u32).wrapping_mul(0x9E37_79B9)
        ^ (world_x as u32).rotate_left(11)
        ^ (world_z as u32).rotate_left(23)
        ^ item.rotate_left(7);
    obj_stack_next(&mut seed);
    seed
}

/// An actor spot model and the actor's draw matrix it is drawn with.
pub(super) struct ActorSpot {
    /// The actor transform rotation (`ActorState::rotation`).
    pub(super) rotation: [f32; 4],
    /// `-5` minus the decoration offset of the actor's tile.
    pub(super) y_offset: i32,
    /// The ground skew inputs (effect hillskew mode 3).
    pub(super) ground: [i32; 3],
    pub(super) height: i32,
    /// The BAS slot transform plus slot offset for the delay, and the slot's
    /// angle (else the actor's angle).
    pub(super) slot: Option<([i32; 3], i32)>,
}

impl ActorSpot {
    pub(super) fn new(
        actor: &crate::entities910::Player,
        spot: &crate::entities910::animation_state::Spot,
        bas: Option<&crate::protocol910::bas_types::Bas>,
    ) -> Self {
        let slot = usize::try_from(spot.delay).ok().and_then(|slot| {
            let bas = bas?;
            let row = |t: &Option<Vec<Option<Vec<i32>>>>| {
                t.as_ref()
                    .and_then(|t| t.get(slot))
                    .and_then(Option::as_ref)
                    .cloned()
            };
            let transform = row(&bas.slot_transforms)?;
            let mut offset = [0; 3];
            for t in std::iter::once(transform).chain(row(&bas.slot_offsets)) {
                for (o, v) in offset.iter_mut().zip(t) {
                    *o += v;
                }
            }
            let angle = actor
                .actor
                .wear_angles
                .as_ref()
                .and_then(|a| a.get(slot))
                .copied()
                .filter(|&a| a != -1)
                .unwrap_or(actor.angle);
            Some((offset, angle))
        });
        Self {
            rotation: actor.actor.rotation,
            y_offset: (-5i32).wrapping_sub(actor.actor.scene.decoration_offset),
            ground: actor.actor.scene.ground,
            height: spot.height,
            slot,
        }
    }

    /// The actor stands on ground of its own tilt and is drawn with its own
    /// lift, unlike a player's fixed five units.
    pub(super) fn stood_on(mut self, ground: [i32; 3], y_offset: i32) -> Self {
        self.ground = ground;
        self.y_offset = y_offset;
        self
    }

    /// Places the spot animation on the effect model (actor-local space).
    pub(super) fn place(
        &self,
        model: &mut crate::gpumodel::GpuModel,
        effect: &crate::protocol910::effect_types::Effect,
        orientation: i32,
        actor_angle: i32,
    ) {
        match self.slot {
            None => {
                // The effect model at `orientation * 2048` and the tilt:
                // the yaw (plus the type's own orientation), then
                // hillskew mode 3 follows the actor's tilt.
                crate::loctype::effect_yaw(model, effect, orientation.wrapping_mul(2048));
                if effect.hillskew_mode == 3 {
                    let [pitch, roll, y] = self.ground;
                    if pitch != 0 {
                        model.rotate_x(pitch);
                    }
                    if roll != 0 {
                        model.rotate_z(roll);
                    }
                    if y != 0 {
                        model.translate(0, y, 0);
                    }
                }
            }
            Some(([mut x, y, mut z], slot_angle)) => {
                // The effect model takes no yaw here, so only
                // the type's own orientation turns the copy.
                crate::loctype::effect_yaw(model, effect, 0);
                // only a horizontal offset turns the model.
                if z != 0 || x != 0 {
                    let turn = orientation
                        .wrapping_mul(2048)
                        .wrapping_add(slot_angle)
                        .wrapping_sub(actor_angle)
                        & 0x3FFF;
                    if turn != 0 {
                        model.rotate_y_keep_normals(turn);
                    }
                    let (sin, cos) = (crate::trig::sin(turn), crate::trig::cos(turn));
                    let rx = (x * cos + z * sin) >> 14;
                    z = (z * cos - x * sin) >> 14;
                    x = rx;
                }
                model.translate(x, y, z);
            }
        }
        if self.height != 0 {
            model.translate(0, -self.height << 2, 0);
        }
    }

    /// The actor draw matrix at `position`.
    pub(super) fn matrix(&self, position: [f32; 3]) -> crate::actor_matrix::Matrix {
        crate::actor_matrix::Matrix::actor(self.rotation, position, self.y_offset as f32)
    }

    /// Bake the draw matrix's rotation and height offset into the model; the
    /// temporary entity supplies the translation.
    pub(super) fn apply_matrix(&self, model: &mut crate::gpumodel::GpuModel) {
        let [x, y, z, w] = self.rotation;
        model.apply_srt([x, y, z, w, 0., self.y_offset as f32, 0., 1., 1., 1.]);
    }
}

impl ViewerApp {
    /// Materialize retained spot/projectile effects as ordinary temporary
    /// scene entities. The game tick owns node progression; this scene owner
    /// prepares the current pose before uploading the temporary mesh.
    pub(super) fn add_transient_entities(&mut self) -> anyhow::Result<()> {
        let mut visibility = if self.faithful_scene_required() {
            None
        } else {
            self.renderer
                .as_ref()
                .zip(self.scene.graph.as_ref())
                .zip(self.scene.live.as_ref())
                .and_then(|((renderer, scene), live)| {
                    let mut camera = self.view.camera.scene_camera(renderer.scene_size());
                    if camera.legacy.is_some() {
                        return None;
                    }
                    camera.target[0] -= self.scene.floor_base.0 * scene.tile_size;
                    camera.target[2] -= self.scene.floor_base.1 * scene.tile_size;
                    let mut projection = camera.projection();
                    crate::camera::glx_flip_y(&mut projection);
                    let eye = camera.eye();
                    Some(TransientVisibility {
                        cpu: crate::camera::CpuProjection::new(
                            camera.view_entries(),
                            projection,
                            [0, 0, camera.viewport.0, camera.viewport.1],
                        ),
                        eye: [eye[0] >> scene.size, eye[2] >> scene.size],
                        distance: live.draw.distance,
                        size_shift: scene.size,
                        tile_size: scene.tile_size,
                        columns: HashMap::new(),
                    })
                })
        };
        self.particles.npc_bindings.clear();
        self.particles.effect_bindings.clear();
        self.particles.temporary.clear();
        // (position in `entities`, owner key, draws the owner's particles).
        let mut particle_slots: Vec<(usize, u64, bool)> = Vec::new();
        // The animation-detail gate of the static/dynamic loc split
        // (`locs.rs`).
        let anim_detail = self.core.session.game().map_or(1, |game| {
            crate::rebuild::BuildPrefs::from_options(&game.ui_variables.queries.preferences.options)
                .anim_detail
        });
        struct Spec {
            projectile: bool,
            index: usize,
            npc_spot: Option<(usize, usize)>,
            player_spot: Option<(usize, usize)>,
            effect_id: i32,
            effect: crate::protocol910::effect_types::Effect,
            node: Option<crate::entities910::animation_state::Node>,
            level: i32,
            occlude_level: i32,
            position: [f32; 3],
            orientation: i32,
            actor_angle: i32,
            pitch: f64,
            yaw: f64,
            /// Scene entries (`player_scene::cutscene_pushes`): an actor spot
            /// is drawn by each of its actor's entries.
            pushes: usize,
            /// An actor spot model's placement and the actor's
            /// draw matrix.
            actor: Option<ActorSpot>,
            /// The colour shift of the NPC an effect model belongs to.
            antimacro: Option<[i32; 4]>,
        }
        struct NpcSpec {
            index: usize,
            /// The NPC type after the multinpc selection.
            type_id: u32,
            /// The NPC's BAS id.
            bas: i32,
            /// The NPC's actor state, for the tilt and tint of its body.
            path: crate::entities910::Player,
            /// The fade-in of a new arrival and its type's duration.
            fade: crate::npc_draw::Fade,
            fade_in: i32,
            /// The ground decoration offset after this draw's smoothing.
            decoration_offset: i32,
            look: crate::npc_draw::Look,
            /// The animation nodes of the ground shadow, chosen as the
            /// shadow's own rule does.
            shadow_node: Option<crate::entities910::animation_state::Node>,
            level: i32,
            occlude_level: i32,
            fine_x: f32,
            fine_z: f32,
            height: f32,
            angle: i32,
            node: crate::entities910::animation_state::Node,
            /// Scene entries (`player_scene::cutscene_pushes`).
            pushes: usize,
            /// The walk node when it has a
            /// sequence and is not an idle stance under an active main.
            walk: Option<crate::entities910::animation_state::Node>,
            overlays: Vec<Option<crate::entities910::animation_state::Node>>,
            wear_angles: Option<Vec<i32>>,
            bas_type: Option<std::rc::Rc<crate::protocol910::bas_types::Bas>>,
            /// The NPC body customisation (NPC_INFO mask 0x400).
            body: Option<crate::entities910::npc_custom::Custom>,
        }
        let hint_trails = self
            .core
            .session
            .ui()
            .map(|ui| ui.engine.scene.hint_trails.clone())
            .unwrap_or_else(|| std::array::from_fn(|_| None));
        let (hint_base_x, hint_base_z, hint_level) = self
            .core
            .session
            .game()
            .map(|game| {
                (
                    game.runtime.map.base_x,
                    game.runtime.map.base_z,
                    game.runtime.feed.state.players.current_level,
                )
            })
            .unwrap_or((0, 0, 0));
        // Ground-object packets already maintain the cost/count ordering in
        // the retained zone owner. Snapshot the visible stacks here so model
        // creation and scene insertion stay outside packet decoding.
        let ground_store = self.core.session.as_ref().and_then(|session| {
            let game = session.game.as_ref()?;
            let objects = &game.runtime.feed.state.zones.objects;
            let map = &game.runtime.map;
            let revision = (
                objects.revision,
                game.runtime.terrain_generation,
                map.base_x,
                map.base_z,
                map.width,
                map.height,
            );
            let cache = &mut self.entities.ground_stacks;
            if cache.revision != Some(revision) {
                cache.rows.clear();
                for (&key, stack) in &objects.stacks {
                    let level = ((key >> 28) & 3) as i32;
                    let world_x = (key & 0x3fff) as i32;
                    let world_z = ((key >> 14) & 0x3fff) as i32;
                    let local_x = world_x - map.base_x;
                    let local_z = world_z - map.base_z;
                    if !(0..4).contains(&level)
                        || local_x < 0
                        || local_z < 0
                        || local_x >= map.width
                        || local_z >= map.height
                    {
                        continue;
                    }
                    let objects: Vec<_> = stack
                        .iter()
                        .map(|object| (object.id, object.count))
                        .collect();
                    if let Some(ranks) = crate::obj_stack::select(&objects) {
                        cache
                            .rows
                            .push((level, world_x, world_z, local_x, local_z, ranks));
                    }
                }
                cache.revision = Some(revision);
            }
            session.ui.state.objs.clone()
        });
        let ground_specs = self.entities.ground_stacks.rows.clone();
        let location_specs: Vec<crate::protocol910::zone_state::Location> = self
            .core
            .session
            .as_ref()
            .and_then(|session| {
                let game = session.game.as_ref()?;
                let zones = &game.runtime.feed.state.zones;
                let mut by_key = std::collections::BTreeMap::new();
                for request in &zones.locations {
                    by_key.insert(
                        (request.level, request.layer, request.x, request.z),
                        request.clone(),
                    );
                }
                for request in &zones.customisations {
                    let key = (request.level, request.layer, request.x, request.z);
                    if let Some(existing) = by_key.get_mut(&key) {
                        existing.id = request.id;
                        existing.shape = request.shape;
                        existing.angle = request.angle;
                        existing.custom = request.custom.clone();
                        existing.pending = request.pending;
                        existing.remove = request.remove;
                    } else {
                        by_key.insert(key, request.clone());
                    }
                }
                Some(by_key.into_values().collect())
            })
            .unwrap_or_default();
        let npc_types = self
            .core
            .session
            .ui()
            .and_then(|ui| ui.engine.configs.npcs.clone());
        if self
            .entities
            .npc_definitions
            .store
            .as_ref()
            .zip(npc_types.as_ref())
            .is_none_or(|(old, new)| !std::rc::Rc::ptr_eq(old, new))
        {
            self.entities.npc_definitions.rows.clear();
            self.entities.npc_definitions.store = npc_types.clone();
        }
        // Entity placement: NPCs (and cutscene NPCs while
        // sceneState == 0) stand on the heightmap unless force-moving, the
        // same rule `player_scene::append` applies to players.
        if let Some(game) = self.core.session.game_mut() {
            let cycle = game.game.cycle;
            let terrain = game.game.runtime.terrain.as_ref();
            let npcs = &mut game.game.runtime.feed.state.npcs;
            for index in npcs.slots.clone() {
                if let Some(npc) = npcs.entities.get_mut(&index) {
                    let e = &mut npc.path;
                    if (e.forced[6] <= cycle && e.forced[7] < cycle) || e.forced[5] == e.forced[4] {
                        if let Ok(y) = crate::protocol910::terrain::height(
                            terrain,
                            e.fine_x as i32,
                            e.fine_z as i32,
                            e.level,
                        ) {
                            e.motion.y = y as f32;
                        }
                    }
                }
            }
        }
        let scene_graph = self.scene.graph.as_ref();
        // The scene viewport variant without projectiles never draws them.
        let projectiles_shown = !self
            .core
            .session
            .ui()
            .is_some_and(|ui| ui.state.scene_without_projectiles);
        let (specs, npc_specs, detail) = self
            .core
            .session
            .game()
            .map(|game| {
                let mut specs = Vec::new();
                let mut npc_specs = Vec::new();
                let cutscene = game.cutscene.scene_state == 0;
                for (index, projectile) in game
                    .runtime
                    .feed
                    .state
                    .zones
                    .transients
                    .projectiles
                    .iter()
                    .enumerate()
                    .filter(|_| projectiles_shown)
                {
                    if let Some(effect) = game.inputs.animation.effects.get(&projectile.effect) {
                        specs.push(Spec {
                            projectile: true,
                            index,
                            npc_spot: None,
                            player_spot: None,
                            effect_id: projectile.effect,
                            effect: effect.clone(),
                            node: Some(projectile.animation.clone()),
                            level: projectile.level,
                            occlude_level: projectile.level,
                            position: projectile.position,
                            orientation: 0,
                            actor_angle: 0,
                            pitch: projectile.vy.atan2(projectile.speed),
                            yaw: projectile.vx.atan2(projectile.vz) - std::f64::consts::PI,
                            pushes: 1,
                            actor: None,
                            antimacro: None,
                        });
                    }
                }
                for (index, spot) in game
                    .runtime
                    .feed
                    .state
                    .zones
                    .transients
                    .spots
                    .iter()
                    .enumerate()
                {
                    if let Some(effect) = game.inputs.animation.effects.get(&spot.effect) {
                        specs.push(Spec {
                            projectile: false,
                            index,
                            npc_spot: None,
                            player_spot: None,
                            effect_id: spot.effect,
                            effect: effect.clone(),
                            node: spot.animation.clone(),
                            level: spot.level,
                            occlude_level: spot.occlude,
                            position: spot.position,
                            orientation: spot.orientation,
                            actor_angle: 0,
                            pitch: 0.,
                            yaw: 0.,
                            pushes: 1,
                            actor: None,
                            antimacro: None,
                        });
                    }
                }
                for (index, player) in game.runtime.feed.state.players.players.iter().enumerate() {
                    let Some(player) = player.as_ref() else {
                        continue;
                    };
                    for (slot, spot) in player.animation.spots.iter().enumerate() {
                        if spot.id < 0 {
                            continue;
                        }
                        let Some(effect) = game.inputs.animation.effects.get(&spot.id) else {
                            continue;
                        };
                        specs.push(Spec {
                            projectile: false,
                            index: 0,
                            npc_spot: None,
                            player_spot: Some((index, slot)),
                            effect_id: spot.id,
                            effect: effect.clone(),
                            node: Some(spot.node.clone()),
                            level: player.level,
                            occlude_level: player.occlude_level,
                            position: [player.fine_x, player.motion.y, player.fine_z],
                            orientation: spot.orientation,
                            actor_angle: player.angle,
                            pitch: 0.,
                            yaw: 0.,
                            pushes: if cutscene {
                                crate::player_scene::cutscene_pushes(
                                    player.size,
                                    player.fine_x,
                                    player.fine_z,
                                )
                            } else {
                                1
                            },
                            actor: Some(ActorSpot::new(
                                player,
                                spot,
                                game.inputs.bas.get(&player.appearance.bas),
                            )),
                            antimacro: None,
                        });
                    }
                }
                for &index in &game.runtime.feed.state.npcs.slots {
                    let Some(npc) = game.runtime.feed.state.npcs.entities.get(&index) else {
                        continue;
                    };
                    let Ok(type_id) = u32::try_from(npc.type_id) else {
                        continue;
                    };
                    // Entity placement outside a cutscene: a tile-centred NPC is
                    // added unless its `drawPriority < 0` or it is deferred
                    // (another actor owns the shared tile, `npc_scene_flags`),
                    // and the moving ones are added with a non-negative
                    // priority. A deferred NPC draws nothing.
                    if !cutscene && !crate::entity_elements::npc_added_to_scene(&npc.path) {
                        continue;
                    }
                    // The multinpc selection over the local player's game state; a
                    // null selection draws nothing.
                    let read = |bit: bool, id: i32| -> Option<i32> {
                        let varps = game.runtime.feed.state.varps.as_ref()?;
                        if bit {
                            varps.get_bit(&game.inputs.bits.get(id, false).ok()?).ok()
                        } else {
                            varps.get(id).ok()
                        }
                    };
                    let (type_id, bas) = match npc_types.as_deref() {
                        Some(store) => {
                            let Ok(base) = crate::npc_type_model::list(store, type_id) else {
                                continue;
                            };
                            let bas = if npc.bas_override != -1 {
                                npc.bas_override
                            } else if base.multinpc.is_empty() {
                                base.bas
                            } else {
                                base.multi_npc(&read)
                                    .and_then(|id| crate::npc_type_model::list(store, id).ok())
                                    .map(|t| t.bas)
                                    .filter(|&bas| bas != -1)
                                    .unwrap_or(base.bas)
                            };
                            match crate::npc_type_model::resolve(store, base, &read) {
                                Ok(Some(resolved)) => (resolved.id, bas),
                                Ok(None) => continue,
                                Err(error) => {
                                    crate::logging::warn_repeated!(
                                        "[client910] NPC {type_id} multinpc: {error:#}"
                                    );
                                    continue;
                                }
                            }
                        }
                        None => (type_id, npc.bas_override),
                    };
                    let base_config = npc_types.as_deref().map(|store| {
                        let base = crate::npc_type_model::list(store, npc.type_id as u32).ok();
                        let resolved = crate::npc_type_model::list(store, type_id).ok();
                        (base, resolved)
                    });
                    let bas_type = game.inputs.bas.get(&bas);
                    let options = &game.ui_variables.queries.preferences.options;
                    let graphics = &game.inputs.appearance.defaults.graphics.scalars;
                    let revision = NpcDefinitionRevision {
                        base: npc.type_id as u32,
                        resolved: type_id,
                        bas,
                        seeds: npc.recolours,
                        shadows: options.get("characterShadows") == Some(1),
                        textures: options.get("textures") == Some(1),
                        shadow_texture: (
                            graphics.spotshadowtexture,
                            graphics.spotshadowtexture_alpha,
                        ),
                    };
                    let definition =
                        self.entities
                            .npc_definitions
                            .definition(revision, || NpcDefinition {
                                look: crate::npc_draw::Look::new(
                                    base_config.as_ref().and_then(|(base, resolved)| {
                                        Some((base.as_deref()?, resolved.as_deref()?))
                                    }),
                                    bas_type,
                                    npc.recolours,
                                    &crate::npc_draw::LookInputs {
                                        shadows: revision.shadows,
                                        textures: revision.textures,
                                        default_shadow_texture: revision.shadow_texture,
                                    },
                                ),
                                fade_in: base_config
                                    .as_ref()
                                    .and_then(|(_, resolved)| resolved.as_deref())
                                    .map_or(0, |resolved| resolved.fade_in),
                                bas: bas_type.cloned().map(std::rc::Rc::new),
                            });
                    let look = definition.look;
                    let fade_in = definition.fade_in;
                    let decor = scene_graph
                        .and_then(|scene| {
                            let tile = scene.tile(
                                npc.path.level as usize,
                                (npc.path.fine_x as usize) >> 9,
                                (npc.path.fine_z as usize) >> 9,
                            )?;
                            scene.ground_decors.get(tile.ground_decoration?)
                        })
                        .map(|decor| decor.decor_height);
                    let decoration_offset = crate::npc_draw::settle_decoration_offset(
                        npc.path.actor.scene.decoration_offset,
                        decor,
                    );
                    let default_stance = crate::protocol910::bas_types::Bas::default();
                    let stance = bas_type.unwrap_or(&default_stance);
                    let ground = crate::player_body::stance_ground(
                        &npc.path,
                        stance,
                        game.runtime.terrain.as_ref(),
                    )
                    .unwrap_or([0; 3]);
                    let shadow_node = {
                        let main = &npc.path.animation.main;
                        let main_delayed = main.sequence.is_some() && main.delay != 0;
                        let walk = &npc.path.actor.walk;
                        if walk.node.sequence.is_some() && (!walk.idle || !main_delayed) {
                            Some(walk.node.clone())
                        } else if main_delayed {
                            Some(main.clone())
                        } else {
                            None
                        }
                    };
                    // Entity placement while sceneState == 0.
                    let pushes = if cutscene {
                        crate::player_scene::cutscene_pushes(
                            npc.path.size,
                            npc.path.fine_x,
                            npc.path.fine_z,
                        )
                    } else {
                        1
                    };
                    npc_specs.push(NpcSpec {
                        index,
                        type_id,
                        bas,
                        path: npc.path.clone(),
                        fade: crate::npc_draw::Fade {
                            alpha: npc.fade_alpha,
                            start: npc.fade_start,
                        },
                        fade_in,
                        decoration_offset,
                        look: look.clone(),
                        shadow_node,
                        level: npc.path.level,
                        occlude_level: npc.path.occlude_level,
                        fine_x: npc.path.fine_x,
                        fine_z: npc.path.fine_z,
                        height: npc.path.motion.y,
                        angle: npc.path.angle,
                        node: npc.path.animation.main.clone(),
                        pushes,
                        walk: {
                            let main_active = npc.path.animation.main.sequence.is_some()
                                && npc.path.animation.main.delay == 0;
                            let w = &npc.path.actor.walk;
                            (w.node.sequence.is_some() && !(w.idle && main_active))
                                .then(|| w.node.clone())
                        },
                        overlays: npc.path.animation.overlays.clone(),
                        wear_angles: npc.path.actor.wear_angles.clone(),
                        bas_type: definition.bas,
                        body: npc.body.clone(),
                    });
                    for (slot, spot) in npc.path.animation.spots.iter().enumerate() {
                        if spot.id < 0 {
                            continue;
                        }
                        let Some(effect) = game.inputs.animation.effects.get(&spot.id) else {
                            continue;
                        };
                        specs.push(Spec {
                            projectile: false,
                            index: 0,
                            npc_spot: Some((index, slot)),
                            player_spot: None,
                            effect_id: spot.id,
                            effect: effect.clone(),
                            node: Some(spot.node.clone()),
                            level: npc.path.level,
                            occlude_level: npc.path.occlude_level,
                            position: [npc.path.fine_x, npc.path.motion.y, npc.path.fine_z],
                            orientation: spot.orientation,
                            actor_angle: npc.path.angle,
                            pitch: 0.,
                            yaw: 0.,
                            pushes,
                            actor: Some(
                                ActorSpot::new(&npc.path, spot, game.inputs.bas.get(&bas))
                                    .stood_on(ground, crate::npc_draw::lift(decoration_offset)),
                            ),
                            antimacro: look.antimacro,
                        });
                    }
                }
                let detail = crate::rebuild::BuildPrefs::from_options(
                    &game.ui_variables.queries.preferences.options,
                )
                .model_detail();
                (specs, npc_specs, detail)
            })
            .unwrap_or_default();
        if specs.is_empty()
            && npc_specs.is_empty()
            && ground_specs.is_empty()
            && location_specs.is_empty()
            && hint_trails.iter().all(Option::is_none)
        {
            return Ok(());
        }
        if self.scene.pack_root.is_none() {
            return Ok(());
        }
        let Some(materials) = self.scene.material_store.as_ref() else {
            return Ok(());
        };
        let pack = self.pack.clone();
        if self.entities.billboards.is_none() {
            self.entities.billboards = Some(crate::billboard::BillboardStore::load(&pack)?);
        }
        if self.entities.emitters.is_none() {
            self.entities.emitters = Some(crate::particle::EmitterStore::load(&pack)?);
        }
        if self.entities.animation_assets.is_none() {
            self.entities.animation_assets =
                Some(crate::animation_assets::AnimationAssets::load(&pack)?);
        }
        let billboards = self.entities.billboards.as_ref().unwrap();
        let emitters = self.entities.emitters.as_ref().unwrap();
        let animations = self.entities.animation_assets.as_mut().unwrap();
        let mut entities = Vec::new();
        if !location_specs.is_empty() && self.entities.loc_store.is_none() {
            match crate::config::LocStore::load(&pack) {
                Ok(store) => self.entities.loc_store = Some(store),
                Err(error) => crate::logging::warn_repeated!(
                    "[client910] loc change configs unavailable: {error:#}"
                ),
            }
        }
        let scene_tiles = self
            .scene
            .graph
            .as_ref()
            .map_or([0, 0], |scene| [scene.max_x as i32, scene.max_z as i32]);
        // An added loc's animation ends with its request.
        self.entities.added_loc_animations.retain(|location, _| {
            location_specs.iter().any(|request| {
                !request.remove && (request.level, request.layer, request.x, request.z) == *location
            })
        });
        if let Some(store) = self.entities.loc_store.as_ref() {
            for request in location_specs.iter().filter(|request| !request.remove) {
                let Ok(loc_id) = u32::try_from(request.id) else {
                    continue;
                };
                let Some(loc) = store.get(loc_id) else {
                    continue;
                };
                let level = request.level.clamp(0, 3) as usize;
                let Some(floor) = self.scene.floors.get(level).and_then(Option::as_ref) else {
                    continue;
                };
                // A request is applied inside the scene's border tiles only.
                if request.x < 1
                    || request.z < 1
                    || request.x > scene_tiles[0] - 2
                    || request.z > scene_tiles[1] - 2
                {
                    continue;
                }
                // It stands where the map would place it: on the centre of
                // its footprint.
                let footprint = crate::locs::Footprint::new(
                    loc,
                    [request.shape, request.angle],
                    [request.x, request.z],
                    &floor.heights,
                    scene_tiles,
                );
                let [x, y, z] = footprint.centre;
                let above = self
                    .scene
                    .floors
                    .get(level + 1)
                    .and_then(Option::as_ref)
                    .map(|above| &above.heights);
                // A loc-change request adds a loc: one that is not static
                // becomes a dynamic loc, which plays its type's animations
                // and whose draw binds its model's particles and draws them
                // (scenery and the wall/decor forms).
                let is_static = (!loc.has_anim
                    || (loc.disable_anim_low_detail && anim_detail == 0))
                    && !loc.has_multiloc
                    && !loc.force_dynamic
                    && !loc.always_dynamic;
                let location = (request.level, request.layer, request.x, request.z);
                let pose = if is_static {
                    None
                } else {
                    let placed = (loc_id, request.shape, request.angle);
                    let state = self
                        .entities
                        .added_loc_animations
                        .entry(location)
                        .or_default();
                    if state.placed != Some(placed) {
                        *state = AddedLocAnimation {
                            placed: Some(placed),
                            ..Default::default()
                        };
                    }
                    let context = crate::dynamic_loc::LocContext {
                        base: loc,
                        resolved: Some(loc),
                        assets: animations,
                        cycle: self.core.cycle,
                        detail: anim_detail,
                    };
                    state
                        .playback
                        .begin(&context, 0, &mut self.entities.added_loc_random)?;
                    let playback = &mut state.playback.animation;
                    if playback.node.id() == -1 {
                        None
                    } else {
                        Some(animations.prepare(&pack, playback, request.angle & 3)?)
                    }
                };
                let custom_salt = request.custom.as_ref().map(|custom| custom.salt);
                let posed = if let Some(pose) = &pose {
                    // The animated base model (as the dynamic scene builds
                    // it), posed, then turned, fitted to the terrain and
                    // offset like a placed loc's.
                    let diagonal = request.shape == crate::loctype::shape::CENTREPIECE_DIAGONAL;
                    let angle = request.angle + if diagonal { 4 } else { 0 };
                    let shape = if diagonal {
                        crate::loctype::shape::CENTREPIECE_STRAIGHT
                    } else if crate::loctype::shape::is_wall_decor(request.shape) {
                        crate::loctype::shape::WALLDECOR_STRAIGHT_NOOFFSET
                    } else {
                        request.shape
                    };
                    let mut flags = 0x1F01F | pose.flags;
                    if loc.hillchange == 3 {
                        flags |= 0x7;
                    } else {
                        if loc.hillchange != 0 || loc.post_yoff != 0 {
                            flags |= 0x2;
                        }
                        if loc.post_xoff != 0 {
                            flags |= 0x1;
                        }
                        if loc.post_zoff != 0 {
                            flags |= 0x4;
                        }
                    }
                    if shape == crate::loctype::shape::CENTREPIECE_STRAIGHT && angle > 3 {
                        flags |= 0x5;
                    }
                    let key = (loc_id, shape, angle, detail, custom_salt, None, flags);
                    let base = if let Some(model) = self.entities.loc_models.get(&key) {
                        model.clone()
                    } else {
                        let source = crate::loctype::ModelSource::new(
                            &pack, materials, billboards, emitters, detail,
                        );
                        let Some(model) = crate::loctype::build_base_model_custom(
                            &source,
                            loc,
                            flags,
                            shape,
                            angle,
                            request.custom.as_ref(),
                        )?
                        else {
                            continue;
                        };
                        self.entities.loc_models.insert(key, model.clone());
                        model
                    };
                    let mut model = base;
                    crate::loctype::finish_animated_model(
                        &mut model,
                        loc,
                        [shape, angle],
                        pose,
                        crate::gpumodel::TerrainHeights {
                            floor: &floor.heights,
                            above,
                        },
                        [x, y, z],
                    );
                    Some(model)
                } else {
                    None
                };
                let key = (
                    loc_id,
                    request.shape,
                    request.angle,
                    detail,
                    custom_salt,
                    (loc.hillchange != 0).then_some(footprint.centre),
                    0,
                );
                let mut model = if let Some(model) = posed {
                    model
                } else if let Some(model) = self.entities.loc_models.get(&key) {
                    model.clone()
                } else {
                    let source = crate::loctype::ModelSource::new(
                        &pack, materials, billboards, emitters, detail,
                    );
                    let Some(model) = crate::loctype::get_dynamic_model_custom(
                        &source,
                        loc,
                        crate::loctype::ModelRequest {
                            flags: 0x1F01F,
                            shape: request.shape,
                            rotation: request.angle,
                        },
                        crate::loctype::LocGround {
                            floor: Some(&floor.heights),
                            above,
                        },
                        [x, y, z],
                        request.custom.as_ref(),
                    )?
                    else {
                        continue;
                    };
                    self.entities.loc_models.insert(key, model.clone());
                    model
                };
                let srt = request.transform.map(|transform| {
                    let local = crate::player_picking::bounds(&mut model);
                    let srt = crate::scene::LocSrtPick {
                        srt: crate::map::LocSrt {
                            rot: [transform[0], transform[1], transform[2], transform[3]],
                            trans: [transform[4], transform[5], transform[6]],
                            scale: [transform[7], transform[8], transform[9]],
                        },
                        bounds: [
                            local.min[0],
                            local.min[1],
                            local.min[2],
                            local.max[0],
                            local.max[1],
                            local.max[2],
                        ],
                        horizontal_radius: model.horizontal_radius(),
                    };
                    model.apply_srt(transform);
                    srt
                });
                if !is_static && model.has_particles {
                    let key = crate::particle::keys::location(
                        request.level,
                        request.layer,
                        request.x,
                        request.z,
                    );
                    let mut particles = crate::particle::Binding {
                        key,
                        level: request.level,
                        ..Default::default()
                    };
                    particles.add_model(
                        &model,
                        &crate::particle::translation([x as f32, y as f32, z as f32]),
                        crate::particle::Rotation::IDENTITY,
                        crate::particle::keys::BODY,
                    );
                    self.particles.effect_bindings.push(particles);
                    particle_slots.push((entities.len(), key, true));
                }
                entities.push(crate::scene::TemporaryEntity {
                    player: usize::MAX,
                    npc_index: None,
                    location_key: Some((request.level, request.layer, request.x, request.z)),
                    pick: Some(crate::scene::TemporaryPick::Loc {
                        id: request.id,
                        shape: request.shape,
                        angle: request.angle,
                        tile: [request.x, request.z],
                        srt,
                    }),
                    transient: true,
                    level: request.level,
                    occlude_level: request.level,
                    position: [x as f32, y as f32, z as f32],
                    bounds: footprint.bounds(),
                    // The overlay height: the untransformed model's minY.
                    overlay_height: srt.map_or_else(|| model.min_y(), |srt| srt.bounds[1]),
                    transparent: model.has_transparency,
                    spot_shadow: false,
                    model: Some(model),
                });
            }
        }
        if let Some(store) = ground_store.as_deref() {
            let mut radii = HashMap::new();
            for (level, world_x, world_z, local_x, local_z, ranks) in ground_specs {
                let floor = self
                    .scene
                    .floors
                    .get(level.clamp(0, 3) as usize)
                    .and_then(Option::as_ref);
                let fine = [local_x * 512 + 256, local_z * 512 + 256];
                // The stack origin is the tile centre on the installed floor.
                let y = floor.map_or(0, |floor| floor.heights.get_fine_height(fine[0], fine[1]));
                // Raised entities in the tile's list and its ground decoration
                // lift the whole stack.
                let raise = {
                    let scene = self.scene.graph.as_ref();
                    let tile = scene.and_then(|scene| {
                        scene.tile(
                            level.clamp(0, 3) as usize,
                            local_x as usize,
                            local_z as usize,
                        )
                    });
                    let raised: Vec<i32> = tile
                        .map(|tile| {
                            tile.entities
                                .iter()
                                .filter_map(|entry| match *entry {
                                    // Static scenery overlay height (actors and the other
                                    // temporaries are never `raised`).
                                    crate::scene::PrimaryRef::Scenery(i) => {
                                        let e = scene?.scenery.get(i)?;
                                        e.raised
                                            .then(|| e.model.clone().map_or(0, |mut m| m.min_y()))
                                    }
                                    crate::scene::PrimaryRef::Temporary(_) => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let decoration = tile
                        .and_then(|tile| tile.ground_decoration)
                        .and_then(|g| scene?.ground_decors.get(g))
                        .map(|g| g.decor_height);
                    crate::obj_stack::overlay_raise(&raised, decoration)
                };
                // the tilt, sized by the previous draw's radius,
                // only while the stack rests on the ground.
                let stack_key = (level, local_x, local_z);
                let tilt = (raise == 0).then(|| {
                    let radius = self
                        .entities
                        .obj_stack_radius
                        .get(&stack_key)
                        .copied()
                        .unwrap_or(0);
                    crate::obj_stack::tilt(radius, y, |dx, dz| {
                        floor.map_or(0, |floor| {
                            floor.heights.get_fine_height(fine[0] + dx, fine[1] + dz)
                        })
                    })
                });
                let mut radius = 0;
                // tertiary, secondary, then primary.
                for rank in (0..3).rev() {
                    let Some((item_id, count)) = ranks[rank] else {
                        continue;
                    };
                    let Some(base) = u32::try_from(item_id).ok().and_then(|id| store.get(id))
                    else {
                        continue;
                    };
                    // The count model selects the last threshold that
                    // is met, then resolves that variant with count one.
                    let resolved_id = base
                        .inventory
                        .countobj
                        .as_ref()
                        .and_then(|variants| {
                            variants
                                .iter()
                                .filter(|(_, threshold)| *threshold != 0 && count >= *threshold)
                                .map(|(id, _)| *id)
                                .next_back()
                        })
                        .and_then(|id| u32::try_from(id).ok())
                        .unwrap_or(base.id);
                    let Some(item) = store.get(resolved_id) else {
                        continue;
                    };
                    let Some(&model_id) = item.models.first() else {
                        continue;
                    };
                    let key = (item.id, detail);
                    let mut model = if let Some(model) = self.entities.ground_models.get(&key) {
                        model.clone()
                    } else {
                        let mut raw = match crate::modelunlit::ModelUnlit::load(&pack, model_id) {
                            Ok(raw) => raw,
                            Err(error) => {
                                crate::logging::warn_repeated!(
                                    "[client910] ground object {} model {} unavailable: {error:#}",
                                    item.id,
                                    model_id
                                );
                                continue;
                            }
                        };
                        if raw.version < 13 {
                            raw.scale_by_power_of_two(2);
                        }
                        let mut model = crate::gpumodel::GpuModel::new(
                            &crate::gpumodel::ModelStores {
                                materials,
                                billboards,
                                emitters,
                            },
                            &raw,
                            crate::gpumodel::BuildParams {
                                flags: 0x1F01F,
                                ambient: item.inventory.ambient + 64,
                                contrast: item.inventory.contrast.saturating_mul(5) + 850,
                                detail,
                            },
                        )?;
                        for (&src, &dst) in item.recol_s.iter().zip(&item.recol_d) {
                            model.recolor(src as i16, dst as i16);
                        }
                        for (&src, &dst) in item.retex_s.iter().zip(&item.retex_d) {
                            model.retexture(materials, src as i16, dst as i16)?;
                        }
                        let [sx, sy, sz] = item.inventory.resize;
                        if sx != 128 || sy != 128 || sz != 128 {
                            model.scale(sx, sy, sz);
                        }
                        self.entities.ground_models.insert(key, model.clone());
                        model
                    };
                    // The largest drawn radius.
                    radius = radius.max(model.radius());
                    if item.scattered_drop {
                        // A scattered drop keeps its matrix while it stays in the
                        // stack; the stable per-object seed stands in for the
                        // random source.
                        let mut random = obj_stack_seed(level, world_x, world_z, item.id);
                        let [first, offset, second] = crate::obj_stack::scatter(|| {
                            f64::from(obj_stack_next(&mut random) >> 8) / f64::from(1u32 << 24)
                        });
                        model.rotate_y(first);
                        model.translate(offset, 0, 0);
                        model.rotate_y(second);
                    }
                    if let Some(t) = tilt {
                        model.rotate_x(t.pitch);
                        model.rotate_z(-t.roll);
                        model.translate(0, t.lift, 0);
                    }
                    let position = [fine[0] as f32, (y + raise - 10) as f32, fine[1] as f32];
                    if visibility.as_mut().is_some_and(|visibility| {
                        !visibility.visible_model(
                            [local_x, local_x, local_z, local_z],
                            &mut model,
                            position,
                            &self.scene.floors,
                        )
                    }) {
                        continue;
                    }
                    entities.push(crate::scene::TemporaryEntity {
                        player: usize::MAX,
                        npc_index: None,
                        location_key: None,
                        pick: Some(crate::scene::TemporaryPick::Obj {
                            tile: [local_x, local_z],
                            rank,
                        }),
                        transient: true,
                        level,
                        occlude_level: level,
                        // `trans.y - 10`, `trans.y` already raised.
                        position,
                        bounds: [local_x, local_x, local_z, local_z],
                        // The stack overlay height.
                        overlay_height: -10,
                        transparent: model.has_transparency,
                        spot_shadow: false,
                        model: Some(model),
                    });
                }
                radii.insert(stack_key, radius);
            }
            self.entities.obj_stack_radius = radii;
        }
        for trail in hint_trails.iter().flatten() {
            if trail.points.is_empty() {
                continue;
            }
            let mut model = if let Some(model) = self.entities.hint_models.get(&trail.model) {
                model.clone()
            } else {
                let mut raw = match crate::modelunlit::ModelUnlit::load(
                    &pack,
                    u32::try_from(trail.model).unwrap_or_default(),
                ) {
                    Ok(raw) => raw,
                    Err(error) => {
                        crate::logging::warn_repeated!(
                            "[client910] hint trail model {} unavailable: {error:#}",
                            trail.model
                        );
                        continue;
                    }
                };
                if raw.version < 13 {
                    raw.scale_by_power_of_two(2);
                }
                let model = crate::gpumodel::GpuModel::new(
                    &crate::gpumodel::ModelStores {
                        materials,
                        billboards,
                        emitters,
                    },
                    &raw,
                    crate::gpumodel::BuildParams {
                        flags: 0x1F01F,
                        ambient: 64,
                        contrast: 768,
                        detail,
                    },
                )?;
                self.entities.hint_models.insert(trail.model, model.clone());
                model
            };
            let mut point = trail.points[0];
            for target in trail.points.iter().skip(1) {
                while point != *target {
                    if point[0] < target[0] {
                        point[0] += 1;
                    } else if point[0] > target[0] {
                        point[0] -= 1;
                    }
                    if point[1] < target[1] {
                        point[1] += 1;
                    } else if point[1] > target[1] {
                        point[1] -= 1;
                    }
                    let local_x = point[0] - hint_base_x;
                    let local_z = point[1] - hint_base_z;
                    let in_scene = self.scene.graph.as_ref().is_some_and(|scene| {
                        local_x >= 0
                            && local_z >= 0
                            && local_x < scene.max_x as i32
                            && local_z < scene.max_z as i32
                    });
                    if !in_scene {
                        continue;
                    }
                    // The point keeps the current player level, its occlude level
                    // steps up under a link-below tile, and its height is the
                    // heightmap height at the current player level (bridge
                    // sampling included).
                    let fine = [local_x * 512 + 256, local_z * 512 + 256];
                    let level = hint_level;
                    let terrain = self
                        .core
                        .session
                        .game()
                        .and_then(|game| game.runtime.terrain.as_ref());
                    let occlude_level = level
                        + i32::from(
                            terrain
                                .is_some_and(|t| crate::minimap::link_below(t, local_x, local_z)),
                        );
                    let mut y =
                        crate::protocol910::terrain::height(terrain, fine[0], fine[1], level)
                            .unwrap_or(0);
                    // Lifted by the ground decoration on its tile.
                    if let Some(decor) = self.scene.graph.as_ref().and_then(|scene| {
                        let tile = scene.tile(
                            level.clamp(0, 3) as usize,
                            local_x as usize,
                            local_z as usize,
                        )?;
                        scene.ground_decors.get(tile.ground_decoration?)
                    }) {
                        y -= decor.decor_height;
                    }
                    entities.push(crate::scene::TemporaryEntity {
                        player: usize::MAX,
                        npc_index: None,
                        location_key: None,
                        pick: None,
                        transient: true,
                        level,
                        occlude_level,
                        position: [fine[0] as f32, y as f32, fine[1] as f32],
                        bounds: [local_x, local_x, local_z, local_z],
                        overlay_height: model.min_y(),
                        transparent: model.has_transparency,
                        spot_shadow: false,
                        model: Some(model.clone()),
                    });
                }
            }
        }
        let mut node_updates = Vec::new();
        let mut npc_spot_updates = Vec::new();
        let mut player_spot_updates = Vec::new();
        let mut npc_node_updates = Vec::new();
        let mut npc_height_updates = Vec::new();
        let npc_store = self
            .core
            .session
            .ui()
            .and_then(|ui| ui.engine.configs.npcs.clone());
        if let Some(npc_store) = npc_store {
            let game_cycle = self.core.session.game().map_or(0, |game| game.cycle);
            self.entities.npc_picks.clear();
            for spec in npc_specs {
                let Some(npc) = npc_store.get(spec.type_id) else {
                    continue;
                };
                let session = self.core.session.game();
                let sources = crate::npc_draw::Sources {
                    pack: &pack,
                    materials,
                    billboards,
                    emitters,
                    bases: session.map(|game| &game.inputs.bas),
                    detail,
                };
                let drawn = self.entities.npcs.draw(
                    animations,
                    &sources,
                    crate::npc_draw::Spec {
                        config: npc,
                        custom: spec.body.as_ref(),
                        bas: spec.bas,
                        bas_type: spec.bas_type.as_deref(),
                        path: &spec.path,
                        node: spec.node,
                        walk: spec.walk,
                        overlays: spec.overlays,
                        wear_angles: spec.wear_angles.as_deref(),
                        angle: spec.angle,
                        look: &spec.look,
                        shadow_node: spec.shadow_node,
                        fade: spec.fade,
                        fade_in: spec.fade_in,
                        decoration_offset: spec.decoration_offset,
                        terrain: session.and_then(|game| game.runtime.terrain.as_ref()),
                        cycle: game_cycle,
                    },
                );
                let drawn = match drawn {
                    Ok(Some(drawn)) => drawn,
                    Ok(None) => continue,
                    Err(error) => {
                        crate::logging::warn_repeated!(
                            "[client910] NPC {} draw: {error:#}",
                            spec.type_id
                        );
                        continue;
                    }
                };
                npc_node_updates.push((spec.index, drawn.node, drawn.walk, drawn.overlays));
                npc_height_updates.push((
                    spec.index,
                    drawn.min_y,
                    drawn.overlay_height,
                    drawn.ground,
                    spec.decoration_offset,
                    drawn.fade.alpha,
                ));
                self.entities.npc_picks.insert(
                    spec.index,
                    (
                        crate::npc_draw::Placement {
                            position: [spec.fine_x, spec.height, spec.fine_z],
                            rotation: spec.path.actor.rotation,
                            lift: crate::npc_draw::lift(spec.decoration_offset),
                        },
                        spec.look.pick,
                    ),
                );
                let model = drawn.body;
                // The NPC draw binds its particles: the yaw the draw matrix
                // carries is baked into the vertices here.
                let mut particles = crate::particle::Binding {
                    key: crate::particle::keys::npc(spec.index),
                    level: spec.level,
                    ..Default::default()
                };
                particles.add_model(
                    &model,
                    &crate::particle::translation([spec.fine_x, spec.height, spec.fine_z]),
                    crate::particle::Rotation::yaw(spec.angle),
                    crate::particle::keys::BODY,
                );
                self.particles.npc_bindings.insert(spec.index, particles);
                let tile_x = (spec.fine_x as i32) >> 9;
                let tile_z = (spec.fine_z as i32) >> 9;
                let size = i32::from(npc.size.max(1));
                let bounds = [tile_x, tile_x + size - 1, tile_z, tile_z + size - 1];
                let mut model = model;
                let mut shadow = drawn.shadow;
                if visibility.as_mut().is_some_and(|visibility| {
                    !visibility.visible_model(
                        bounds,
                        &mut model,
                        [spec.fine_x, spec.height, spec.fine_z],
                        &self.scene.floors,
                    ) && shadow.as_mut().is_none_or(|shadow| {
                        !visibility.visible_model(
                            bounds,
                            shadow,
                            [spec.fine_x, spec.height, spec.fine_z],
                            &self.scene.floors,
                        )
                    })
                }) {
                    continue;
                }
                let shadow = shadow.map(|shadow| crate::scene::TemporaryEntity {
                    player: usize::MAX,
                    npc_index: None,
                    location_key: None,
                    pick: None,
                    transient: true,
                    level: spec.level,
                    occlude_level: spec.occlude_level,
                    position: [spec.fine_x, spec.height, spec.fine_z],
                    bounds: [tile_x, tile_x + size - 1, tile_z, tile_z + size - 1],
                    overlay_height: drawn.min_y,
                    transparent: true,
                    spot_shadow: true,
                    model: Some(shadow),
                });
                let entity = crate::scene::TemporaryEntity {
                    player: usize::MAX,
                    npc_index: Some(spec.index),
                    location_key: None,
                    pick: None,
                    transient: true,
                    level: spec.level,
                    occlude_level: spec.occlude_level,
                    position: [spec.fine_x, spec.height, spec.fine_z],
                    bounds: [tile_x, tile_x + size - 1, tile_z, tile_z + size - 1],
                    overlay_height: drawn.min_y,
                    transparent: model.has_transparency || shadow.is_some(),
                    spot_shadow: false,
                    model: Some(model),
                };
                for _ in 1..spec.pushes {
                    entities.extend(shadow.clone());
                    particle_slots.push((
                        entities.len(),
                        crate::particle::keys::npc(spec.index),
                        true,
                    ));
                    entities.push(entity.clone());
                }
                entities.extend(shadow);
                particle_slots.push((entities.len(), crate::particle::keys::npc(spec.index), true));
                entities.push(entity);
            }
        }
        for spec in specs {
            // The animation's render flags
            // widen the request before the model-cache test, so
            // the cached base carries the label groups the pose moves.
            let mut node = spec.node;
            let pose = match node.as_mut().filter(|node| node.sequence.is_some()) {
                Some(node) => match animations.actor_pose(
                    &pack,
                    node,
                    crate::animation_assets::Filter::default(),
                ) {
                    Ok(pose) => Some(pose),
                    Err(error) => {
                        crate::logging::warn_repeated!(
                            "[client910] transient animation: {error:#}"
                        );
                        None
                    }
                },
                None => None,
            };
            let requested =
                crate::loctype::effect_flags(pose.as_ref().map_or(0, |pose| pose.flags));
            let cached = self.entities.effect_models.get(&spec.effect_id).cloned();
            let rebuild = crate::npc_type_model::rebuild_flags(cached.as_ref(), requested);
            let mut model = if let (None, Some(model)) = (rebuild, cached) {
                model
            } else {
                let Some(model) = crate::loctype::build_effect_model(
                    &pack,
                    materials,
                    billboards,
                    emitters,
                    &spec.effect,
                    rebuild.unwrap_or(requested),
                    detail,
                )?
                else {
                    continue;
                };
                self.entities
                    .effect_models
                    .insert(spec.effect_id, model.clone());
                model
            };
            // the copy is animated, then resized.
            if let Some(pose) = pose {
                model.has_transparency |= pose.flags & 0x100 != 0;
                model.apply_animation(&pose.transforms);
            }
            crate::loctype::effect_resize(&mut model, &spec.effect);
            if let Some(node) = node {
                if let Some((npc_index, slot)) = spec.npc_spot {
                    npc_spot_updates.push((npc_index, slot, node));
                } else if let Some((player_index, slot)) = spec.player_spot {
                    player_spot_updates.push((player_index, slot, node));
                } else {
                    node_updates.push((spec.projectile, spec.index, node));
                }
            }
            // The actor's draw matrix, applied after the particles bind through it.
            let mut actor_matrix = None;
            if let Some(actor) = spec.actor.as_ref() {
                actor.place(&mut model, &spec.effect, spec.orientation, spec.actor_angle);
                actor_matrix = Some(actor.matrix(spec.position));
            } else if spec.projectile {
                // The projectile model takes no yaw argument, so only the type's orientation turns it
                // before the flight pitch/yaw.
                crate::loctype::effect_yaw(&mut model, &spec.effect, 0);
                let to_angle =
                    |radians: f64| (radians * (8192.0 / std::f64::consts::PI)).round() as i32;
                model.rotate_x(to_angle(spec.pitch));
                model.rotate_y(to_angle(spec.yaw));
            } else {
                // The spot model yaw is `orientation * 2048`.
                crate::loctype::effect_yaw(
                    &mut model,
                    &spec.effect,
                    spec.orientation
                        .wrapping_mul(2048)
                        .wrapping_add(spec.actor_angle),
                );
            }
            // Projectiles and spots own a particle system each; actor spot
            // models join the actor's system.
            let (key, id_base) = if spec.projectile {
                (
                    crate::particle::keys::projectile(spec.index),
                    crate::particle::keys::BODY,
                )
            } else if let Some((npc_index, slot)) = spec.npc_spot {
                (
                    crate::particle::keys::npc(npc_index),
                    crate::particle::keys::spot(slot),
                )
            } else if let Some((player_index, slot)) = spec.player_spot {
                (
                    crate::particle::keys::player(player_index),
                    crate::particle::keys::spot(slot),
                )
            } else {
                (
                    crate::particle::keys::map_spot(spec.index),
                    crate::particle::keys::BODY,
                )
            };
            let rotation = if spec.projectile {
                let to_angle =
                    |radians: f64| (radians * (8192.0 / std::f64::consts::PI)).round() as i32;
                crate::particle::Rotation::pitch(to_angle(spec.pitch))
                    .then(&crate::particle::Rotation::yaw(to_angle(spec.yaw)))
            } else {
                crate::particle::Rotation::yaw(
                    spec.orientation
                        .wrapping_mul(2048)
                        .wrapping_add(spec.actor_angle),
                )
            };
            let mut particles = crate::particle::Binding {
                key,
                level: spec.level,
                ..Default::default()
            };
            match actor_matrix.as_ref() {
                // Bind the actor's particles with its draw matrix.
                Some(matrix) => particles.add_model(
                    &model,
                    matrix,
                    crate::particle::Rotation::IDENTITY,
                    id_base,
                ),
                None => particles.add_model(
                    &model,
                    &crate::particle::translation(spec.position),
                    rotation,
                    id_base,
                ),
            }
            self.particles.effect_bindings.push(particles);
            if let Some(actor) = spec.actor.as_ref() {
                actor.apply_matrix(&mut model);
            }
            if let Some([hue, saturation, luminence, weight]) = spec.antimacro {
                model.tint(hue, saturation, luminence, weight);
            }
            // Projectiles and map spots draw their own particles; actor spot
            // models are drawn (and their particles) by the actor's draw.
            let draws = spec.npc_spot.is_none() && spec.player_spot.is_none();
            let tile_x = (spec.position[0] as i32) >> 9;
            let tile_z = (spec.position[2] as i32) >> 9;
            if visibility.as_mut().is_some_and(|visibility| {
                !visibility.visible_model(
                    [tile_x, tile_x, tile_z, tile_z],
                    &mut model,
                    spec.position,
                    &self.scene.floors,
                )
            }) {
                continue;
            }
            let entity = crate::scene::TemporaryEntity {
                player: usize::MAX,
                npc_index: None,
                location_key: None,
                pick: None,
                transient: true,
                level: spec.level,
                occlude_level: spec.occlude_level,
                position: spec.position,
                bounds: [tile_x, tile_x, tile_z, tile_z],
                overlay_height: model.min_y(),
                transparent: model.has_transparency,
                spot_shadow: false,
                model: Some(model),
            };
            for _ in 1..spec.pushes {
                particle_slots.push((entities.len(), key, draws));
                entities.push(entity.clone());
            }
            particle_slots.push((entities.len(), key, draws));
            entities.push(entity);
        }
        // The NPC draw stamps its draw cycle with the scene cycle (this
        // full redraw's, `ClientCore::begin_draw_scene`); the player bodies
        // use the same cycle (`players.refresh`).
        let next_scene_cycle = self.core.scene_cycle;
        if let Some(game) = self.core.session.game_mut() {
            for (projectile, index, node) in node_updates {
                if projectile {
                    if let Some(projectile) = game
                        .runtime
                        .feed
                        .state
                        .zones
                        .transients
                        .projectiles
                        .get_mut(index)
                    {
                        projectile.animation = node;
                    }
                } else if let Some(spot) = game
                    .runtime
                    .feed
                    .state
                    .zones
                    .transients
                    .spots
                    .get_mut(index)
                {
                    spot.animation = Some(node);
                }
            }
            for (index, node, walk, overlays) in npc_node_updates {
                if let Some(npc) = game.runtime.feed.state.npcs.entities.get_mut(&index) {
                    npc.path.animation.main = node;
                    if let Some(walk) = walk {
                        npc.path.actor.walk.node = walk;
                    }
                    npc.path.animation.overlays = overlays;
                }
            }
            for (index, min_y, height, ground, decoration_offset, fade_alpha) in npc_height_updates
            {
                if let Some(npc) = game.runtime.feed.state.npcs.entities.get_mut(&index) {
                    let scene = &mut npc.path.actor.scene;
                    scene.min_y = min_y;
                    scene.height = height;
                    scene.ground = ground;
                    scene.decoration_offset = decoration_offset;
                    scene.draw_cycle = next_scene_cycle;
                    npc.fade_alpha = fade_alpha;
                }
            }
            for (index, slot, node) in npc_spot_updates {
                if let Some(npc) = game.runtime.feed.state.npcs.entities.get_mut(&index) {
                    if let Some(spot) = npc.path.animation.spots.get_mut(slot) {
                        spot.node = node;
                    }
                }
            }
            for (index, slot, node) in player_spot_updates {
                if let Some(player) = game
                    .runtime
                    .feed
                    .state
                    .players
                    .players
                    .get_mut(index)
                    .and_then(Option::as_mut)
                {
                    if let Some(spot) = player.animation.spots.get_mut(slot) {
                        spot.node = node;
                    }
                }
            }
        }
        if let Some(scene) = self.scene.graph.as_mut() {
            let mut slots = particle_slots.into_iter().peekable();
            for (position, entity) in entities.into_iter().enumerate() {
                let id = scene.add_temporary(entity);
                if let Some((_, key, draws)) = slots.next_if(|(p, _, _)| *p == position) {
                    self.particles.temporary.insert(id, (key, draws));
                }
            }
        }
        Ok(())
    }

    pub(super) fn upload_transient_meshes(&mut self) -> anyhow::Result<()> {
        self.scene.meshes.clear_transients();
        let Some(renderer) = self.renderer.as_mut() else {
            return Ok(());
        };
        let Some(materials) = self.scene.material_store.as_ref() else {
            return Ok(());
        };
        if self.scene.pack_root.is_none() {
            return Ok(());
        }
        let Some(live) = self.scene.live.as_ref() else {
            return Ok(());
        };
        let Some(scene) = self.scene.graph.as_ref() else {
            return Ok(());
        };
        let pack = self.pack.clone();
        self.scene.meshes.upload_transients(
            renderer.faithful(),
            &pack,
            materials,
            live,
            scene,
            self.scene.floor_base,
        )
    }

    /// Each retained `TEXT_COORD` label still before its expiry cycle is
    /// projected by `project(level, (x << 9) + 256, (z << 9) + 256, 0,
    /// height * 2)` (the same projection the 2D entity elements use) and
    /// drawn centred at `(int) (projection + viewport origin)`. The client draws
    /// every label without a visibility test: the map border yields `-1`
    /// and a point outside the clip volume NaN (cast to 0), as here.
    pub(super) fn scene_text_overlays(&self) -> Vec<crate::ui_backend::SceneText> {
        let Some(session) = self.core.session.as_ref() else {
            return Vec::new();
        };
        let Some(game) = session.game.as_ref() else {
            return Vec::new();
        };
        let ui = &session.ui;
        let Some(cpu) = self.scene_element_projection() else {
            return Vec::new();
        };
        let Some((rect, _)) = ui.state.viewport else {
            return Vec::new();
        };
        let terrain = game.runtime.terrain.as_ref();
        let (map_w, map_h) = (game.runtime.map.width, game.runtime.map.height);
        game.runtime
            .feed
            .state
            .zones
            .text_coords
            .iter()
            .filter(|label| label.expiry_cycle > game.cycle)
            .map(|label| {
                let (x, z) = ((label.x << 9) + 256, (label.z << 9) + 256);
                let projected =
                    if x < 512 || z < 512 || x > (map_w - 2) * 512 || z > (map_h - 2) * 512 {
                        [-1.0, -1.0]
                    } else {
                        let ground =
                            crate::protocol910::terrain::height(terrain, x, z, label.level)
                                .unwrap_or(0);
                        let p = cpu.project_clipped([
                            x as f32,
                            (ground - label.height * 2) as f32,
                            z as f32,
                        ]);
                        [p[0], p[1]]
                    };
                crate::ui_backend::SceneText {
                    position: [
                        (projected[0] + rect[0] as f32) as i32,
                        (projected[1] + rect[1] as f32) as i32,
                    ],
                    colour: label.colour,
                    text: label.text.clone(),
                }
            })
            .collect()
    }

    /// The camera for 2D scene elements: the player-picking frame (`player_picking::Frame::new`) — the camera
    /// target rebased to the scene origin so scene-local fine coordinates and
    /// the scene heightmap project directly, the GPU toolkit's unflipped
    /// projection, and the scene viewport in canvas (logical) units.
    pub(super) fn scene_element_projection(&self) -> Option<crate::camera::CpuProjection> {
        let renderer = self.renderer.as_ref()?;
        let session = self.core.session.as_ref()?;
        let game = session.game.as_ref()?;
        let ui = &session.ui;
        let (rect, _) = ui.state.viewport?;
        let mut local = self.view.camera.scene_camera(renderer.scene_size());
        local.target[0] -= game.runtime.map.base_x << 9;
        local.target[2] -= game.runtime.map.base_z << 9;
        let mut projection = local.projection();
        crate::camera::glx_flip_y(&mut projection);
        Some(crate::camera::CpuProjection::new(
            local.view_entries(),
            projection,
            [0, 0, rect[2], rect[3]],
        ))
    }

    /// The 2D entity elements over the live players and NPCs, projected with
    /// the scene camera, plus the tile hint arrows. The pure pass lives in
    /// `entity_elements`; this owner supplies projection, sprites and fonts.
    pub(super) fn scene_entity_elements(
        &mut self,
    ) -> (
        crate::entity_elements::Elements,
        Vec<crate::entity_elements::Draw>,
    ) {
        use crate::entity_elements as ee;
        let empty = || (ee::Elements::default(), Vec::new());
        let Some(cpu) = self.scene_element_projection() else {
            return empty();
        };
        let scene_cycle = self.core.scene_cycle;
        let arrow_state = self.minimap.hint_arrow_state.clone();
        let hint_sprites = self.minimap.hintarrows.clone();
        let pack = self.pack.clone();
        let Some(session) = self.core.session.as_mut() else {
            return empty();
        };
        let (Some(game), ui) = (session.game.as_mut(), &mut session.ui) else {
            return empty();
        };
        let Some((rect, _)) = ui.state.viewport else {
            return empty();
        };
        let terrain = game.game.runtime.terrain.as_ref();
        let (map_w, map_h) = (game.game.runtime.map.width, game.game.runtime.map.height);
        // The projection: the map border yields -1, the height is
        // subtracted from the heightmap height, then the clipped projection.
        let project = |level: i32, fine_x: f32, fine_z: f32, height: i32| -> [f32; 2] {
            let (x, z) = (fine_x as i32, fine_z as i32);
            if x < 512 || z < 512 || x > (map_w - 2) * 512 || z > (map_h - 2) * 512 {
                return [-1.0, -1.0];
            }
            let ground = crate::protocol910::terrain::height(terrain, x, z, level).unwrap_or(0);
            let p = cpu.project_clipped([x as f32, (ground - height) as f32, z as f32]);
            [p[0], p[1]]
        };
        let project_tile = |level: i32, fine_x: i32, fine_z: i32, height: i32| {
            project(level, fine_x as f32, fine_z as f32, height)
        };
        let limits = game
            .game
            .inputs
            .appearance
            .defaults
            .graphics
            .entity_limits(game.game.inputs.logic_rate);
        let graphics = &game.game.inputs.appearance.defaults.graphics;
        let hitmark_positions = graphics.hitmark_positions.clone();
        let headbar_gap = graphics.scalars.headbar_gap;
        let varps = &game.game.runtime.feed.state.varps;
        let bits = &game.game.inputs.bits;
        let read_var = |bit: bool, id: i32| -> Option<i32> {
            let varps = varps.as_ref()?;
            if bit {
                varps.get_bit(&bits.get(id, false).ok()?).ok()
            } else {
                varps.get(id).ok()
            }
        };
        let social = &ui.engine.social;
        let friend_test = |name: &str| social.friend_test(name);
        let emoji = &ui.engine.builtins.emoji;
        let substitute_emoji = |text: &str| {
            String::from_utf16_lossy(&emoji.substitute(&text.encode_utf16().collect::<Vec<_>>()))
        };
        let frame = ee::Frame {
            loop_cycle: game.game.cycle,
            scene_cycle,
            viewport: rect,
            hitmark_positions: &hitmark_positions,
            headbar_gap,
            player_chat_visible: limits.player_chat_visible,
            npc_chat_visible: limits.npc_chat_visible,
            chat_effects: game.game.chat_effects,
            public_chat_filter: ui.engine.messages.filters[0].unwrap_or(0),
            friend_test: &friend_test,
            emoji: emoji
                .autochat
                .then_some(&substitute_emoji as &dyn Fn(&str) -> String),
            logic_rate: game.game.inputs.logic_rate,
            hitmarks: &game.game.inputs.combat_types.hits,
            headbars: &game.game.inputs.combat_types.bars,
            read_var: &read_var,
        };
        let npc_store = ui.engine.configs.npcs.clone();
        let bas = &game.game.inputs.bas;
        let bas_height = |id: i32| bas.get(&id).map_or(-1, |b| b.height);
        // The entity height: the entity tile's ground decoration offset.
        let scene_graph = self.scene.graph.as_ref();
        let ground_decoration = |level: i32, fine_x: f32, fine_z: f32| -> Option<i32> {
            let scene = scene_graph?;
            let (x, z) = ((fine_x as i32) >> 9, (fine_z as i32) >> 9);
            if x < 1 || z < 1 || x > map_w - 1 || z > map_h - 1 {
                return None;
            }
            let tile = scene.tile(usize::try_from(level).ok()?, x as usize, z as usize)?;
            tile.ground_decoration
                .map(|i| scene.ground_decors[i].decor_height)
        };
        let local = game.game.runtime.map.local;
        let local_level = game
            .game
            .runtime
            .feed
            .state
            .players
            .players
            .get(local)
            .and_then(Option::as_ref)
            .map_or(0, |p| p.level);
        let high = game.game.runtime.feed.state.players.high_indices.clone();
        // An NPC row reads the player at the same high-resolution index
        // (a stale slot past the live count) for its headbar sprites.
        let npc_bar_partners: Vec<i32> = {
            let players = &game.game.runtime.feed.state.players;
            (0..game.runtime.feed.state.npcs.slots.len())
                .map(|k| {
                    players
                        .high_resolution_player(high.len() + k)
                        .map_or(0, |p| p.partner)
                })
                .collect()
        };
        let mut entities: Vec<ee::Entity> = Vec::new();
        // High-resolution players first, then the NPC slots.
        let mut players: Vec<(usize, &mut crate::entities910::Player)> = game
            .game
            .runtime
            .feed
            .state
            .players
            .players
            .iter_mut()
            .enumerate()
            .filter_map(|(i, p)| p.as_mut().map(|p| (i, p)))
            .filter(|(i, _)| high.contains(i))
            .collect();
        players.sort_by_key(|(i, _)| high.iter().position(|h| h == i));
        for (index, p) in players {
            let s = &p.actor.scene;
            let skip = s.priority < 0 || (s.draw_cycle != scene_cycle && p.level != local_level);
            let deferred = s.deferred;
            let height = ee::entity_height(
                bas_height(p.appearance.bas),
                s.height,
                ground_decoration(p.level, p.fine_x, p.fine_z),
            );
            let head_ids = p.appearance.head_ids;
            let head_groups = p.appearance.head_groups;
            let partner = p.partner;
            entities.push(ee::Entity {
                path: p,
                kind: ee::Kind::Player {
                    index,
                    head_ids,
                    head_groups,
                    partner,
                },
                height,
                skip,
                deferred,
            });
        }
        let slots = game.game.runtime.feed.state.npcs.slots.clone();
        let mut npcs: Vec<(usize, &mut crate::entities910::Npc)> = game
            .game
            .runtime
            .feed
            .state
            .npcs
            .entities
            .iter_mut()
            .filter(|(i, _)| slots.contains(i))
            .map(|(i, n)| (*i, n))
            .collect();
        npcs.sort_by_key(|(i, _)| slots.iter().position(|s| s == i));
        for (index, n) in npcs {
            let row = slots.iter().position(|s| *s == index).unwrap_or(0);
            let base_type = u32::try_from(n.type_id)
                .ok()
                .and_then(|id| npc_store.as_deref().and_then(|store| store.get(id)));
            // A null multinpc selection skips the entity.
            let visible = base_type
                .is_some_and(|t| t.multinpc.is_empty() || t.multi_npc(&read_var).is_some());
            let bas_id = if n.bas_override != -1 {
                n.bas_override
            } else {
                base_type.map_or(-1, |t| t.bas)
            };
            let s = &n.path.actor.scene;
            // as for players: `drawPriority < 0 || sceneCycle !=
            // drawCycle && the local player level != level`.
            let skip = !visible
                || s.priority < 0
                || (s.draw_cycle != scene_cycle && n.path.level != local_level);
            let deferred = s.deferred;
            let height = ee::entity_height(
                bas_height(bas_id),
                s.height,
                ground_decoration(n.path.level, n.path.fine_x, n.path.fine_z),
            );
            entities.push(ee::Entity {
                path: &mut n.path,
                kind: ee::Kind::Npc {
                    index,
                    head_icons: n.head_icons.as_ref(),
                    bar_partner: npc_bar_partners.get(row).copied().unwrap_or(0),
                },
                height,
                skip,
                deferred,
            });
        }
        let arrows: Vec<ee::HintArrow> = arrow_state
            .iter()
            .flatten()
            .filter_map(|a| {
                let target = match a.hint_type {
                    1 => a.npc_index?,
                    10 => a.player_index?,
                    _ => return None,
                };
                Some(ee::HintArrow {
                    hint_type: i32::from(a.hint_type),
                    target: target as i32,
                    sprite: i32::from(a.sprite),
                    blink: a.blink,
                })
            })
            .collect();
        let tiles: Vec<(i32, [i32; 2], i32, i32)> = arrow_state
            .iter()
            .flatten()
            .filter(|a| a.hint_type == 2)
            .filter_map(|a| Some((a.level?, a.fine?, a.height, i32::from(a.sprite))))
            .collect();
        struct Res<'a> {
            pack: &'a Pack,
            target: &'a mut crate::ui_backend::Target,
            fonts: Option<&'a crate::ui_fonts::Fonts>,
            hints: &'a [std::rc::Rc<crate::ui_sprites::Sprite>],
        }
        impl Res<'_> {
            fn font(&self, f: ee::FontRef) -> Option<std::rc::Rc<crate::ui_fonts::Font>> {
                let fonts = self.fonts?;
                let id = match f {
                    ee::FontRef::P11 => *fonts.ids.as_ref()?.first()?,
                    ee::FontRef::B12 => *fonts.ids.as_ref()?.get(2)?,
                    ee::FontRef::Config { id, mono } => {
                        return fonts.get_font(id, true, mono).ok().flatten()
                    }
                };
                fonts.get_font(id, true, true).ok().flatten()
            }
            fn width(&self, f: ee::FontRef, text: &str) -> i32 {
                let units: Vec<u16> = text.encode_utf16().collect();
                self.font(f)
                    .and_then(|font| font.metrics.width_utf16(&units, None).ok())
                    .unwrap_or(0)
            }
        }
        impl ee::Resources for Res<'_> {
            fn config_sprite(
                &mut self,
                group: i32,
            ) -> Option<std::rc::Rc<crate::ui_sprites::Sprite>> {
                self.target.config_sprite(self.pack, group)
            }
            fn head_icon(
                &mut self,
                group: i32,
                frame: i32,
            ) -> Option<std::rc::Rc<crate::ui_sprites::Sprite>> {
                self.target
                    .scene_icon(self.pack, group, i16::try_from(frame).ok()?)
            }
            fn hint_arrow(&mut self, index: i32) -> Option<std::rc::Rc<crate::ui_sprites::Sprite>> {
                self.hints.get(usize::try_from(index).ok()?).cloned()
            }
            fn has_font(&mut self, f: ee::FontRef) -> bool {
                self.font(f).is_some()
            }
            fn string_width(&mut self, f: ee::FontRef, text: &str) -> i32 {
                self.width(f, text)
            }
            fn ascent(&mut self, f: ee::FontRef) -> i32 {
                self.font(f).map_or(0, |font| font.metrics.ascent)
            }
            fn descent(&mut self, f: ee::FontRef) -> i32 {
                self.font(f).map_or(0, |font| font.metrics.descent)
            }
        }
        let mut res = Res {
            pack: &pack,
            target: &mut ui.target,
            fonts: ui.state.fonts.as_ref(),
            hints: &hint_sprites,
        };
        let elements = ee::draw(&frame, &mut entities, &arrows, &project, &mut res);
        let tile_arrows = ee::draw_tile_hint_arrows(&frame, &tiles, &project_tile, &mut res);
        (elements, tile_arrows)
    }

    /// NPC cover markers: the pairs of `cover_marker::pairs`, drawn between
    /// the entity loop and the chat of the 2D entity elements, each with its
    /// cover marker clickbox. The renderer publishes the clickboxes
    /// to the retained menu owner for the next scene-input tick.
    ///
    /// While a cutscene draws, entity placement makes no pairing; the draw
    /// walks the last ordinary frame's pairing again ([`cover_marker::Memory`]),
    /// each owner's slot counter stepping below zero, against the logic
    /// actors (the cutscene's own actors are the drawn ones).
    pub(super) fn scene_cover_marker_overlays(
        &mut self,
    ) -> (
        Vec<crate::ui_backend::SceneIcon>,
        Vec<crate::ui_runtime::CoverMarker>,
    ) {
        let none = || (Vec::new(), Vec::new());
        let Some(cpu) = self.scene_element_projection() else {
            return none();
        };
        let pack = self.pack.clone();
        let cutscene_actors = self.core.drawing_cutscene;
        let memory = &mut self.entities.cover_marker_memory;
        let Some(session) = self.core.session.as_mut() else {
            memory.clear();
            return none();
        };
        let (Some(game), ui) = (session.game.as_ref(), &mut session.ui) else {
            memory.clear();
            return none();
        };
        let replaying = game.cutscene.scene_state == 0;
        let Some((rect, _)) = ui.state.viewport else {
            return none();
        };
        // The logic actors: the feed's, or, while the cutscene's actors are
        // swapped into the feed for this draw, the ones parked in the cutscene.
        let (players, npcs) = if replaying && cutscene_actors {
            (&game.cutscene.players, &game.cutscene.npcs)
        } else {
            let state = &game.runtime.feed.state;
            (&state.players, &state.npcs)
        };
        let state = &game.runtime.feed.state;
        let varps = &state.varps;
        let bits = &game.inputs.bits;
        let read = |bit: bool, id: i32| -> Option<i32> {
            let varps = varps.as_ref()?;
            if bit {
                varps.get_bit(&bits.get(id, false).ok()?).ok()
            } else {
                varps.get(id).ok()
            }
        };
        let npc_store = ui.engine.configs.npcs.clone();
        // A multinpc selection's own marker, else the base type's.
        let cover_marker = |n: &crate::entities910::Npc| -> i32 {
            let base = u32::try_from(n.type_id)
                .ok()
                .and_then(|id| npc_store.as_deref().and_then(|store| store.get(id)));
            let Some(base) = base else {
                return n.covermarker;
            };
            if !base.multinpc.is_empty() {
                let selected = base
                    .multi_npc(&read)
                    .and_then(|id| npc_store.as_deref().and_then(|store| store.get(id)));
                if let Some(t) = selected.filter(|t| t.covermarker != -1) {
                    return t.covermarker;
                }
            }
            base.covermarker
        };
        let mut rows = Vec::new();
        let mut slots = Vec::new();
        for &index in &players.high_indices {
            let Some(p) = players.players.get(index).and_then(Option::as_ref) else {
                continue;
            };
            rows.push(crate::cover_marker::Actor {
                key: index as i32,
                npc: false,
                level: p.level,
                fine: [p.fine_x as i32, p.fine_z as i32],
                size: p.size,
                priority: p.actor.scene.priority,
                deferred: p.actor.scene.deferred,
                cover_marker: -1,
            });
            slots.push(None);
        }
        for &index in &npcs.slots {
            let Some(n) = npcs.entities.get(&index) else {
                continue;
            };
            rows.push(crate::cover_marker::Actor {
                key: index as i32 + 2048,
                npc: true,
                level: n.path.level,
                fine: [n.path.fine_x as i32, n.path.fine_z as i32],
                size: n.path.size,
                priority: n.path.actor.scene.priority,
                deferred: n.path.actor.scene.deferred,
                cover_marker: cover_marker(n),
            });
            slots.push(Some(index));
        }
        let size = [game.runtime.map.width, game.runtime.map.height];
        let pairs: Vec<crate::cover_marker::Pair> = if replaying {
            // An actor that has left the lists has no draw.
            let index = |key: i32| rows.iter().position(|r| r.key == key);
            memory
                .replay()
                .into_iter()
                .filter_map(|(owner, marker, slot)| {
                    Some(crate::cover_marker::Pair {
                        owner: index(owner)?,
                        marker: index(marker)?,
                        slot,
                    })
                })
                .collect()
        } else {
            let pairs = crate::cover_marker::pairs(&rows, size);
            memory.record(&rows, &pairs);
            pairs
        };
        let terrain = game.runtime.terrain.as_ref();
        let mut icons = Vec::new();
        let mut markers = Vec::new();
        for pair in pairs {
            let (owner, marker) = (&rows[pair.owner], &rows[pair.marker]);
            let Some(npc) = slots[pair.marker] else {
                continue;
            };
            // frame 0 of the marker group, or nothing at all.
            let Some(sprite) = ui.target.scene_icon(&pack, marker.cover_marker, 0) else {
                continue;
            };
            // `project(level, x, z, size * 256, 0)`: the map border yields
            // -1; otherwise the view matrix is translated by `size * 256`
            // along view x.
            let [x, z] = owner.fine;
            let projection =
                if x < 512 || z < 512 || x > (size[0] - 2) * 512 || z > (size[1] - 2) * 512 {
                    [-1.0, -1.0]
                } else {
                    let ground = crate::protocol910::terrain::height(terrain, x, z, owner.level)
                        .unwrap_or(0);
                    let mut view = cpu.view;
                    view[12] += (owner.size * 256) as f32;
                    let shifted =
                        crate::camera::CpuProjection::new(view, cpu.projection, cpu.viewport);
                    let p = shifted.project_clipped([x as f32, ground as f32, z as f32]);
                    [p[0], p[1]]
                };
            let [px, py] = crate::cover_marker::position(projection, [rect[0], rect[1]], pair.slot);
            icons.push(crate::ui_backend::SceneIcon {
                position: [px, py],
                sprite,
                outline: pair.owner == pair.marker,
            });
            markers.push(crate::ui_runtime::CoverMarker {
                npc,
                rect: [px, py, px + 16, py + 16],
            });
        }
        (icons, markers)
    }
}

/// The pick entries of the transient NPC bodies (`npc_draw::pickable`).
pub(super) fn append_npc_picks(
    scene: &mut crate::scene::Scene,
    picks: &HashMap<usize, (crate::npc_draw::Placement, crate::npc_draw::PickOptions)>,
    frame: &mut crate::player_picking::Frame,
) {
    let mut entries = Vec::new();
    for temporary in &mut scene.temporary {
        let Some(index) = temporary.npc_index else {
            continue;
        };
        let Some(model) = temporary.model.as_mut() else {
            continue;
        };
        let depth = crate::ui_scene_options::transform(
            &frame.view,
            temporary.position[0],
            temporary.position[1],
            temporary.position[2],
        )[2] as i32;
        let place = picks.get(&index).map(|(place, options)| (place, options));
        entries.push(crate::npc_draw::pickable(index, model, place, depth, frame));
    }
    frame.npc_picks = entries;
}
