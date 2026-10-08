//! The cutscene stack: the `cutscenes` JS5 file format, load/reset/save/finish,
//! and the per-logic-cycle action clock driven from the game update.
//!
//! Ownership split:
//! * [`Definition`] is the decoded file.
//! * [`Manager`] holds the cutscene state (scene state, cutscene id and
//!   appearance, camera-active flag, the playback start cycle and next action
//!   index, the fade and the map-square capacity). It lives on
//!   [`crate::game_runtime::Game`].
//! * Actions that reach other owners (camera, audio, client scripts,
//!   viewport profile, outgoing packets) are queued as [`UiRequest`]s and
//!   drained by the retained UI runtime in the same logic cycle, before the
//!   interface walk and before the frame is drawn
//!   (`ui_host_game::apply_cutscene_requests`).
//! * Cutscene NPC/player entities are real [`crate::entities910`] actors
//!   stored in the manager's own `Players`/`Npcs` collections and advanced by
//!   `actor::tick_cutscene`.
use crate::ui_bytes::Cursor;
use anyhow::{Context, Result};
use rs910_core::fault::Fault;
use std::collections::BTreeMap;

// The file-format half moved to rs910-config (Phase 2.6).
pub use rs910_config::cutscene_file::*;

/// The cutscene loading stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    ReadFile,
    WaitModels,
    Ready,
}

/// Runtime half of a cutscene entity: the actor
/// itself lives in [`Manager::npcs`] (key = entity index) or
/// [`Manager::players`] (slot [`Manager::player_slot`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntityState {
    pub exists: bool,
    /// The draw priority, taken from the manager's draw-priority counter.
    pub draw_priority: i32,
}

/// Runtime half of a cutscene location.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocationState {
    pub level: i32,
    pub x: i32,
    pub z: i32,
    pub angle: i32,
}

/// The screen fade (start/end cycle and start/end colour) with its initial
/// values: opaque black until the first fade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fade {
    pub start_cycle: i32,
    pub end_cycle: i32,
    /// Start alpha, red, green and blue.
    pub start: [i32; 4],
    /// `fadeAlpha/Red/Green/Blue`.
    pub end: [i32; 4],
}
impl Default for Fade {
    fn default() -> Self {
        Self {
            start_cycle: -1,
            end_cycle: -1,
            start: [0; 4],
            end: [255, 0, 0, 0],
        }
    }
}
impl Fade {
    /// Start the fade cutscene action.
    pub fn begin(&mut self, cycle: i32, duration: i32, colour: i32) {
        self.start_cycle = cycle;
        self.end_cycle = duration.wrapping_add(cycle);
        self.start = self.end;
        let c = colour as u32;
        self.end = [
            (c >> 24) as i32,
            (c >> 16 & 0xFF) as i32,
            (c >> 8 & 0xFF) as i32,
            (c & 0xFF) as i32,
        ];
    }
    /// The retained UI draw owner's copy.
    pub fn ui(&self) -> crate::screen_fade::Fade {
        crate::screen_fade::Fade {
            start_cycle: self.start_cycle,
            end_cycle: self.end_cycle,
            start: self.start,
            end: self.end,
        }
    }
}

/// Side effects the actions perform directly on the camera, audio, client
/// scripts, the minimenu or the game connection. They are drained by
/// `ui_host_game::apply_cutscene_requests` inside the same logic update.
#[derive(Clone, Debug, PartialEq)]
pub enum UiRequest {
    /// Close and reset the minimenu.
    CloseMenu,
    /// Save the state: viewport limits and the song.
    SaveState { viewport: [i32; 2] },
    /// Restore the saved state (only acts when state was saved).
    RestoreState,
    /// Move the camera to `(x, z, height)` with the given acceleration and speed.
    CameraMoveTo {
        x: i32,
        z: i32,
        height: i32,
        acceleration: i32,
        speed: i32,
        instant: bool,
    },
    /// Force the camera angle.
    CameraForceAngle { pitch: i32, yaw: i32, roll: i32 },
    /// Move the camera along its two spline paths.
    CameraMoveAlong {
        splines: [Vec<Vec<i32>>; 2],
        pos_keyframe: usize,
        target_keyframe: usize,
        min_speed: i32,
        max_speed: i32,
    },
    /// Create the sound and validate its volume at decode time, keyed by the
    /// action index.
    SoundCreate {
        tag: usize,
        sound: i32,
        repeats: i32,
        volume: i32,
        rate: i32,
    },
    /// Start the sound.
    SoundStart { tag: usize },
    /// `cleanup`: `fadeOut(50)` and hand the sound to the mixer.
    SoundCleanup { tag: usize },
    /// Play a jingle.
    Jingle { id: i32, volume: i32 },
    /// Play a song (volume 255).
    Song { id: i32 },
    /// Preload a song.
    PreloadSong { id: i32, volume: i32 },
    /// Restore the song saved before the cutscene.
    RestoreSong,
    /// Run the cutscene subtitle script.
    Subtitle {
        cutscene: i32,
        text: String,
        subtitle: i32,
    },
    /// Run the cutscene-end script.
    End { cutscene: i32 },
    /// `CUTSCENE_FINISHED` (client packet) with `p1(skipped ? 0 : 1)`.
    Finished { completed: bool },
}

/// UI-owned cutscene state: the saved viewport limits and song (min/max
/// viewport height and fov, the saved song), the
/// mirrored scene state/fade for interface drawing, the cancel binding
/// result and this update's requests.
#[derive(Clone, Debug)]
pub struct UiState {
    pub scene_state: i32,
    /// The mirrored cutscene id; the minimenu update returns early while it
    /// is `>= 0`.
    pub client_id: i32,
    pub fade: crate::screen_fade::Fade,
    /// `[minViewportHeight, maxViewportHeight, minViewportFov, maxViewportFov]`.
    pub saved: Option<[i32; 4]>,
    pub saved_song: i32,
    pub cancel_binding: Option<crate::ui_defaults::Binding>,
    pub cancel: bool,
    pub requests: Vec<UiRequest>,
}
impl Default for UiState {
    fn default() -> Self {
        Self {
            scene_state: 3,
            client_id: -1,
            fade: Fade::default().ui(),
            saved: None,
            saved_song: -1,
            cancel_binding: None,
            cancel: false,
            requests: Vec::new(),
        }
    }
}

/// The cutscene manager state plus the cutscene-owned client state.
pub struct Manager {
    /// The scene state: 3 normal, 2 loading, 1 built,
    /// 0 playing, 4 finished and waiting for the server's rebuild.
    pub scene_state: i32,
    /// The client-side cutscene id.
    pub client_id: i32,
    /// Whether the cutscene camera is active.
    pub camera_active: bool,
    /// The loop cycle playback started.
    pub start_cycle: i32,
    /// The next action index.
    pub next_action: usize,
    /// The cutscene appearance bytes.
    pub appearance: Option<Vec<u8>>,
    /// The map-square array capacity of the cutscene rebuild.
    pub map_capacity: i32,
    /// The last rebuild was a cutscene rebuild.
    pub rebuild_type_cutscene: bool,
    pub fade: Fade,
    /// The loaded cutscene id and its loading stage.
    pub loaded_id: i32,
    pub stage: Option<Stage>,
    pub definition: Option<Definition>,
    pub entities: Vec<EntityState>,
    pub players: crate::protocol910::Players,
    pub npcs: crate::protocol910::npc::Npcs,
    pub locations: Vec<LocationState>,
    /// Var overrides while the cutscene plays (varbit keys carry `1 << 32`).
    pub var_overrides: BTreeMap<i64, i32>,
    pub draw_priority_counter: i32,
    /// The camera state was saved (the values live with the UI owner).
    pub camera_state_saved: bool,
    pub requests: Vec<UiRequest>,
    /// The cutscene defaults' cancel binding.
    pub cancel_binding: Option<crate::ui_defaults::Binding>,
    defaults_loaded: bool,
    /// Set by the UI when `cancelbinding.test` held this update.
    pub cancel_requested: bool,
    /// The cutscenes and defaults JS5 handle (cached archive indexes).
    pack: Option<crate::cache::Pack>,
}
impl Default for Manager {
    fn default() -> Self {
        Self {
            scene_state: 3,
            client_id: -1,
            camera_active: false,
            start_cycle: 0,
            next_action: 0,
            appearance: None,
            map_capacity: 0,
            rebuild_type_cutscene: false,
            fade: Fade::default(),
            loaded_id: -1,
            stage: None,
            definition: None,
            entities: Vec::new(),
            players: crate::protocol910::Players::default(),
            npcs: crate::protocol910::npc::Npcs::default(),
            locations: Vec::new(),
            var_overrides: BTreeMap::new(),
            draw_priority_counter: 1,
            camera_state_saved: false,
            requests: Vec::new(),
            cancel_binding: None,
            defaults_loaded: false,
            cancel_requested: false,
            pack: None,
        }
    }
}

/// Decode the cutscene defaults from the
/// `defaults` archive's cutscene group (8) file.
pub fn decode_defaults(bytes: &[u8]) -> Result<Option<crate::ui_defaults::Binding>> {
    let mut p = Cursor::new(bytes);
    let mut binding = None;
    loop {
        match p.g1()? {
            0 => return Ok(binding),
            1 => binding = crate::ui_defaults::Binding::decode_from(&mut p)?,
            _ => {}
        }
    }
}

impl Manager {
    /// Map a cutscene entity index onto a `Players` slot that never aliases
    /// the local player slot (the renderer's local-player owner).
    pub fn player_slot(index: usize, local: usize) -> usize {
        if index < local {
            index
        } else {
            index + 1
        }
    }
    /// The `CUTSCENE` packet.
    pub fn begin(&mut self, id: u16, parameter: u16, appearance: &[u8]) {
        self.start_cycle = -1;
        self.client_id = i32::from(id);
        self.scene_state = 2;
        self.requests.push(UiRequest::CloseMenu);
        self.map_capacity = i32::from(parameter);
        self.appearance = Some(appearance.to_vec());
    }
    /// Reset the cutscene (logout).
    pub fn reset_cutscene(&mut self) {
        self.client_id = -1;
        self.scene_state = 3;
        self.appearance = None;
        self.reset();
    }
    /// Reset the manager.
    pub fn reset(&mut self) {
        self.var_overrides.clear();
        self.definition = None;
        self.entities.clear();
        self.locations.clear();
        self.players = crate::protocol910::Players::default();
        self.npcs = crate::protocol910::npc::Npcs::default();
        self.draw_priority_counter = 1;
        self.stage = None;
        self.loaded_id = -1;
        if self.camera_state_saved {
            self.requests.push(UiRequest::RestoreState);
            self.camera_state_saved = false;
        }
    }
    /// Load the cutscene. The Rust client
    /// reads the local pack synchronously, so the `WAIT_MODELS` readiness of
    /// models/frames is satisfied as soon as the file decodes; `SoundSong`
    /// still issues its `preloadSong` side effect and sounds are created in
    /// decode order.
    pub fn load(&mut self, pack: &crate::cache::Pack, id: i32) -> Result<bool> {
        if self.loaded_id != id || self.stage.is_none() {
            self.reset();
            self.stage = Some(Stage::ReadFile);
            self.loaded_id = id;
        }
        if self.stage == Some(Stage::ReadFile) {
            let def = Definition::load(pack, id)?;
            for (tag, action) in def.actions.iter().enumerate() {
                if let ActionKind::Sound31 {
                    sound,
                    volume,
                    rate,
                    repeats,
                }
                | ActionKind::SoundVorbis {
                    sound,
                    volume,
                    rate,
                    repeats,
                } = action.kind
                {
                    self.requests.push(UiRequest::SoundCreate {
                        tag,
                        sound,
                        repeats,
                        volume,
                        rate,
                    });
                }
            }
            self.entities = vec![EntityState::default(); def.entities.len()];
            self.locations = vec![LocationState::default(); def.locations.len()];
            self.definition = Some(def);
            self.stage = Some(Stage::WaitModels);
        }
        if self.stage == Some(Stage::WaitModels) {
            let def = self.definition.as_ref().context("cutscene definition")?;
            for action in &def.actions {
                if let ActionKind::SoundSong { song, volume } = action.kind {
                    self.requests
                        .push(UiRequest::PreloadSong { id: song, volume });
                }
            }
            self.stage = Some(Stage::Ready);
        }
        Ok(true)
    }
    /// Save the viewport limits and song.
    pub fn save_state(&mut self) {
        self.camera_state_saved = true;
        let (w, h) = self
            .definition
            .as_ref()
            .map_or((0, 0), |d| (d.viewport_width, d.viewport_height));
        self.requests
            .push(UiRequest::SaveState { viewport: [w, h] });
    }
    /// Finish the cutscene.
    pub fn finish(&mut self, completed: bool) {
        if self.scene_state == 4 || self.scene_state == 3 {
            return;
        }
        if !completed {
            if let Some(def) = &self.definition {
                for (tag, action) in def.actions.iter().enumerate() {
                    if matches!(
                        action.kind,
                        ActionKind::Sound31 { .. } | ActionKind::SoundVorbis { .. }
                    ) {
                        self.requests.push(UiRequest::SoundCleanup { tag });
                    }
                }
            }
            self.requests.push(UiRequest::RestoreSong);
        }
        self.scene_state = 4;
        self.appearance = None;
        self.camera_active = false;
        if self.loaded_id > 0 {
            self.requests.push(UiRequest::End {
                cutscene: self.loaded_id,
            });
        }
        self.reset();
        self.requests.push(UiRequest::Finished { completed });
    }
    /// Take this update's requests for the UI owner.
    pub fn take_requests(&mut self) -> Vec<UiRequest> {
        std::mem::take(&mut self.requests)
    }
    /// Load the cutscene defaults once.
    pub fn ensure_defaults(&mut self, pack: &crate::cache::Pack) -> Result<()> {
        if self.defaults_loaded {
            return Ok(());
        }
        self.defaults_loaded = true;
        if let Some(bytes) = crate::js5_fetch::fetch_file(pack, "defaults", 8)? {
            self.cancel_binding = decode_defaults(&bytes)?;
        }
        Ok(())
    }
    /// The actor behind cutscene entity `index`.
    pub fn entity_mut(
        &mut self,
        index: usize,
        local: usize,
    ) -> Result<&mut crate::entities910::Player> {
        let npc = self
            .definition
            .as_ref()
            .and_then(|d| d.entities.get(index))
            .with_context(|| {
                Fault::IndexOutOfRange.message(format_args!("cutscene entity {index}"))
            })?
            .npc_id
            >= 0;
        if npc {
            self.npcs.entities.get_mut(&index).map(|n| &mut n.path)
        } else {
            self.players
                .players
                .get_mut(Self::player_slot(index, local))
                .and_then(Option::as_mut)
        }
        .with_context(|| {
            Fault::MissingValue.message(format_args!("cutscene entity {index} does not exist"))
        })
    }
    /// The actor behind cutscene entity `index`:
    /// `None` for an index outside the definition or an entity that does not
    /// exist.
    pub fn entity(&self, index: usize, local: usize) -> Option<&crate::entities910::Player> {
        let npc = self.definition.as_ref()?.entities.get(index)?.npc_id >= 0;
        if npc {
            self.npcs.entities.get(&index).map(|n| &n.path)
        } else {
            self.players
                .players
                .get(Self::player_slot(index, local))
                .and_then(Option::as_ref)
        }
    }
    /// Existing cutscene entities in array order, with their actor key.
    pub fn existing(&self, local: usize) -> Vec<(usize, bool, usize)> {
        let Some(def) = &self.definition else {
            return Vec::new();
        };
        def.entities
            .iter()
            .zip(&self.entities)
            .filter(|(_, s)| s.exists)
            .map(|(d, _)| {
                if d.npc_id >= 0 {
                    (d.index, true, d.index)
                } else {
                    (d.index, false, Self::player_slot(d.index, local))
                }
            })
            .collect()
    }
}

/// The world transaction of a cutscene map rebuild: the standard build area
/// with the map size shifted into chunks, entering rebuild state 3.
pub fn cutscene_world(
    def: &Definition,
    prior: &crate::protocol910::rebuild_state::World,
    capacity: i32,
    land_groups: &std::collections::BTreeSet<i32>,
) -> Result<(
    crate::protocol910::rebuild_state::World,
    crate::protocol910::rebuild_state::RegionLayout,
)> {
    use crate::protocol910::rebuild_state::{Kind, RegionLayout};
    // The standard build area size.
    let size = crate::protocol910::rebuild_state::area_size(0)
        .map_err(|e| anyhow::anyhow!("build area: {e:?}"))?;
    let templates = def.region_templates(size)?;
    let squares = def.map_squares(capacity, land_groups)?;
    let mut w = prior.clone();
    w.last_kind = Kind::Cutscene;
    w.width = size;
    w.height = size;
    w.area = Some(0);
    w.region_x = size >> 4;
    w.region_z = size >> 4;
    w.base_x = 0;
    w.base_z = 0;
    w.map_squares = squares.clone();
    w.groups = squares
        .iter()
        .map(|s| (s >> 8) | ((s & 0xFF) << 7))
        .collect();
    w.group_count = squares.len();
    let chunks = usize::try_from(size >> 3)?;
    Ok((
        w,
        RegionLayout {
            chunks_x: chunks,
            chunks_z: chunks,
            templates,
        },
    ))
}

fn protocol(e: crate::protocol910::Error) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

/// Turn the actor to `angle`, instantly.
fn turn_to(e: &mut crate::entities910::Player, angle: i32) {
    e.desired_angle = angle & 0x3FFF;
    e.angle = e.desired_angle;
    e.motion.yaw_velocity = 0;
}

impl crate::game_runtime::Game {
    /// The game update. With `sceneState == 3` the normal
    /// players/NPCs update; otherwise the cancel binding, the cutscene load
    /// and map build, the action clock and the cutscene entities run instead.
    /// Returns `true` when this update requested the cutscene map build.
    /// `textures` is `Preferences.textures` (read by the caller, client_game).
    pub fn update_scene_state(
        &mut self,
        pack: &crate::cache::Pack,
        textures: bool,
    ) -> Result<bool> {
        let cancel = std::mem::take(&mut self.cutscene.cancel_requested);
        if self.cutscene.scene_state == 3 {
            self.update_actors().map_err(protocol)?;
            return Ok(false);
        }
        if self.runtime.map_request.is_some() || !self.runtime.feed.state.initialized {
            return Ok(false);
        }
        let pack = self
            .cutscene
            .pack
            .get_or_insert_with(|| pack.clone())
            .clone();
        let pack = &pack;
        self.cutscene.ensure_defaults(pack)?;
        let mut requested = false;
        if cancel {
            self.cutscene.finish(false);
        } else {
            if self.cutscene.scene_state == 2
                && self.cutscene.load(pack, self.cutscene.client_id)?
            {
                requested = self.request_cutscene_map()?;
                self.cutscene.scene_state = 1;
            }
            // `state != 3`: the requested map is built and installed.
            if self.cutscene.scene_state == 1 && self.runtime.map_request.is_none() {
                self.cutscene.var_overrides.clear();
                self.cutscene.scene_state = 0;
                self.cutscene.start_cycle = self.cycle;
                self.cutscene.next_action = 0;
                self.cutscene.camera_active = false;
                self.cutscene.save_state();
            }
            if self.cutscene.scene_state == 0 {
                let elapsed = self.cycle.wrapping_sub(self.cutscene.start_cycle);
                while let Some(action) = self
                    .cutscene
                    .definition
                    .as_ref()
                    .and_then(|d| d.actions.get(self.cutscene.next_action))
                    .cloned()
                {
                    if action.start_tick > elapsed {
                        break;
                    }
                    let tag = self.cutscene.next_action;
                    if crate::game_debug_flags::flags().cutscene_trace {
                        log::info!(
                            "[cutscene] cycle {} tick {elapsed} action {tag} {:?}",
                            self.cycle,
                            action.kind
                        );
                    }
                    self.execute_cutscene_action(tag, action.kind, textures)?;
                    if self.cutscene.scene_state != 0 {
                        break;
                    }
                    self.cutscene.next_action += 1;
                }
                if self.cutscene.scene_state == 0 {
                    self.update_cutscene_entities()?;
                }
            }
        }
        // Chat expiry keeps running for the normal
        // actors' chat lines while their movement is suspended.
        let state = &mut self.runtime.feed.state;
        for &id in &state.players.high_indices {
            if let Some(chat) = state.players.players[id]
                .as_mut()
                .and_then(|p| p.chat.as_mut())
            {
                chat.tick();
            }
        }
        for id in &state.npcs.slots {
            if let Some(chat) = state
                .npcs
                .entities
                .get_mut(id)
                .and_then(|n| n.path.chat.as_mut())
            {
                chat.tick();
            }
        }
        Ok(requested)
    }

    /// The cutscene map rebuild request, followed by the rebuild transaction
    /// whose mode-3 rebase keeps and shifts every NPC.
    fn request_cutscene_map(&mut self) -> Result<bool> {
        let def = self
            .cutscene
            .definition
            .as_ref()
            .context("cutscene definition")?;
        let prior = self.runtime.installed_world.clone();
        let (world, layout) = cutscene_world(
            def,
            &prior,
            self.cutscene.map_capacity,
            &self.inputs.land_groups,
        )?;
        self.cutscene.rebuild_type_cutscene = true;
        // An unchanged region keeps the current scene.
        if (prior.region_x, prior.region_z) == (world.region_x, world.region_z)
            && prior.area == world.area
        {
            self.runtime.installed_world = world.clone();
            self.runtime.feed.state.world = Some(world);
            return Ok(false);
        }
        let scene =
            self.runtime
                .terrain
                .as_ref()
                .map(|t| crate::protocol910::rebuild_state::SceneBounds {
                    width: t.width as i32,
                    height: t.height as i32,
                    level_tiles: true,
                });
        let mut effects = crate::protocol910::rebuild_state::rebase(
            &mut self.runtime.feed.state,
            &crate::protocol910::rebuild_state::Rebase {
                old_x: prior.base_x,
                old_z: prior.base_z,
                base_x: world.base_x,
                base_z: world.base_z,
                width: world.width,
                height: world.height,
                mode: 3,
                preserve_outside: false,
                loc_sizes: &self.inputs.locs.sizes,
                scene,
            },
            crate::protocol910::rebuild_state::Effects {
                region: Some(layout),
                ..Default::default()
            },
        )
        .map_err(protocol)?;
        effects.resized = prior.area != world.area;
        if effects.mode == 3 {
            self.camera.rebase(effects.delta_x, effects.delta_z);
        }
        self.pending_refresh.clear();
        self.pending_loc_animations.clear();
        self.pending_actor_sounds.clear();
        self.runtime.feed.state.world = Some(world.clone());
        self.runtime.request_map(world, effects);
        Ok(true)
    }

    fn cutscene_npc_type(&self, id: i32) -> Result<&crate::protocol910::npc::NpcType> {
        self.inputs
            .npc_types
            .get(&id)
            .with_context(|| format!("cutscene NPC type {id}"))
    }

    /// The actor behind cutscene entity `index`.
    fn cutscene_entity(&mut self, index: usize) -> Result<&mut crate::entities910::Player> {
        let local = self.runtime.map.local;
        self.cutscene.entity_mut(index, local)
    }

    /// Move a cutscene entity to a tile, facing a direction.
    fn cutscene_move_to_facing(
        &mut self,
        index: usize,
        x: i32,
        z: i32,
        level: i32,
        angle: i32,
        textures: bool,
    ) -> Result<()> {
        let def = self
            .cutscene
            .definition
            .as_ref()
            .and_then(|d| d.entities.get(index))
            .cloned()
            .with_context(|| {
                Fault::IndexOutOfRange.message(format_args!("cutscene entity {index}"))
            })?;
        let local = self.runtime.map.local;
        let exists = self.cutscene.entities[index].exists;
        if !exists {
            self.cutscene.entities[index].exists = true;
            self.cutscene.draw_priority_counter += 1;
            self.cutscene.entities[index].draw_priority = self.cutscene.draw_priority_counter - 1;
            if def.npc_id >= 0 {
                // A new NPC draws four random values for its recolours (as
                // when NPCs are added by the NPC info packet);
                // `textures` is `Preferences.textures`.
                let r: [f64; 4] = std::array::from_fn(|_| self.random.next());
                let mut npc = crate::entities910::Npc::new([
                    (r[0] * 4.) as i32 + 32,
                    (r[1] * 2.) as i32 + 3,
                    (r[2] * 3.) as i32 + 16,
                    (r[3] * if textures { 6. } else { 12. }) as i32,
                ]);
                let t = self.cutscene_npc_type(def.npc_id)?;
                npc.head_icons = crate::protocol910::npc::head_icons(t.head_icons.as_deref());
                npc.covermarker = t.covermarker;
                npc.type_id = def.npc_id;
                npc.name = t.name.clone();
                npc.vislevel = t.vislevel;
                npc.path.size = t.size;
                npc.turn_speed = t.turnspeed.wrapping_shl(3);
                npc.update_serial = self.cycle;
                self.cutscene.npcs.entities.insert(index, npc);
                self.cutscene.npcs.slots.push(index);
                self.cutscene.npcs.slots.sort_unstable();
            } else {
                let appearance = self.cutscene.appearance.clone().with_context(|| {
                    Fault::MissingValue.message("local player appearance for the cutscene")
                })?;
                let mut player = crate::entities910::Player::default();
                let mut cache = crate::entities910::appearance::CachedPacket {
                    data: appearance,
                    consumed: 0,
                };
                crate::protocol910::appearance::apply(
                    &mut cache,
                    &mut player,
                    &self.inputs.appearance.appearance,
                )
                .map_err(protocol)?;
                let slot = crate::cutscene::Manager::player_slot(index, local);
                let players = &mut self.cutscene.players;
                anyhow::ensure!(slot < players.players.len(), "cutscene player slot {slot}");
                players.players[slot] = Some(player);
                players.appearances[slot] = Some(cache);
                players.generations[slot] = players.generations[slot].wrapping_add(1);
                players.high_indices.push(slot);
                players.high_indices.sort_unstable();
            }
        }
        if def.npc_id >= 0 {
            self.cutscene_move_to(index, level, x, z)?;
            turn_to(self.cutscene_entity(index)?, angle);
        } else {
            let e = self.cutscene_entity(index)?;
            e.level = level as i8 as i32;
            e.occlude_level = e.level;
            e.tele(x, z);
            turn_to(e, angle);
        }
        Ok(())
    }

    /// Move a cutscene entity: an NPC move with the teleport flag or a player
    /// teleport.
    fn cutscene_move_to(&mut self, index: usize, level: i32, x: i32, z: i32) -> Result<()> {
        let bridge = self.runtime.map.bridges.contains(&(x, z));
        let npc = self
            .cutscene
            .definition
            .as_ref()
            .and_then(|d| d.entities.get(index))
            .is_some_and(|d| d.npc_id >= 0);
        let e = self.cutscene_entity(index)?;
        if npc {
            e.level = level as i8 as i32;
            e.occlude_level = e.level + i32::from(bridge);
            e.animation.cancel_for_move();
            e.face_override = -1;
            e.tele(x, z);
        } else {
            e.level = level as i8 as i32;
            e.occlude_level = e.level;
            e.tele(x, z);
        }
        Ok(())
    }

    /// Start a cutscene route.
    fn cutscene_start_route(&mut self, index: usize, route: usize, level: i32) -> Result<()> {
        let route = self
            .cutscene
            .definition
            .as_ref()
            .and_then(|d| d.routes.get(route))
            .cloned()
            .with_context(|| {
                Fault::IndexOutOfRange.message(format_args!("cutscene route {route}"))
            })?;
        let first = *route
            .waypoints
            .first()
            .with_context(|| Fault::IndexOutOfRange.message("first waypoint of the route"))?;
        self.cutscene_move_to(index, level, ((first as u32) >> 16) as i32, first & 0xFFFF)?;
        let e = self.cutscene_entity(index)?;
        e.route_length = 0;
        for (&speed, &waypoint) in route.speeds.iter().zip(&route.waypoints).rev() {
            anyhow::ensure!(
                e.route_length < e.x.len(),
                "{}",
                Fault::IndexOutOfRange.message(format_args!("route waypoint {}", e.route_length))
            );
            e.x[e.route_length] = waypoint >> 16;
            e.z[e.route_length] = waypoint & 0xFFFF;
            // Move speed serial ids: crawl 0, walk 1, run 2.
            e.speeds[e.route_length] = match speed {
                0 => 0,
                2 => 2,
                _ => 1,
            };
            e.route_length += 1;
        }
        Ok(())
    }

    /// A loc placement or removal for a cutscene location (`place` at the
    /// tile with the state's angle, or remove with id -1), through the same
    /// retained request list as `LOC_ADD_CHANGE`/`LOC_DEL`.
    fn cutscene_loc_update(&mut self, place: LocationState, layer: i32, id: i32, shape: i32) {
        let LocationState { level, x, z, angle } = place;
        let zones = &mut self.runtime.feed.state.zones;
        let at = if let Some(i) = zones
            .locations
            .iter()
            .position(|r| r.level == level && r.x == x && r.z == z && r.layer == layer)
        {
            i
        } else {
            let mut r = crate::protocol910::zone_state::Location {
                level,
                layer,
                x,
                z,
                old_id: -1,
                old_shape: 0,
                old_angle: 0,
                id: 0,
                shape: 0,
                angle: 0,
                transform: None,
                custom: None,
                pending: true,
                remove: false,
            };
            if let Some(old) = self.scene_locs.get(&(level, layer, x, z)) {
                r.old_id = old.id;
                r.old_shape = old.shape;
                r.old_angle = old.angle;
                r.transform = old.transform;
            }
            zones.locations.push(r);
            zones.locations.len() - 1
        };
        let r = &mut zones.locations[at];
        r.id = id;
        r.shape = shape;
        r.angle = angle;
        r.pending = true;
        r.remove = false;
    }

    /// Execute one cutscene action of any kind.
    fn execute_cutscene_action(
        &mut self,
        tag: usize,
        kind: ActionKind,
        textures: bool,
    ) -> Result<()> {
        let height = |game: &Self, x: i32, z: i32, level: i32| {
            crate::protocol910::terrain::height(game.runtime.terrain.as_ref(), x, z, level)
                .map_err(protocol)
        };
        match kind {
            // Entity hitmark: add a hitmark to the entity.
            ActionKind::EntityHitmark {
                entity,
                hitmark,
                damage,
                secondary,
                secondary_damage,
            } => {
                let cycle = self.cycle;
                let local = self.runtime.map.local;
                let config = &self.inputs.combat;
                let e = self.cutscene.entity_mut(entity, local)?;
                let combat = e
                    .combat
                    .get_or_insert_with(|| crate::entities910::combat::Combat::new(config.slots));
                crate::protocol910::combat::hit(
                    combat,
                    config,
                    [hitmark, damage, secondary, secondary_damage],
                    cycle,
                    0,
                )
                .map_err(protocol)?;
            }
            ActionKind::EntityMove {
                entity,
                x,
                z,
                level,
                angle,
            } => self.cutscene_move_to_facing(entity, x, z, level, angle, textures)?,
            ActionKind::Finish => self.cutscene.finish(true),
            ActionKind::Sound31 { .. } | ActionKind::SoundVorbis { .. } => {
                self.cutscene.requests.push(UiRequest::SoundStart { tag });
            }
            ActionKind::SetVar { key, value } => {
                self.cutscene.var_overrides.insert(key, value);
            }
            ActionKind::SoundJingle { jingle, volume } => {
                self.cutscene
                    .requests
                    .push(UiRequest::Jingle { id: jingle, volume });
            }
            ActionKind::EntityRoute {
                entity,
                route,
                level,
            } => self.cutscene_start_route(entity, route, level)?,
            // Text coord: add a world-space text label.
            ActionKind::TextCoord {
                x,
                z,
                text,
                colour,
                duration,
            } => {
                let level = self.runtime.feed.state.players.current_level;
                let y = height(self, x, z, level)?;
                self.runtime.feed.state.zones.text_coords.push(
                    crate::protocol910::zone_state::TextCoord {
                        level,
                        x,
                        z,
                        height: y,
                        expiry_cycle: self.cycle.wrapping_add(duration),
                        colour,
                        text,
                    },
                );
            }
            ActionKind::EntityLook { entity, direction } => {
                turn_to(self.cutscene_entity(entity)?, direction);
            }
            // Projectile animation action.
            ActionKind::ProjAnim {
                level,
                src_entity,
                src_x,
                src_z,
                dest_entity,
                dest_x,
                dest_z,
                spot,
                start_height,
                target_height,
                duration,
                peak_pitch,
                arc,
            } => {
                let (sx, sz, mut lvl) = if src_x >= 0 {
                    (src_x * 512 + 256, src_z * 512 + 256, level)
                } else {
                    let e = self.cutscene_entity(src_entity as usize)?;
                    (e.fine_x as i32, e.fine_z as i32, e.level)
                };
                // The source z is tested for `>= 0` here.
                let (dx, dz) = if src_z >= 0 {
                    (dest_x * 512 + 256, dest_z * 512 + 256)
                } else {
                    let e = self.cutscene_entity(dest_entity as usize)?;
                    if lvl < 0 {
                        lvl = e.level;
                    }
                    (e.fine_x as i32, e.fine_z as i32)
                };
                let effect = self
                    .inputs
                    .selection
                    .effects
                    .get(&spot)
                    .with_context(|| format!("cutscene projectile effect {spot}"))?;
                let mut node = crate::entities910::animation_state::Node::default();
                node.set(effect.sequence, 0, 0, &self.inputs.selection)
                    .map_err(protocol)?;
                let offset_start = start_height << 2;
                let y = height(self, sx, sz, lvl)?.wrapping_sub(offset_start);
                let end = duration.wrapping_add(self.cycle);
                let mut projectile = crate::entities910::transient::Projectile {
                    effect: spot,
                    level: lvl,
                    offset_start,
                    offset_end: target_height << 2,
                    start: self.cycle,
                    end,
                    pitch: peak_pitch,
                    arc: arc << 2,
                    source: src_entity + 1,
                    target: dest_entity + 1,
                    slot: 0,
                    targeted: 0,
                    follow_ground: false,
                    mobile: false,
                    position: [sx as f32, y as f32, sz as f32],
                    rotation: [0., 0., 0., 1.],
                    vx: 0.,
                    vz: 0.,
                    speed: 0.,
                    vy: 0.,
                    ay: 0.,
                    animation: node,
                };
                let terrain = self.runtime.terrain.as_ref();
                let h = |x: i32, z: i32, l: i32| {
                    crate::protocol910::terrain::height(terrain, x, z, l).unwrap_or(0)
                };
                projectile.velocity(dx, dz, target_height << 2, end, &h);
                self.runtime
                    .feed
                    .state
                    .zones
                    .transients
                    .projectiles
                    .push(projectile);
            }
            ActionKind::EntitySay {
                entity,
                text,
                colour,
                time,
            } => {
                // Set the entity's chat line.
                let e = self.cutscene_entity(entity)?;
                let chat = e.chat.get_or_insert(crate::entities910::chat::Chat {
                    text: None,
                    colour: 0,
                    effect: 0,
                    total: 0,
                    time: 0,
                });
                chat.text = Some(text);
                chat.colour = colour;
                chat.effect = 0;
                chat.time = time;
                chat.total = time;
            }
            // Entity animation: queue the sequences.
            ActionKind::EntityAnim {
                entity,
                seq,
                slot_mask,
            } => {
                let local = self.runtime.map.local;
                let config = &self.inputs.selection;
                let e = self.cutscene.entity_mut(entity, local)?;
                if slot_mask == 0 {
                    let route = e.route_length;
                    let mut steps = e.steps_remaining;
                    e.animation
                        .select_modes(vec![seq; 4], 0, false, route, &mut steps, config)
                        .map_err(protocol)?;
                    e.steps_remaining = steps;
                } else {
                    e.animation
                        .overlay(seq, 0, slot_mask, config)
                        .map_err(protocol)?;
                }
            }
            ActionKind::EntityDel { entity } => {
                // Destroy the cutscene entity.
                let local = self.runtime.map.local;
                let state = self.cutscene.entities.get_mut(entity).with_context(|| {
                    Fault::IndexOutOfRange.message(format_args!("cutscene entity {entity}"))
                })?;
                state.exists = false;
                self.cutscene.npcs.entities.remove(&entity);
                self.cutscene.npcs.slots.retain(|&i| i != entity);
                let slot = crate::cutscene::Manager::player_slot(entity, local);
                if let Some(p) = self.cutscene.players.players.get_mut(slot) {
                    *p = None;
                }
                self.cutscene.players.high_indices.retain(|&i| i != slot);
            }
            ActionKind::LocCreate {
                location,
                x,
                z,
                level,
                angle,
            } => {
                let def = *self
                    .cutscene
                    .definition
                    .as_ref()
                    .and_then(|d| d.locations.get(location))
                    .with_context(|| {
                        Fault::IndexOutOfRange.message(format_args!("cutscene location {location}"))
                    })?;
                self.cutscene_loc_update(
                    LocationState { level, x, z, angle },
                    def.layer,
                    def.loc_id,
                    def.shape,
                );
                self.cutscene.locations[location] = LocationState { level, x, z, angle };
            }
            ActionKind::Fade { duration, colour } => {
                self.cutscene.fade.begin(self.cycle, duration, colour);
            }
            ActionKind::LocDel { location } => {
                let def = *self
                    .cutscene
                    .definition
                    .as_ref()
                    .and_then(|d| d.locations.get(location))
                    .with_context(|| {
                        Fault::IndexOutOfRange.message(format_args!("cutscene location {location}"))
                    })?;
                let s = self.cutscene.locations[location];
                self.cutscene_loc_update(s, def.layer, -1, def.shape);
            }
            // Loc animation: play a loc animation.
            ActionKind::LocAnim { location, seq } => {
                let def = *self
                    .cutscene
                    .definition
                    .as_ref()
                    .and_then(|d| d.locations.get(location))
                    .with_context(|| {
                        Fault::IndexOutOfRange.message(format_args!("cutscene location {location}"))
                    })?;
                let s = self.cutscene.locations[location];
                let map = &self.runtime.map;
                if s.x >= 0 && s.z >= 0 && s.x < map.width - 1 && s.z < map.height - 1 {
                    self.pending_loc_animations.push_back(
                        crate::protocol910::zone_state::LocAnimation {
                            level: s.level,
                            x: s.x,
                            z: s.z,
                            layer: def.layer,
                            shape: def.shape,
                            angle: s.angle,
                            transform: None,
                            sequence: seq,
                            delay: 0,
                        },
                    );
                }
            }
            ActionKind::CamMove {
                x,
                height,
                z,
                pitch,
                yaw,
            } => {
                // Camera move action.
                self.cutscene.requests.push(UiRequest::CameraMoveTo {
                    x,
                    z,
                    height,
                    acceleration: 100,
                    speed: 100,
                    instant: false,
                });
                self.cutscene.requests.push(UiRequest::CameraForceAngle {
                    pitch,
                    yaw,
                    roll: 0,
                });
                self.cutscene.camera_active = true;
            }
            ActionKind::EntitySpot {
                spot,
                orientation,
                height,
                entity,
                slot,
                delay,
            } => {
                let local = self.runtime.map.local;
                let config = &self.inputs.selection;
                let e = self.cutscene.entity_mut(entity, local)?;
                anyhow::ensure!(
                    slot < e.animation.spots.len(),
                    "{}",
                    Fault::IndexOutOfRange.message(format_args!("spot animation slot {slot}"))
                );
                e.animation
                    .spot(
                        slot,
                        crate::entities910::animation_state::SpotRequest {
                            id: spot,
                            param: height << 16,
                            orientation,
                            delay,
                            flag: false,
                        },
                        config,
                    )
                    .map_err(protocol)?;
            }
            ActionKind::SoundSong { song, .. } => {
                self.cutscene.requests.push(UiRequest::Song { id: song });
            }
            // Map animation action.
            ActionKind::MapAnim {
                spot,
                orientation,
                height: offset,
                tile_x,
                tile_z,
                level,
            } => {
                let x = tile_x * 512 + 256;
                let z = tile_z * 512 + 256;
                let occlude = if level < 3 && self.runtime.map.bridges.contains(&(tile_x, tile_z)) {
                    level + 1
                } else {
                    level
                };
                let effect = self
                    .inputs
                    .selection
                    .effects
                    .get(&spot)
                    .with_context(|| format!("cutscene map spot effect {spot}"))?
                    .clone();
                let animation = if effect.sequence == -1 {
                    None
                } else {
                    let mut node = crate::entities910::animation_state::Node::default();
                    node.set(
                        effect.sequence,
                        0,
                        if effect.looping { 0 } else { 2 },
                        &self.inputs.selection,
                    )
                    .map_err(protocol)?;
                    Some(node)
                };
                let y = height(self, x, z, level)?.wrapping_sub(offset);
                self.runtime.feed.state.zones.transients.spots.push(
                    crate::entities910::transient::Spot {
                        key: i64::from(tile_x << 16 | tile_z),
                        effect: spot,
                        level,
                        occlude,
                        position: [x as f32, y as f32, z as f32],
                        orientation,
                        targeted: 0,
                        animation,
                    },
                );
            }
            ActionKind::CamMoveAlong {
                pos_spline,
                target_spline,
                pos_keyframe,
                target_keyframe,
                min_speed,
                max_speed,
            } => {
                let def = self.cutscene.definition.as_ref().context("definition")?;
                let spline = |i: usize| {
                    def.splines.get(i).map(Spline::rows).with_context(|| {
                        Fault::IndexOutOfRange.message(format_args!("cutscene spline {i}"))
                    })
                };
                let splines = [spline(pos_spline)?, spline(target_spline)?];
                self.cutscene.requests.push(UiRequest::CameraMoveAlong {
                    splines,
                    pos_keyframe,
                    target_keyframe,
                    min_speed,
                    max_speed,
                });
                self.cutscene.camera_active = true;
            }
            ActionKind::Subtitle { text, subtitle } => {
                if self.cutscene.client_id != -1 {
                    self.cutscene.requests.push(UiRequest::Subtitle {
                        cutscene: self.cutscene.client_id,
                        text,
                        subtitle,
                    });
                }
            }
        }
        Ok(())
    }

    /// The logic pass for every existing cutscene entity in array order, then
    /// their chat expiry.
    fn update_cutscene_entities(&mut self) -> Result<()> {
        let order = self.cutscene.existing(self.runtime.map.local);
        let varps = self.runtime.feed.state.varps.as_ref();
        let bits = &self.inputs.bits;
        // Multi-NPC resolution reads the local player's variable state.
        let vars = |bit: bool, id: i32| -> Option<i32> {
            if bit {
                let definition = bits.get(id, false).ok()?;
                varps?.get_bit(&definition).ok()
            } else {
                varps?.get(id).ok()
            }
        };
        let c = crate::actor::Inputs {
            selection: &self.inputs.selection,
            sequences: &self.inputs.animation.sequences.sequences,
            bases: &self.inputs.bas,
            npcs: &self.inputs.appearance.types.npcs,
            vars: Some(&vars),
        };
        let mut sounds = Vec::new();
        let local = self.runtime.map.local;
        let listener = self
            .runtime
            .feed
            .state
            .players
            .players
            .get(local)
            .and_then(Option::as_ref)
            .map(|p| (p.level, -(local as i32) - 1));
        crate::actor::tick_cutscene(
            &mut self.cutscene.players,
            &mut self.cutscene.npcs,
            &order,
            &crate::actor::TickContext {
                inputs: &c,
                map: &self.runtime.map,
                scene: self.runtime.terrain.as_ref(),
                cycle: self.cycle,
            },
            &mut crate::actor::TickOutput {
                rng: &mut self.random,
                sounds: &mut sounds,
            },
            listener,
        )
        .map_err(protocol)?;
        self.pending_actor_sounds.extend(sounds);
        Ok(())
    }
}
