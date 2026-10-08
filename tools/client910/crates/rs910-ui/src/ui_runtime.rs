//! Live retained UI owner. read executes lifecycle hooks at the packet
//! boundary; updateInterfaces and updateGame poll delayed
//! changes before draining hooks. The game lends its authoritative variables.
use crate::{
    cache::Pack,
    ui_components::Store,
    ui_draw::Frame,
    ui_hook_host::{Domains, Execution, Runner},
    ui_hooks::Pool,
    ui_properties::State,
    ui_scripts::{Provider, Scripts},
    ui_vars::Variables,
};
use anyhow::Result;
use native910::{
    vars::VarScope,
    vm::{Host, InstructionContext, Value, VmError, VmResult},
};
use std::collections::BTreeMap;
#[path = "ui_host_game.rs"]
pub mod host_game;

/// `Engine` and its owned sub-states (engine.rs).
mod engine;
pub use engine::*;
/// The engine-command table and its per-family handlers (commands/*.rs).
mod commands;
/// The `MiniMenu` builder and click path (menu_builder.rs).
mod menu_builder;
#[cfg(test)]
use menu_builder::{npc_menu_name, npc_menu_operation_slots, npc_type_ops};
/// `CacheConfigs` and its install (cache_configs.rs).
mod cache_configs;
/// The world-map owner calls (world_map.rs).
mod world_map;
pub use cache_configs::CacheConfigs;
/// The server-packet router and its per-family appliers (packets/*.rs).
mod packets;

/// Engine commands without a richer owner (ui_host_builtins.rs).
#[path = "ui_host_builtins.rs"]
pub mod host_builtins;

/// Credentials requested by the original client's login/lobby scripts. The app session owner
/// consumes this after the retained VM tick and starts the existing
/// non-blocking login worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub new_auth_preference: String,
    pub auth_dont_trust: bool,
    pub lobby: bool,
    /// The social network the player picked; the login then carries no
    /// username or password.
    pub sso: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LobbyEnterGameRequest {
    pub new_auth_preference: String,
    pub auth_dont_trust: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveChatPhrase {
    pub id: u16,
    pub dynamics: Vec<i32>,
}

/// `img` for a wire id; `None` is
/// `img == -1` (NONE) or an id `decode` does not know.
pub(crate) fn crown_image(crown: i32) -> Option<i32> {
    match crown {
        1 => Some(0),
        2 => Some(1),
        3 => Some(8),
        4 => Some(9),
        5 => Some(10),
        6 => Some(11),
        7 => Some(12),
        8 => Some(13),
        _ => None,
    }
}

/// One `addMessage(text, colour, effect)` call
/// (-> setChatLine).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverheadChat {
    pub player: usize,
    pub text: String,
    pub colour: i32,
    pub effect: i32,
}

/// A cover-marker click box: the renderer publishes the screen rectangle
/// and the normal scene-menu tick consumes it before ground picking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverMarker {
    pub npc: usize,
    pub rect: [i32; 4],
}

/// The arguments of `addChatLine`
/// minus its flags, for [`Engine::add_crowned_line`].
pub(crate) struct CrownedLine<'a> {
    pub chat_type: i32,
    pub name: &'a str,
    pub name_unfiltered: &'a str,
    pub name_simple: String,
    pub clan: Option<String>,
    pub phrase: i32,
    pub message: String,
    pub crown: u8,
}

/// Moved to audio_runtime.rs, the owner that consumes it (Phase 2.5).
pub use crate::audio_runtime::SoundRequest;

/// `StockmarketSlot`: one retained offer row from the three-by-eight
/// client stockmarket table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StockmarketSlot {
    pub state: u8,
    pub object: i32,
    pub price: i32,
    pub count: i32,
    pub completed_count: i32,
    pub completed_gold: i32,
}
impl StockmarketSlot {
    fn offer_type(self) -> i32 {
        i32::from((self.state & 8) != 0)
    }
    fn status(self) -> i32 {
        i32::from(self.state & 7)
    }
}

/// `HintTrail` state retained until the scene owner materializes the
/// stepped model path. Coordinates are absolute world tiles, matching the
/// packet's `g2` anchor plus signed-byte deltas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HintTrail {
    pub model: i32,
    pub points: Vec<[i32; 2]>,
}

#[derive(Clone, Debug)]
pub enum ActiveEntity {
    Player {
        #[allow(dead_code, reason = "entity slot index retained; no reader yet")]
        index: i32,
        name: String,
        /// `title` (appearance title with a `<name>` slot).
        title: Option<String>,
        chat: Option<String>,
        overlay_height: i32,
        target: i32,
        position: [f32; 3],
        screen_bounds: Option<crate::scene_player_pick::ScreenBounds>,
    },
    Npc {
        index: i32,
        type_id: i32,
        name: String,
        chat: Option<String>,
        stats: [i32; 6],
        stat_max: [i32; 6],
        vislevel: i32,
        active: bool,
        overlay_height: i32,
        target: i32,
        position: [f32; 3],
        screen_bounds: Option<crate::scene_player_pick::ScreenBounds>,
    },
    Loc {
        #[allow(dead_code, reason = "entity type id retained; no reader yet")]
        id: i32,
        overlay_height: i32,
        position: [f32; 3],
        screen_bounds: Option<crate::scene_player_pick::ScreenBounds>,
    },
    Obj {
        #[allow(dead_code, reason = "entity type id retained; no reader yet")]
        id: i32,
        overlay_height: i32,
        position: [f32; 3],
        screen_bounds: Option<crate::scene_player_pick::ScreenBounds>,
    },
}

/// Project a retained scene footprint through the same view-projection frame
/// used by player/NPC picks. The original client's screen bounding box is built from the
/// rendered graph entity; loc bindings have the authoritative config footprint
/// here, while ground objects use their one-tile scene footprint.
fn project_active_bounds(
    frame: &crate::player_picking::Frame,
    position: [f32; 3],
    half_x: f32,
    half_z: f32,
    height: f32,
) -> Option<crate::scene_player_pick::ScreenBounds> {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let mut visible = false;
    for x in [position[0] - half_x, position[0] + half_x] {
        for y in [position[1], position[1] + height] {
            for z in [position[2] - half_z, position[2] + half_z] {
                let clip = crate::ui_scene_options::transform(&frame.vp, x, y, z);
                if !clip[3].is_finite() || clip[3] <= 0.0 {
                    continue;
                }
                let point = [
                    frame.screen[0] + frame.screen[2] * clip[0] / clip[3],
                    frame.screen[1] + frame.screen[3] * clip[1] / clip[3],
                ];
                if !point.iter().all(|value| value.is_finite()) {
                    continue;
                }
                visible = true;
                min[0] = min[0].min(point[0]);
                min[1] = min[1].min(point[1]);
                max[0] = max[0].max(point[0]);
                max[1] = max[1].max(point[1]);
            }
        }
    }
    visible.then_some(crate::scene_player_pick::ScreenBounds {
        a: [min[0] as i32, min[1] as i32],
        b: [max[0] as i32, max[1] as i32],
        radius: 0,
        enabled: true,
    })
}

/// `crossX/Y`, `crossMode` (0 none, 1 walk, 2
/// interact) and `crossCycle` (+20 per logic cycle until 400).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cross {
    pub x: i32,
    pub y: i32,
    pub mode: i32,
    pub cycle: i32,
}

/// `FullscreenMode`: width, height, bit depth, refresh rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FullscreenMode {
    pub width: i32,
    pub height: i32,
    pub bit_depth: i32,
    pub refresh: i32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerOp {
    pub name: Option<String>,
    pub cursor: i32,
    pub deprioritised: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldSwitchRequest {
    pub world_id: u16,
    pub host: String,
}

impl Engine {
    /// `crown.ignorable && (dobVerified && !playerIsQuickChat ||
    /// loggedInQuickChat || ignoreTest(name))`: the free-text drop used by
    /// MESSAGE_PRIVATE/FRIENDCHANNEL/CLANCHANNEL/PLAYER_GROUP.
    fn drop_free_text(&self, crown: u8, name: &str) -> bool {
        crown != 2
            && (self.account.dob_verified && !self.account.player_is_quickchat
                || self.account.logged_in_quickchat
                || self.social.ignore_test(name))
    }

    /// `crown.ignorable && ignoreTest(name)`: the quick-chat drop.
    fn drop_quick_chat(&self, crown: u8, name: &str) -> bool {
        crown != 2 && self.social.ignore_test(name)
    }

    /// `getFullscreenModes`: keep 24-bit+ (or unknown depth)
    /// modes of at least 800x600 within the screen-size preference, the
    /// deepest per resolution, sorted by area.
    pub fn install_display_modes(&mut self, raw: &[FullscreenMode], size: i32) {
        self.platform.fullscreen_modes = Some(crate::ui_window::filter_modes(raw, size));
    }
    /// The next uniform double: two LCG steps (26 + 27 bits) over 2^53.
    fn next_double(&mut self) -> f64 {
        let mut next = |bits: u32| -> u64 {
            self.random_seed = (self
                .random_seed
                .wrapping_mul(0x5DEE_CE66D)
                .wrapping_add(0xB))
                & ((1 << 48) - 1);
            self.random_seed >> (48 - bits)
        };
        let hi = next(26);
        let lo = next(27);
        ((hi << 27) + lo) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}
fn absent(command: &str) -> VmError {
    VmError::UnknownCommand {
        command: command.into(),
    }
}

/// The chat prefix `text` starts with, as its index in `rows`, and the
/// rest of the text. The English spellings are tried first, then the launch
/// language's. Prefixes match whatever the case of the text.
fn strip_chat_prefix(text: &str, rows: &[rs910_core::texts::Msg]) -> Option<(usize, String)> {
    strip_chat_prefix_in(rs910_core::texts::client_language(), text, rows)
}

/// [`strip_chat_prefix`] for a client in `language`.
fn strip_chat_prefix_in(
    language: crate::ui_text_compare::Language,
    text: &str,
    rows: &[rs910_core::texts::Msg],
) -> Option<(usize, String)> {
    use crate::ui_text_compare::Language;
    let lower = text.to_lowercase();
    let languages =
        std::iter::once(Language::En).chain((language != Language::En).then_some(language));
    for language in languages {
        for (index, row) in rows.iter().enumerate() {
            let Some(prefix) = row.for_lang(language) else {
                continue;
            };
            if lower.starts_with(prefix) {
                // The prefix is as many characters of the text as of itself.
                let rest = text.chars().skip(prefix.chars().count()).collect();
                return Some((index, rest));
            }
        }
    }
    None
}

fn encode_pjstr(text: &str) -> Option<Vec<u8>> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.contains(&0) {
        return None;
    }
    let mut bytes = units
        .into_iter()
        .map(crate::ui_dialogue::cp1252_encode_unit)
        .collect::<Vec<_>>();
    bytes.push(0);
    Some(bytes)
}

fn truncate_utf16(text: String, units: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(units).collect::<Vec<_>>())
}

impl Engine {
    fn imported_enum_query(
        &mut self,
        output_type: i32,
        string_type: u16,
        context: &InstructionContext<'_>,
        resource: Option<&native910::execution::Resource>,
        ints: &mut Vec<i32>,
    ) -> VmResult<Option<Value>> {
        use rs910_config::{
            ui_enum_resource::Library,
            ui_enum_schema::{QueryError, Value as EnumValue},
        };
        const ARGUMENT_COUNT: usize = 4;
        const INPUT_TYPE: usize = 0;
        const OUTPUT_TYPE: usize = 1;
        const ENUM_ID: usize = 2;
        const KEY: usize = 3;
        let failed = |error: anyhow::Error| VmError::TrapFailed {
            command: context.command.into(),
            reason: format!("{error:#}"),
        };
        let start = ints
            .len()
            .checked_sub(ARGUMENT_COUNT)
            .ok_or(VmError::StackUnderflow { stack: "int" })?;
        let inputs = ints.split_off(start);
        if inputs[OUTPUT_TYPE] != output_type {
            return Err(failed(anyhow::anyhow!(
                "enum output type changed from its imported contract"
            )));
        }
        let resource = resource.ok_or_else(|| failed(anyhow::anyhow!("enum resource missing")))?;
        let library = match self.configs.imported_enums.entry(resource.digest()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Library::decode_resource(resource.bytes()).map_err(failed)?)
            }
        };
        if library.string_type != string_type {
            return Err(failed(anyhow::anyhow!(
                "enum string type changed from its imported contract"
            )));
        }
        let definition = library
            .definitions
            .get(&inputs[ENUM_ID])
            .ok_or_else(|| failed(QueryError::TypeMismatch.into()))?;
        let value = definition
            .query(
                inputs[INPUT_TYPE],
                inputs[OUTPUT_TYPE],
                inputs[KEY],
                string_type,
            )
            .map_err(|error| failed(error.into()))?;
        Ok(Some(match value {
            EnumValue::Int(value) => Value::Int(value),
            EnumValue::Text(value) => Value::Str(value),
        }))
    }
}

impl Host for Engine {
    fn trap_resource_context(
        &mut self,
        operation: native910::execution::HostOperation,
        context: &InstructionContext<'_>,
        resource: Option<&native910::execution::Resource>,
        ints: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        use native910::execution::HostOperation;
        if let HostOperation::Enum {
            output_type,
            string_type,
        } = operation
        {
            return self.imported_enum_query(output_type, string_type, context, resource, ints);
        }
        use rs910_config::ui_db_schema::{Database, FieldValue};
        const FIELD_INPUTS: usize = 3;
        const COUNT_INPUTS: usize = 2;
        const ROW_SLOT: usize = 0;
        const FIELD_SLOT: usize = 1;
        const INDEX_SLOT: usize = 2;
        let (field, arguments) = match operation {
            HostOperation::DatabaseField { field, .. } => (field, FIELD_INPUTS),
            HostOperation::DatabaseFieldCount { field } => (field, COUNT_INPUTS),
            _ => return self.trap_operation_context(operation, context, ints, objects, longs),
        };
        let failed = |error: anyhow::Error| VmError::TrapFailed {
            command: context.command.into(),
            reason: format!("{error:#}"),
        };
        let start = ints
            .len()
            .checked_sub(arguments)
            .ok_or(VmError::StackUnderflow { stack: "int" })?;
        let inputs = ints.split_off(start);
        let actual_field = inputs[FIELD_SLOT];
        if actual_field != field {
            return Err(failed(anyhow::anyhow!(
                "database field changed from its imported contract"
            )));
        }
        let resource =
            resource.ok_or_else(|| failed(anyhow::anyhow!("database resource missing")))?;
        let database = match self.configs.imported_databases.entry(resource.digest()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Database::decode_resource(resource.bytes()).map_err(failed)?)
            }
        };
        if matches!(operation, HostOperation::DatabaseFieldCount { .. }) {
            return Ok(Some(Value::Int(
                database.field_count(inputs[ROW_SLOT], field) as i32,
            )));
        }
        database
            .visit_field(inputs[ROW_SLOT], field, inputs[INDEX_SLOT], |value| {
                match value {
                    FieldValue::Int(value) => ints.push(*value),
                    FieldValue::Long(value) => longs.push(*value),
                    FieldValue::Text(value) => objects.push(Some(value.clone())),
                    FieldValue::Coordinate { .. } => {
                        anyhow::bail!("coordinate database values require tagged VM objects")
                    }
                }
                Ok(())
            })
            .map_err(failed)?;
        Ok(None)
    }
    fn trap_context(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        self.dispatch_command(c, ints, objs, longs)
    }
    fn var_get(&mut self, _: VarScope, _: u16, _: bool) -> VmResult<Value> {
        Err(absent("unbound variable"))
    }
    fn var_set(&mut self, _: VarScope, _: u16, _: bool, _: Value) -> VmResult<()> {
        Err(absent("unbound variable"))
    }
    fn varbit_get(&mut self, _: u16, _: bool) -> VmResult<i32> {
        Err(absent("unbound varbit"))
    }
    fn varbit_set(&mut self, _: u16, _: bool, _: i32) -> VmResult<()> {
        Err(absent("unbound varbit"))
    }
    fn array_define(&mut self, _: i32, _: usize) -> VmResult<()> {
        Err(absent("unbound array"))
    }
    fn array_len(&mut self, _: i32) -> VmResult<usize> {
        Err(absent("unbound array"))
    }
    fn array_get(&mut self, _: i32, _: i32) -> VmResult<i32> {
        Err(absent("unbound array"))
    }
    fn array_set(&mut self, _: i32, _: i32, _: i32) -> VmResult<()> {
        Err(absent("unbound array"))
    }
}

#[derive(Default)]
pub struct Diagnostics {
    pub hooks: usize,
    pub failures: usize,
    pub missing: BTreeMap<i32, usize>,
    /// Bounded distinct failures; no VM stacks or component graphs retained.
    pub errors: BTreeMap<String, usize>,
    pub capture: bool,
    /// Lifecycle service requests drained (see `drain_services`).
    pub services_drained: usize,
    #[cfg(any(test, feature = "test-hooks"))]
    pub executions: Vec<serde_json::Value>,
}
impl Diagnostics {
    fn record(&mut self, executions: Vec<Execution>, missing: Vec<i32>) {
        for id in missing {
            *self.missing.entry(id).or_default() += 1;
        }
        for e in executions {
            self.record(e.nested, e.missing);
            self.hooks += 1;
            #[cfg(any(test, feature = "test-hooks"))]
            if self.capture {
                self.executions.push(serde_json::json!({"id":e.id,"ok":e.result.is_ok(),"error":e.result.as_ref().err().map(ToString::to_string),"ints":e.snapshot.ints,"pc":e.snapshot.pc,"steps":e.snapshot.steps,"current_script":e.snapshot.script_id,"current_name":e.snapshot.script_name}));
            }
            if let Err(error) = &e.result {
                self.failures += 1;
                let key = format!(
                    "hook {} script {:?} pc {}: {error}",
                    e.id, e.snapshot.script_id, e.snapshot.pc
                );
                if self.errors.contains_key(&key) || self.errors.len() < 128 {
                    let count = self.errors.entry(key.clone()).or_default();
                    if *count == 0 && !self.capture {
                        log::info!("[client910] retained UI {key}");
                    }
                    *count += 1;
                }
            }
        }
    }
}

pub mod renderer_settings;

pub struct Runtime {
    pub store: Store,
    pub state: State,
    pub scripts: Scripts,
    pub pool: Pool,
    pub frame: Frame,
    pub engine: Engine,
    pub diagnostics: Diagnostics,
    pub target: crate::ui_backend::Target,
    pub renderer_settings: renderer_settings::Panel,
    /// `audioApi`: the ported audio stack (decoder, voices,
    /// mixer, device sink) consuming every retained sound request.
    pub audio: crate::audio_runtime::AudioRuntime,
    /// This cycle's sampled input for `ui_loop::update_top_level`.
    pub input: crate::ui_loop::Input,
    /// The keyboard, fed by the window owner.
    pub keyboard: crate::ui_keyboard::Keyboard,
    /// The press events of the current cycle.
    pub key_presses: Vec<crate::ui_keyboard::Event>,
    /// This logic update's keyboard events were already split by
    /// [`Runtime::poll_update_input`] ahead of `updateGame`.
    keyboard_polled: bool,
    canvas_ready: bool,
    scene_delta: i32,
    /// The scene delta as the last draw consumed it; positioned sound reads it
    /// after the interfaces.
    pub drawn_scene_delta: i32,
    /// The input telemetry watch: sampled mouse events (client_watch.rs).
    pub client_watch: crate::client_watch::InputTelemetry,
    /// The camera state and orbit angles sampled before this cycle's camera
    /// update, for the telemetry send (which precedes the camera update).
    pub telemetry_camera: Option<crate::client_watch::CameraSample>,
}
impl Runtime {
    /// After a successful game-login reply: sets allow-members on the world
    /// loc list, the obj list and the NPC list. A changed obj/NPC store
    /// resets its cache (for objects that includes the inventory icon cache):
    /// the reset is the stores' members view reading the new flag. The
    /// asynchronous-rebuild loc list is the scene builder's store
    /// (`Game::allow_members`).
    pub fn set_allow_members(&mut self, allow: bool) {
        if let Some(locs) = &self.engine.configs.locs {
            locs.allow_members.set(allow);
        }
        if let Some(objs) = &self.engine.configs.objs {
            if objs.allow_members.set(allow) {
                if let Some(icons) = &self.state.icons {
                    icons.borrow_mut().reset();
                }
            }
        }
        if let Some(npcs) = &self.engine.configs.npcs {
            npcs.allow_members.set(allow);
        }
        if crate::ui_debug_flags::flags().members_trace {
            // Diagnostic: the live stores' gated views for sample real types
            // (NPC 44 Banker, loc 215 Wall, obj 4151 Abyssal whip).
            let npc = self.engine.configs.npcs.as_ref().and_then(|s| {
                s.get(rs910_symbols::npc::BANKER.id() as u32).map(|n| {
                    n.ops_for(s.allow_members.get())
                        .map(|o| o.map(str::to_owned))
                })
            });
            let loc = self.engine.configs.locs.as_ref().and_then(|s| {
                s.get(rs910_symbols::loc::GRAPPLE_WALL.id() as u32)
                    .map(|l| {
                        (
                            l.ops_for(s.allow_members.get())
                                .map(|o| o.map(str::to_owned)),
                            l.active_for(s.allow_members.get()),
                        )
                    })
            });
            let params = &self.engine.configs.params;
            let obj = self.engine.configs.objs.as_ref().and_then(|s| {
                s.get(rs910_symbols::obj::ABYSSAL_WHIP.id() as u32)
                    .map(|o| {
                        let g = o.members_gated(s.allow_members.get(), &|k| {
                            params.get(&k).is_none_or(|p| p.autodisable)
                        });
                        (g.iops.clone(), g.inventory.tradeable)
                    })
            });
            log::info!(
                "[members] setAllowMembers({allow}): npc44 ops {npc:?}; loc215 (ops, active) {loc:?}; obj4151 (iops, tradeable) {obj:?}"
            );
        }
    }
    /// `resetCaches(false)`, run by `logout`
    /// This port decodes every config type once, so the per-list
    /// `cacheReset` of the type caches has nothing to evict; the caches that
    /// hold derived state are reset: `objTypeList.cacheReset` ->
    /// `cacheReset` (inventory icons,,
    /// whose rebuild re-samples the icon `Math.random`) and
    /// `reset` (the same icon owner here).
    pub fn reset_caches(&mut self) {
        if let Some(icons) = &self.state.icons {
            icons.borrow_mut().reset();
        }
    }
    /// One redraw's cache work (`rs910_core::cache_schedule`): ageing the
    /// interface caches (sprites, masks, fonts) and the interface model caches,
    /// and, when memory is short, dropping their soft entries. Returns how
    /// many models went.
    pub fn clean_caches(&mut self, frame: rs910_core::cache_schedule::Frame) -> usize {
        use rs910_core::cache_schedule::{INTERFACE_AGE, MODEL_AGE};
        let mut dropped = 0;
        if frame.clean {
            if let Some(sprites) = self.state.sprites.as_mut() {
                sprites.clean(INTERFACE_AGE as i32);
            }
            if let Some(fonts) = self.state.fonts.as_ref() {
                fonts.clean(INTERFACE_AGE as i32);
            }
            if let Some(models) = self.target.models.as_mut() {
                models.clean(MODEL_AGE);
            }
        }
        if frame.clear_soft {
            if let Some(sprites) = self.state.sprites.as_mut() {
                sprites.clear_soft();
            }
            if let Some(fonts) = self.state.fonts.as_ref() {
                fonts.clear_soft();
            }
            if let Some(models) = self.target.models.as_mut() {
                dropped += models.clear_soft();
            }
        }
        dropped
    }
    /// The camera inputs `sendTelemetry` reads
    /// `cameraState`, the orbit angles or
    /// the cam2 ENTITY position's rotation.
    pub fn camera_sample(
        &self,
        orbit_pitch: f32,
        orbit_yaw: f32,
    ) -> crate::client_watch::CameraSample {
        use crate::client_watch::CameraSample;
        if self.engine.camera.cam2.camera_state != 3 {
            return CameraSample::Orbit {
                pitch: orbit_pitch,
                yaw: orbit_yaw,
            };
        }
        match &self.engine.camera.cam2.position {
            Some(crate::ui_cam2::Position::Entity(p)) => CameraSample::Entity(p.rotation),
            _ => CameraSample::Other,
        }
    }
    /// `sendTelemetry` for this
    /// logic cycle's `updateGame`.
    fn send_telemetry(&mut self, vars: &mut Variables<'_>) {
        let camera = self.telemetry_camera.take().unwrap_or_else(|| {
            self.camera_sample(
                self.engine.game_host.orbit_pitch,
                self.engine.game_host.orbit_yaw,
            )
        });
        let prefs = &mut vars.state.queries.preferences;
        let toolkit = prefs.options.get("toolkit").unwrap_or(0);
        // `toolkit.textureFormat()`: only the GL toolkit (id 1) reads
        // driver formats; the other toolkits return null
        let formats = (prefs.options.get("displayMode") == Some(1))
            .then_some(self.engine.platform.gl_texture_formats.as_slice());
        let options = &prefs.options;
        let block = || options.encode();
        self.client_watch.send_telemetry(
            crate::client_watch::Telemetry {
                now: crate::logic_clock::monotonic_millis(),
                keyboard_events: &self.key_presses,
                camera_changed: &mut self.engine.camera.cam2.changed,
                camera,
                focus: self.engine.platform.app_focused,
                preferences_notified: &mut prefs.change_notified,
                preferences_block: &block,
                texture_formats_sent: &mut prefs.texture_formats_sent,
                toolkit,
                texture_formats: formats,
            },
            &mut self.engine.outgoing,
        );
    }
    pub fn new(pack: Pack) -> Result<Self> {
        let shared = rs910_config::login_configs::LoginConfigs::read(&pack);
        Self::new_with(pack, shared)
    }
    /// [`Runtime::new`] decoding the config archives `shared` has read.
    pub fn new_with(pack: Pack, shared: rs910_config::login_configs::LoginConfigs) -> Result<Self> {
        let state = Self::load_state(&pack)?;
        let mut engine = Engine::default();
        engine.load_cache_with(&pack, shared);
        let audio = crate::audio_runtime::AudioRuntime::new(pack.clone());
        Self::assemble(pack, engine, state, audio)
    }
    /// The `SETUP_STATIC_SPRITES` half of [`Runtime::new`]:
    /// the default fonts/sprites and interface configs.
    pub fn load_state(pack: &Pack) -> Result<State> {
        let mut state = State::default();
        state.load(pack)?;
        state.model_animations = Some(std::rc::Rc::new(std::cell::RefCell::new(
            crate::ui_model_animation::Animations::new(pack.clone(), 0)?,
        )));
        Ok(state)
    }
    /// Join the staged loading owners: the engine's config decoders
    /// (`SETUP_CONFIG_DECODERS`), the static sprite/font state and
    /// the audio runtime.
    pub fn assemble(
        pack: Pack,
        engine: Engine,
        mut state: State,
        audio: crate::audio_runtime::AudioRuntime,
    ) -> Result<Self> {
        state.objs = engine.configs.objs.clone();
        state.npcs = engine.configs.npcs.clone();
        // The frame-rate overlay starts on for any mode-where other than LIVE
        // and id 10.
        state.debug_visible[0] = crate::applet_params::get()
            .mode_where()
            .ok()
            .flatten()
            .is_none_or(|id| id != 0 && id != 10);
        if let (Some(objs), Some(fonts)) = (&state.objs, &state.fonts) {
            state.icons = Some(std::rc::Rc::new(std::cell::RefCell::new(
                crate::ui_icons::Icons::new(pack.clone(), objs.clone(), fonts)?,
            )));
        }
        let mut target = crate::ui_backend::Target::with_pack(pack.clone())?;
        target.set_http_host(crate::ui_debug_flags::flags().png_host.clone());
        Ok(Self {
            store: Store::from_pack(pack.clone())?,
            state,
            scripts: Scripts::from_pack(pack.clone())?,
            pool: Pool::default(),
            frame: Frame::default(),
            engine,
            diagnostics: Diagnostics::default(),
            target,
            renderer_settings: Default::default(),
            audio,
            input: Default::default(),
            keyboard: Default::default(),
            key_presses: Vec::new(),
            client_watch: Default::default(),
            telemetry_camera: None,
            keyboard_polled: false,
            canvas_ready: false,
            scene_delta: 0,
            drawn_scene_delta: 0,
        })
    }

    /// `getText` consumes dynamic values from the packet
    /// and delegates enum/object names through the dynamic-name provider.
    fn render_quickchat(&self, phrase_id: u16, bytes: &[u8], pos: &mut usize) -> String {
        self.engine.configs.quickchat.as_ref().map_or_else(
            || format!("[quickchat:{phrase_id}]"),
            |phrases| {
                phrases.render(
                    phrase_id,
                    bytes,
                    pos,
                    &self.state.configs,
                    self.engine.configs.objs.as_deref(),
                )
            },
        )
    }

    /// The cursor subsystem of loginscreen_load1172 calls cursors_login1129.
    /// Direct world login bypasses that title screen, so run the same cache
    /// procedure after local VarCs restore, before opening gameplay interfaces.
    pub fn initialize_cursors(&mut self, vars: &mut Variables<'_>) -> Result<()> {
        use crate::ui_hooks::Executor;
        let provider = Provider {
            scripts: &self.scripts,
            definitions: vars.definitions,
        };
        let mut runner = Runner {
            pool: &mut self.pool,
            provider: &provider,
            engine: &mut self.engine,
            domains: Domains::Game(vars),
            executions: vec![],
            missing: vec![],
        };
        runner.run(
            &mut self.store,
            &mut self.state,
            crate::ui_hooks::Request {
                args: Some(vec![crate::ui_components::Arg::Int(1129)]),
                ..Default::default()
            },
            crate::ui_hooks::ONLOAD_LIMIT,
        )?;
        let failure = runner
            .executions
            .iter()
            .find_map(|e| e.result.as_ref().err().map(ToString::to_string));
        let missing = !runner.missing.is_empty();
        self.diagnostics.record(runner.executions, runner.missing);
        anyhow::ensure!(!missing, "cursor initialization script missing");
        anyhow::ensure!(
            failure.is_none(),
            "cursor initialization failed: {failure:?}"
        );
        Ok(())
    }
    /// Execute an original global client trigger at the packet boundary. This is
    /// the same trigger lookup/VM owner used by active entity processing, but
    /// with the generic `(-1, -1)` fallback used by `executeTriggeredScriptMapElement`.
    fn run_global_trigger(&mut self, vars: &mut Variables<'_>, trigger: i32) -> Result<()> {
        let provider = Provider {
            scripts: &self.scripts,
            definitions: vars.definitions,
        };
        let Some((script_id, script)) = provider.get_trigger(trigger, -1, -1)? else {
            return Ok(());
        };
        let mut runner = Runner {
            pool: &mut self.pool,
            provider: &provider,
            engine: &mut self.engine,
            domains: Domains::Game(vars),
            executions: vec![],
            missing: vec![],
        };
        runner.run_compiled(
            &mut self.store,
            &mut self.state,
            script_id,
            &script,
            crate::ui_hooks::INTERACTIVE_LIMIT,
        )?;
        self.diagnostics.record(runner.executions, runner.missing);
        Ok(())
    }
    /// resizeTopLevel. The first layout/onload sees the real
    /// drawable canvas; later resizes enqueue onresize on this same graph.
    pub fn resize(&mut self, canvas: [i32; 2]) -> Result<()> {
        if self.state.layout.canvas != canvas {
            self.engine.scene.player_picks = None;
        }
        anyhow::ensure!(canvas.iter().all(|&v| v > 0), "invalid UI canvas");
        if self.canvas_ready && self.state.layout.canvas == canvas {
            return Ok(());
        }
        self.state.layout.canvas = canvas;
        self.canvas_ready = true;
        if self.state.life.top != -1 {
            self.state
                .layout
                .interface(&mut self.store, self.state.life.top, canvas, true)?;
        }
        self.state.life.redraw.fill(true);
        Ok(())
    }
    /// `showLogin(true)` / `showLobby(true)` interface
    /// replacement through the ordinary hook runner,
    /// so the closed tree's hooks and the new top level's `onload` use the
    /// same VM pool, domains and diagnostics as packet-driven lifecycle.
    pub fn show_top_level(&mut self, vars: &mut Variables<'_>, top: i32) -> Result<()> {
        anyhow::ensure!(
            self.canvas_ready,
            "UI top level before canvas initialization"
        );
        let provider = Provider {
            scripts: &self.scripts,
            definitions: vars.definitions,
        };
        let mut runner = Runner {
            pool: &mut self.pool,
            provider: &provider,
            engine: &mut self.engine,
            domains: Domains::Game(vars),
            executions: vec![],
            missing: vec![],
        };
        let result =
            crate::ui_lifecycle::replace_top(&mut self.store, &mut self.state, top, &mut runner);
        self.diagnostics.record(runner.executions, runner.missing);
        self.drain_services();
        for rect in std::mem::take(&mut self.state.menu.redraw) {
            self.frame.request_at(&mut self.state, rect);
        }
        result
    }
    fn sync_active_clan_channel(&mut self) {
        self.sync_host_game_links();
        self.state.active_clan_channel =
            self.engine.social.active_channel.as_ref().map(|channel| {
                (
                    self.engine.social.active_channel_affined,
                    channel.users.iter().map(|user| user.name.clone()).collect(),
                )
            });
    }
    /// One `read` packet. The original client stamps each `lastOn*TransmitRedrawCycle`
    /// inside the handler (e.g.,), so the
    /// same update's interface walk sees it.
    pub fn packet(
        &mut self,
        vars: &mut Variables<'_>,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        self.engine.social.player_is_members = self.engine.account.player_is_members;
        self.engine.social.now_seconds = (vars.now)() / 1000;
        let result = self.packet_event(vars, event);
        self.stamp_social_transmits();
        result
    }

    /// Apply pending social/chat redraw stamps and system messages.
    fn stamp_social_transmits(&mut self) {
        for (chat_type, message) in std::mem::take(&mut self.engine.social.system_messages) {
            self.engine
                .messages
                .history
                .add_system_message(chat_type, message);
            self.engine.messages.changed = true;
        }
        let redraw = self.state.life.cycles.redraw;
        let stamps = self.engine.social.take_stamps();
        if stamps.friend {
            self.state.life.cycles.friend = redraw;
        }
        if stamps.clan {
            self.state.life.cycles.clan = redraw;
        }
        if stamps.clan_settings {
            self.state.life.cycles.clan_settings = redraw;
        }
        if stamps.clan_channel {
            self.state.life.cycles.clan_channel = redraw;
        }
        if std::mem::take(&mut self.engine.messages.changed) {
            self.state.life.cycles.chat = redraw;
        }
    }

    /// Called after actor updates, where updateGame calls
    /// updateInterfaces, updateTopLevelInterface,
    /// redrawCycle++ and the three hook queue drains.
    /// A failed component service stays at the queue front; it must not
    /// silently discard this or later delayed changes.
    /// Scene keys of the open active-loc/obj subinterfaces
    /// (active-loc and active-obj sub-interfaces): locs as
    /// `(level, layer, x, z)`, objs as `(level, x, z)`, both scene-local
    /// against the map `base` tile.
    #[allow(clippy::type_complexity)]
    pub fn active_scene_keys(
        &self,
        base: [i32; 2],
    ) -> (Vec<(i32, i32, i32, i32)>, Vec<(i32, i32, i32)>) {
        let (mut locs, mut objs) = (Vec::new(), Vec::new());
        for node in self.state.life.subs.ordered() {
            match node.borrow().binding.as_ref() {
                Some(crate::server_prot::ActiveBinding::Loc { coord, shape, .. }) => {
                    if let Some(layer) = crate::ui_lifecycle::loc_layer(*shape) {
                        locs.push((coord.level, layer, coord.x - base[0], coord.z - base[1]));
                    }
                }
                Some(crate::server_prot::ActiveBinding::Obj { coord, .. }) => {
                    objs.push((coord.level, coord.x - base[0], coord.z - base[1]));
                }
                _ => {}
            }
        }
        (locs, objs)
    }

    fn active_entity(
        &self,
        binding: &crate::server_prot::ActiveBinding,
        vars: &Variables<'_>,
    ) -> Option<ActiveEntity> {
        match binding {
            crate::server_prot::ActiveBinding::Player { index } => {
                let index = usize::try_from(*index).ok()?;
                let player = vars.scene.players?.players.get(index)?.as_ref()?;
                Some(ActiveEntity::Player {
                    index: index as i32,
                    name: player.appearance.name.clone().unwrap_or_default(),
                    title: player.appearance.title.clone(),
                    chat: player.chat.as_ref().and_then(|chat| chat.text.clone()),
                    overlay_height: player.actor.scene.height.max(0),
                    target: player.target,
                    position: [player.fine_x, player.motion.y, player.fine_z],
                    screen_bounds: self.engine.scene.player_picks.as_ref().and_then(|frame| {
                        frame
                            .picks
                            .iter()
                            .find(|pick| pick.id.pid == index as i32)
                            .and_then(|pick| pick.screen_bounds)
                    }),
                })
            }
            crate::server_prot::ActiveBinding::Npc { index } => {
                let index = usize::try_from(*index).ok()?;
                let npc = vars.scene.npcs?.entities.get(&index)?;
                let type_id = npc.type_id;
                let configured = u32::try_from(type_id)
                    .ok()
                    .and_then(|id| self.engine.configs.npcs.as_ref()?.get(id));
                Some(ActiveEntity::Npc {
                    index: index as i32,
                    type_id,
                    name: if npc.name.is_empty() {
                        configured
                            .map(|definition| definition.name.clone())
                            .unwrap_or_default()
                    } else {
                        npc.name.clone()
                    },
                    chat: npc.path.chat.as_ref().and_then(|chat| chat.text.clone()),
                    stats: npc.stats,
                    stat_max: npc.stat_max,
                    vislevel: npc.vislevel,
                    active: configured.is_none_or(|definition| definition.active),
                    overlay_height: npc.path.actor.scene.height.max(0),
                    target: npc.path.target,
                    position: [npc.path.fine_x, npc.path.motion.y, npc.path.fine_z],
                    screen_bounds: self.engine.scene.player_picks.as_ref().and_then(|frame| {
                        frame
                            .npc_picks
                            .iter()
                            .find(|pick| pick.id.pid == index as i32)
                            .and_then(|pick| pick.screen_bounds)
                    }),
                })
            }
            crate::server_prot::ActiveBinding::Loc {
                coord,
                shape,
                angle,
                id,
            } => {
                let x = (coord.x - (vars.scene.base[0] >> 9)) * 512 + 256;
                let z = (coord.z - (vars.scene.base[1] >> 9)) * 512 + 256;
                let y = crate::protocol910::terrain::height(vars.scene.terrain, x, z, coord.level)
                    .unwrap_or(0);
                let position = [x as f32, y as f32, z as f32];
                let (width, length) = self
                    .engine
                    .configs
                    .locs
                    .as_ref()
                    .and_then(|store| u32::try_from(*id).ok().and_then(|id| store.get(id)))
                    .map(|loc| {
                        if angle & 1 == 0 {
                            (loc.width, loc.length)
                        } else {
                            (loc.length, loc.width)
                        }
                    })
                    .unwrap_or((1, 1));
                let screen_bounds = self.engine.scene.player_picks.as_ref().and_then(|frame| {
                    project_active_bounds(
                        frame,
                        position,
                        f32::from(width) * 256.0,
                        f32::from(length) * 256.0,
                        512.0,
                    )
                });
                // get_loc_overlay_height:
                // the bound scene loc's `height()`, captured from the last
                // scene frame.
                let overlay_height = crate::ui_lifecycle::loc_layer(*shape)
                    .and_then(|layer| {
                        let frame = self.engine.scene.player_picks.as_ref()?;
                        let key = (
                            coord.level,
                            layer,
                            coord.x - (vars.scene.base[0] >> 9),
                            coord.z - (vars.scene.base[1] >> 9),
                        );
                        frame.loc_heights.get(&key).copied()
                    })
                    .unwrap_or(0);
                Some(ActiveEntity::Loc {
                    id: *id,
                    overlay_height,
                    position,
                    screen_bounds,
                })
            }
            crate::server_prot::ActiveBinding::Obj { coord, id } => {
                let x = (coord.x - (vars.scene.base[0] >> 9)) * 512 + 256;
                let z = (coord.z - (vars.scene.base[1] >> 9)) * 512 + 256;
                let y = crate::protocol910::terrain::height(vars.scene.terrain, x, z, coord.level)
                    .unwrap_or(0);
                let position = [x as f32, y as f32, z as f32];
                let screen_bounds =
                    self.engine.scene.player_picks.as_ref().and_then(|frame| {
                        project_active_bounds(frame, position, 256.0, 256.0, 512.0)
                    });
                // get_obj_overlay_height:
                // `height()` of the stack on that tile.
                let overlay_height = self
                    .engine
                    .scene
                    .player_picks
                    .as_ref()
                    .and_then(|frame| {
                        frame
                            .obj_heights
                            .get(&(
                                coord.level,
                                coord.x - (vars.scene.base[0] >> 9),
                                coord.z - (vars.scene.base[1] >> 9),
                            ))
                            .copied()
                    })
                    .unwrap_or(10);
                Some(ActiveEntity::Obj {
                    id: *id,
                    overlay_height,
                    position,
                    screen_bounds,
                })
            }
        }
    }

    /// mainloop: keyboard.processEvents then the
    /// repeat/typed and press event splits for this logic update, and the
    /// cutscene cancel test on them.
    fn split_keyboard_events(&mut self, mouse_event: Option<crate::ui_defaults::MouseEvent>) {
        self.keyboard.process_events();
        self.engine.platform.held_keys = self.keyboard.held;
        let (all, presses) = self.keyboard.poll();
        self.input.keys = all.iter().map(|e| (e.code, i32::from(e.ch))).collect();
        self.key_presses = presses;
        self.test_cutscene_cancel(mouse_event);
    }

    /// The mainloop's keyboard step ahead of `updateGame`: split this
    /// update's keyboard events and test `cutsceneDefaults.cancelbinding`
    ///  before the cutscene action clock reads it. The
    /// following [`Runtime::tick`] reuses the split.
    pub fn poll_update_input(&mut self) -> bool {
        self.split_keyboard_events(self.input.event);
        self.keyboard_polled = true;
        std::mem::take(&mut self.engine.camera.cutscene.cancel)
    }

    pub fn tick(&mut self, vars: &mut Variables<'_>) -> Result<()> {
        // updateGame clears hoverComponent before the
        // interface walk; the HOVER_TEXT component pass installs this cycle's
        // visible owner again when it is present.
        self.state.hover_text = None;
        self.state
            .message_box
            .clone_from(&self.engine.builtins.message_box);
        self.sync_active_clan_channel();
        self.sync_host_game(vars)?;
        self.apply_cutscene_requests(vars)?;
        self.engine.world_map.borrow_mut().varps = vars
            .player
            .as_ref()
            .map_or_else(Vec::new, |player| player.current.clone());
        self.engine.update_world_map(self.state.fonts.as_ref());
        let mut world_map_interaction =
            crate::ui_loop::WorldMapInteraction::new(self.engine.world_map.clone());
        // `updateTopLevelInterface(mouseX, mouseY)`: the queued press
        // replaces the cursor for the world-map update too.
        let world_map_mouse = self.input.click.unwrap_or(self.input.mouse);
        vars.state.queries.minimenu_option_count = self.state.minimenu.option_count;
        vars.state.queries.minimenu_submenu_count = self.state.minimenu.submenu_count;
        self.scene_delta = self.scene_delta.wrapping_add(1);
        // updateGame decrements rebootTimer once per logic cycle
        // while it remains above one and stamps the misc transmit redraw.
        if self.engine.reboot_timer > 1 {
            self.engine.reboot_timer -= 1;
            self.state.life.cycles.misc = self.state.life.cycles.redraw;
        }
        if let Some(models) = &mut self.target.models {
            models.set_model_detail(
                crate::rebuild::BuildPrefs::from_options(&vars.state.queries.preferences.options)
                    .model_detail(),
            );
            models.sync(&vars.scene);
            models.sync_player_varps(vars.player.as_deref().map(|p| &p.current[..]));
            models.sync_inventories(&self.engine.inv_cache);
        }
        if crate::ui_debug_flags::flags().inv_trace && vars.cycle % 100 == 0 {
            log::info!(
                "[inventory] cycle {} inv {:?}",
                vars.cycle,
                self.engine
                    .inv_cache
                    .inventory(rs910_symbols::inv::BACKPACK.id(), false)
            );
            let backpack = rs910_symbols::interface::BACKPACK.id();
            for id in [backpack, rs910_symbols::interface::GAME_WINDOW.id()] {
                if let Some(interface) = self.store.interfaces.get(&id) {
                    for c in interface.borrow().components.borrow().iter().flatten() {
                        let c = c.borrow();
                        if id == backpack || (102..=113).contains(&(c.f.parentlayer & 65535)) {
                            log::info!("[inventory] {}:{} layer {} pos {},{} size {},{} hide {} children {}",id,c.f.parentlayer&65535,c.f.layer,c.f.x,c.f.y,c.f.width,c.f.height,c.f.hide,c.children.as_ref().map_or(0,|a|a.borrow().len()));
                        }
                    }
                }
            }
        }
        if crate::ui_debug_flags::flags().camera_trace && vars.cycle % 10 == 0 {
            let cam = &self.engine.camera.cam2;
            log::info!(
                "[camera] cycle={} state={} control={} ready={} angles={:?} eye={:?} lookat={:?}",
                vars.cycle,
                cam.camera_state,
                cam.control_mode,
                cam.ready(),
                cam.position.as_ref().map(|p| match p {
                    crate::ui_cam2::Position::Entity(p) => (
                        crate::ui_cam2::pitch_of(&p.rotation),
                        crate::ui_cam2::yaw_of(&p.rotation)
                    ),
                    _ => (0., 0.),
                }),
                cam.eye(),
                cam.lookat_point()
            );
        }
        // updateGame: the option list is rebuilt every cycle.
        if !self.state.minimenu.open {
            self.state.minimenu.reset();
        }
        // updateGame steps cam2 before updateInterfaces.
        self.engine.camera.cam2.sync_scene(&vars.scene);
        if let Some(free) = self.engine.camera.free_camera.as_mut() {
            // handleOrbitInput replaces the cam2 step.
            let viewport = self.state.viewport.map(|(rect, _)| [rect[2], rect[3]]);
            let keyboard = &self.keyboard;
            if let Err(reason) = free.handle_orbit_input(
                viewport,
                self.engine.platform.mouse,
                self.input.middle_held,
                &|k| keyboard.held(k),
                &self.engine.camera.cam2.scene,
            ) {
                crate::logging::warn_repeated!("[client910] free camera update: {reason}");
            }
        } else if let Err(reason) = self.engine.camera.cam2.step(&mut *vars.now) {
            crate::logging::warn_repeated!("[client910] cam2 update: {reason}");
        }
        self.input.minimap =
            self.engine
                .camera
                .cam2
                .scene
                .local_player
                .map(|player| crate::ui_loop::MinimapInput {
                    player_fine: [
                        player.coord[0].wrapping_sub(self.engine.camera.cam2.scene.base[0]),
                        player.coord[2].wrapping_sub(self.engine.camera.cam2.scene.base[1]),
                    ],
                    player_size: self.engine.camera.cam2.scene.local_size,
                    camera_state: self.engine.camera.cam2.camera_state,
                    camera_yaw: (f64::from(self.engine.camera.cam2.yaw()) * 2607.594587617613)
                        as i32,
                    orbit_yaw: self.engine.minimap.orbit_yaw,
                    anticheat_angle: self.engine.minimap.angle,
                    zoom: self.engine.minimap.zoom,
                    toggle: self.engine.minimap.toggle,
                    walk_text: self.engine.menu.walk_here_text.clone(),
                    walk_cursor: self.engine.menu.default_walk_action,
                });
        vars.state.poll(vars.definitions, &mut *vars.now)?;
        while let Some(change) = vars.state.component_changes.front() {
            crate::ui_lifecycle::apply_change(&mut self.store, &mut self.state, change)?;
            vars.state.component_changes.pop_front();
        }
        if let Some(flag) = self.state.life.map_flag.take() {
            self.engine.minimap.flag = Some(flag);
        }
        self.input.mouse = self.engine.platform.mouse;
        self.engine.platform.mouse_buttons = [
            self.input.left_held,
            self.input.middle_held,
            self.input.right_held,
        ];
        // updateGame: the click cross fades over 400 cycles.
        if self.engine.menu.cross.mode != 0 {
            self.engine.menu.cross.cycle += 20;
            if self.engine.menu.cross.cycle >= 400 {
                self.engine.menu.cross.mode = 0;
            }
        }
        let mouse_event = self.input.event;
        self.engine.scene.player_routes = vars
            .scene
            .players
            .map(|ps| {
                ps.players
                    .iter()
                    .enumerate()
                    .filter_map(|(id, p)| p.as_ref().map(|p| (id, [p.x[0], p.z[0]])))
                    .collect()
            })
            .unwrap_or_default();
        self.engine.scene.npc_routes = vars
            .scene
            .npcs
            .map(|npcs| {
                npcs.entities
                    .iter()
                    .map(|(&index, npc)| (index, [npc.path.x[0], npc.path.z[0]]))
                    .collect()
            })
            .unwrap_or_default();
        self.input.scene_options = self.scene_options(
            &vars.scene,
            menu_builder::LocalVars {
                varps: vars.player.as_deref(),
                varbits: Some(vars.definitions),
            },
        );
        self.install_scene_quest_text(vars)?;
        if !std::mem::take(&mut self.keyboard_polled) {
            self.split_keyboard_events(mouse_event);
        }
        // updateGame sendTelemetry, on the ordinary
        // game connection queue before the interface/menu packets.
        self.send_telemetry(vars);
        self.input.held_keys = (0..112).map(|k| self.keyboard.held(k)).collect();
        self.input.cycle = vars.cycle;
        if self.renderer_settings.input(&self.state, &mut self.input) {
            self.key_presses.clear();
            self.engine.platform.mouse_buttons.fill(false);
            self.state.minimenu.open = false;
        }
        let transmits = crate::ui_loop::Transmits {
            varp: vars.varp_transmit,
            varc: vars.state.varc_transmit.counter(),
            varcstr: vars.state.string_transmit.counter(),
            inv: vars.state.inv_transmit.counter(),
            stat: vars.state.stat_transmit.counter(),
            varclan: vars.state.varclan_transmit.counter(),
        };
        let provider = Provider {
            scripts: &self.scripts,
            definitions: vars.definitions,
        };
        let invalid_active =
            crate::ui_lifecycle::invalid_active_sub_parents(&self.state, &vars.scene);
        let npc_process: Vec<(i32, ActiveEntity)> = vars
            .scene
            .npcs
            .map(|npcs| {
                npcs.slots
                    .iter()
                    .filter_map(|index| {
                        let npc = npcs.entities.get(index)?;
                        let binding = crate::server_prot::ActiveBinding::Npc {
                            index: *index as i32,
                        };
                        Some((npc.type_id, self.active_entity(&binding, vars)?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let player_process: Vec<(usize, ActiveEntity)> = vars
            .scene
            .players
            .map(|players| {
                players
                    .players
                    .iter()
                    .enumerate()
                    .filter_map(|(index, player)| {
                        player.as_ref()?;
                        let binding = crate::server_prot::ActiveBinding::Player {
                            index: index as i32,
                        };
                        Some((index, self.active_entity(&binding, vars)?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let active_entity_subs: Vec<crate::ui_lifecycle::SubRef> = self
            .state
            .life
            .subs
            .ordered()
            .filter(|node| {
                matches!(
                    node.borrow().binding,
                    Some(crate::server_prot::ActiveBinding::Player { .. })
                        | Some(crate::server_prot::ActiveBinding::Npc { .. })
                )
            })
            .cloned()
            .collect();
        // Collect trigger bindings before borrowing the VM runner's mutable
        // game domains. Invalid parents are closed immediately below and are
        // excluded from this pre-walk snapshot.
        let active_triggers: Vec<(i32, i32, i32, Option<ActiveEntity>)> = self
            .state
            .life
            .subs
            .ordered()
            .filter_map(|node| {
                let sub = node.borrow();
                if invalid_active.contains(&sub.parent) {
                    return None;
                }
                let binding = sub.binding.as_ref()?;
                let trigger = match binding {
                    crate::server_prot::ActiveBinding::Npc { .. } => 24,
                    crate::server_prot::ActiveBinding::Player { .. } => 25,
                    crate::server_prot::ActiveBinding::Loc { .. } => 26,
                    crate::server_prot::ActiveBinding::Obj { .. } => 27,
                };
                Some((trigger, sub.id, -1, self.active_entity(binding, vars)))
            })
            .collect();
        let mut runner = Runner {
            pool: &mut self.pool,
            provider: &provider,
            engine: &mut self.engine,
            domains: Domains::Game(vars),
            executions: vec![],
            missing: vec![],
        };
        for parent in invalid_active {
            if let Some(sub) = self.state.life.subs.get(parent) {
                if sub.borrow().binding.is_some() {
                    crate::ui_lifecycle::close_sub(
                        &mut self.store,
                        &mut self.state,
                        &sub,
                        true,
                        false,
                        &mut runner,
                    )?;
                }
            }
        }
        // The active sub-interface runs its trigger before the bound interface
        // is walked. Resolve its JS5 trigger group using the
        // same sub-id fallback order, then execute it through the ordinary VM
        // pool and game domains. Entity-specific host queries remain explicit
        // gaps in Engine; the trigger path itself is live.
        for (trigger, primary, secondary, active_entity) in active_triggers {
            if let Some((script_id, script)) = provider.get_trigger(trigger, primary, secondary)? {
                runner.engine.scene.active_entity = active_entity;
                runner.run_compiled(
                    &mut self.store,
                    &mut self.state,
                    script_id,
                    &script,
                    crate::ui_hooks::INTERACTIVE_LIMIT,
                )?;
                runner.engine.scene.active_entity = None;
            }
        }
        for (type_id, active_entity) in npc_process {
            if let Some((script_id, script)) = provider.get_trigger(22, type_id, -1)? {
                runner.engine.scene.active_entity = Some(active_entity);
                runner.run_compiled(
                    &mut self.store,
                    &mut self.state,
                    script_id,
                    &script,
                    crate::ui_hooks::INTERACTIVE_LIMIT,
                )?;
                runner.engine.scene.active_entity = None;
            }
        }
        if let Some((script_id, script)) = provider.get_trigger(23, -1, -1)? {
            for (index, active_entity) in player_process {
                runner.engine.scene.active_entity = Some(active_entity);
                runner.run_compiled_with_ints(
                    &mut self.store,
                    &mut self.state,
                    script_id,
                    &script,
                    crate::ui_hooks::INTERACTIVE_LIMIT,
                    &[index as i32],
                )?;
                runner.engine.scene.active_entity = None;
            }
        }
        // The same component walk runs for each active player/NPC interface after PROCESS_* triggers.
        // The retained state stores the effective scene viewport rectangle;
        // its origin is the original viewport component's screen origin.
        let active_clip = self
            .state
            .viewport
            .map(|(rect, _)| rect)
            .unwrap_or_else(|| {
                [
                    0,
                    0,
                    self.state.layout.canvas[0],
                    self.state.layout.canvas[1],
                ]
            });
        for sub in active_entity_subs {
            crate::ui_loop::process_active_interface(
                crate::ui_loop::ActiveWalk {
                    store: &mut self.store,
                    state: &mut self.state,
                    frame: &mut self.frame,
                    input: &self.input,
                    transmits: &transmits,
                    clip: active_clip,
                    origin: [active_clip[0], active_clip[1]],
                },
                &sub,
                &mut runner,
            )?;
        }
        crate::ui_interaction::begin_cycle(&mut self.state, self.input.mouse);
        crate::ui_loop::update_top_level_with_world_map(
            &mut self.store,
            &mut self.state,
            &mut self.frame,
            &self.input,
            &transmits,
            Some(&mut runner),
            Some(&mut world_map_interaction),
        )?;
        self.state.life.cycles.redraw = self.state.life.cycles.redraw.wrapping_add(1);
        // mouseEvents / keyboard events and the wheel are consumed per cycle.
        self.input.click = None;
        self.input.event = None;
        self.input.wheel = 0;
        self.input.keys.clear();
        crate::ui_loop::drain(&mut self.store, &mut self.state, &mut runner)?;
        let release = crate::ui_interaction::finish_drag(
            &mut self.store,
            &mut self.state,
            &mut runner,
            self.input.left_held || self.input.middle_held || self.input.right_held,
        )?;
        self.diagnostics.record(runner.executions, runner.missing);
        self.finish_world_map_cycle(world_map_interaction, world_map_mouse)?;
        if let Some([x, y]) = release {
            if self.state.minimenu.pending_open {
                self.open_menu([x, y])?;
            } else if let Some(entry) = self.state.minimenu.pending.take() {
                self.use_menu_option(&entry, x, y, false);
            }
            self.state.minimenu.pending_open = false;
        }
        // updateGame update after the hook queues drain;
        // it returns immediately while a cutscene runs.
        if self.engine.camera.cutscene.client_id < 0 {
            let modifier_held = self.engine.configs.minimenu.as_ref().is_some_and(|d| {
                crate::ui_defaults::key_binding_held(&d.menu_modifier_key, |k| {
                    self.keyboard.held(k)
                })
            });
            let canvas_height = self.state.layout.canvas[1];
            let custom = self.state.menu.custom;
            // update begins with getFontMetrics, including while open.
            let font = self.menu_font()?;
            self.state.minimenu.row_height = font.metrics.ascent + font.metrics.descent;
            self.state.minimenu.popup.ascent = font.metrics.ascent;
            self.state.minimenu.popup.descent = font.metrics.descent;
            self.state.minimenu.popup.custom = custom;
            if !self.state.minimenu.open {
                // getEntryQuests for interface/target entries.
                for (slot, obj) in self.state.minimenu.object_quest_entries() {
                    let tags = self.object_quest_tags(vars, obj)?;
                    self.state.minimenu.set_quest_text(slot, tags);
                }
            }
            self.state
                .minimenu
                .update(modifier_held, canvas_height, custom);
            // Keep the direct Engine host in step with the same owner that the
            // HookHost reads. Scripts outside a component hook still see the
            // entries produced by this cycle's update.
            self.engine.menu.active = self.state.minimenu.view(self.state.minimenu.active);
            self.sync_host_game_minimenu(vars);
            self.engine.menu.secondary = self.state.minimenu.view(self.state.minimenu.secondary);
            self.engine.menu.counts = [
                self.state.minimenu.option_count,
                self.state.minimenu.submenu_count,
            ];
            // update: the click half over this cycle's mouse event.
            self.menu_click(mouse_event)?;
        }
        self.run_actions(vars)?;
        //  friendToastQueue walk.
        for message in self.engine.social.poll_friend_toasts((vars.now)() / 1000) {
            self.engine
                .messages
                .history
                .add_message(crate::ui_chat::NewChatLine::system(5, message));
            self.engine.messages.changed = true;
        }
        self.stamp_social_transmits();
        if let Err(error) = vars.state.queries.preferences.save() {
            log::warn!("[client910] save preferences: {error}");
        }
        let now = (vars.now)();
        if let Err(error) = vars
            .state
            .persistence
            .save_local(&mut vars.state.client, now, false)
        {
            log::warn!("[client910] save client variables: {error:#}");
        }
        self.engine.outgoing.extend(vars.state.persistence.flush(
            &mut vars.state.client,
            now,
            self.engine.outgoing.len() + self.state.interaction.outgoing.len(),
        )?);
        self.frame.drag = self.state.interaction.drag.clone();
        self.engine
            .outgoing
            .append(&mut self.state.interaction.outgoing);
        self.drain_host_game();
        self.audio
            .update_preferences(&vars.state.queries.preferences.options);
        // update: the local player's
        // scene-local `trans`, with cameraState / cam2 yaw.
        let base = vars.scene.base;
        self.audio
            .update_listener(vars.scene.local_player.map(|player| {
                crate::audio_runtime::Listener {
                    level: player.level,
                    x: player.coord[0].wrapping_sub(base[0]) as f32,
                    z: player.coord[2].wrapping_sub(base[1]) as f32,
                }
            }));
        self.audio.set_camera(
            self.engine.camera.cam2.camera_state,
            self.engine.camera.cam2.yaw(),
        );
        self.audio.tick(
            now,
            self.engine.login.world_list_game,
            &mut self.engine.outgoing,
        );
        for sound in std::mem::take(&mut self.engine.effects.sounds) {
            self.audio.submit(&sound);
        }
        let active_target = self.engine.scene.active_target;
        for sound in std::mem::take(&mut self.engine.effects.sequence_sounds) {
            self.audio.play_sequence_sound(&sound, active_target);
        }
        self.drain_services();
        for rect in std::mem::take(&mut self.state.menu.redraw) {
            self.frame.request_at(&mut self.state, rect);
        }
        self.frame.consume_updates(&mut self.store, &mut self.state)
    }

    /// Drain lifecycle service requests so the live session stays bounded.
    /// Animation resets run synchronously before onload hooks. Interface closure
    /// removes its live menu entries through RemoveMenuOptions.
    fn drain_services(&mut self) {
        let pending = std::mem::take(&mut self.state.life.services);
        self.diagnostics.services_drained += pending.len();
        for request in pending {
            if let crate::ui_lifecycle::Service::RemoveMenuOptions(id) = request {
                self.state.minimenu.remove_interface_options(id);
                if self.state.minimenu.open && self.state.minimenu.option_count <= 1 {
                    self.state.minimenu.close_popup();
                    self.state.menu.open = false;
                }
            }
        }
    }
    pub fn paint(
        &mut self,
        cycle: i32,
        scene_ready: bool,
        clear: [f32; 3],
    ) -> Result<crate::ui_backend::Output> {
        anyhow::ensure!(self.canvas_ready, "UI draw before canvas initialization");
        self.drawn_scene_delta = std::mem::take(&mut self.scene_delta);
        crate::ui_model_animation::interface(
            &mut self.store,
            &self.state,
            &self.frame,
            self.state.life.top,
            self.drawn_scene_delta,
        )?;
        if let Some(models) = &mut self.target.models {
            models.camera_planes = (self.engine.camera.cam2.camera_state == 3)
                .then_some(self.engine.camera.cam2.depth_planes);
        }
        self.target
            .begin(self.state.layout.canvas.map(|v| v as u32), clear);
        self.frame.client_state = 18;
        // sceneState / fade statics.
        self.frame.scene_state = if scene_ready {
            self.engine.camera.cutscene.scene_state
        } else {
            0
        };
        self.frame.fade = self.engine.camera.cutscene.fade.clone();
        self.state.life.redraw.fill(true);
        self.frame
            .promote(&mut self.store, &mut self.state, cycle)?;
        self.frame
            .root(&mut self.store, &mut self.state, &mut self.target)?;
        crate::ui_menu_render::paint(
            &mut self.target.leaf,
            &mut self.state,
            self.engine.platform.mouse,
        )?;
        self.renderer_settings
            .paint(&mut self.target.leaf, &self.state)?;
        Ok(self.target.finish())
    }
}

#[cfg(test)]
mod no_op_host_tests;

#[cfg(test)]
mod chat_host_tests;

#[cfg(test)]
mod player_target_tests;

#[cfg(test)]
mod menu_action_dispatch_tests;

#[cfg(test)]
mod scene_query_tests;
#[cfg(test)]
mod world_map_golden_tests;

/// Per-command behaviour tables for the host_builtins / host_game partitions.
#[cfg(test)]
#[path = "ui_command_spec.rs"]
mod command_spec;

#[cfg(test)]
mod chat_prefix_tests {
    use super::strip_chat_prefix_in;
    use crate::ui_text_compare::Language;
    use rs910_core::texts::Msg;

    const COLOURS: [Msg; 4] = [Msg::Chatcol0, Msg::Chatcol1, Msg::Chatcol2, Msg::Chatcol3];

    /// A chat prefix is read in English and in the client's language, in any
    /// case, and the rest of the text is cut by characters (the German
    /// spelling has an umlaut).
    #[test]
    fn chat_prefixes_follow_the_client_language() {
        let german = Language::De;
        assert_eq!(
            strip_chat_prefix_in(german, "BlauGrün:hallo", &COLOURS),
            Some((3, "hallo".into()))
        );
        assert_eq!(
            strip_chat_prefix_in(german, "rot:hi", &COLOURS),
            Some((1, "hi".into()))
        );
        assert_eq!(
            strip_chat_prefix_in(german, "red:hi", &COLOURS),
            Some((1, "hi".into()))
        );
        assert_eq!(strip_chat_prefix_in(german, "hello", &COLOURS), None);
        // A language without texts only has the English spellings.
        assert_eq!(
            strip_chat_prefix_in(Language::Nl, "green:x", &COLOURS),
            Some((2, "x".into()))
        );
        assert_eq!(strip_chat_prefix_in(Language::En, "rot:x", &COLOURS), None);
    }
}
