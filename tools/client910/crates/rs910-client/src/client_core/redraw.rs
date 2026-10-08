//! The frame redraw, at the client's fixed cadence (lane E-A1; code-quality
//! programme Phase 5 split the steps).
//!
//! **Cadence.** The game loop runs the logic steps the timer grants and then
//! exactly one redraw; the timer sleeps until a cycle is due (it grants at
//! least one), so every full redraw follows at least one logic cycle. The windowed shell
//! redraws on every winit `RedrawRequested`: a new logic cycle requests one,
//! while resize/expose events can request additional presentations. The event
//! loop waits until the existing logic timer's next deadline between cycles. [`ClientCore::redraw`] therefore runs a **full redraw** — every
//! step that writes game, UI, audio or scene state
//! ([`ClientCore::full_redraw_state`]) — for the first redraw after a frame
//! that granted logic cycles ([`ClientCore::frame`] sets the due flag), and
//! every other redraw is a **present**: the shell draws the last full
//! redraw's frame again and writes nothing, so the number of presents
//! between cycles is not observable (`present_rate_is_observationally_inert`,
//! client910 `session_replay`). With the fixed clock (one cycle per turn)
//! every redraw is a full redraw, as before.
//!
//! | step | method | covers | runs |
//! |---|---|---|---|
//! | frame | [`ClientCore::redraw`] | the redraw entry; the cutscene's actors are the drawn ones while the scene state is 0 | every redraw (the swap only in a full redraw) |
//! | R0 loading | [`RedrawShell::loading_frame`] | the loading screen | every redraw |
//! | R1 keep-alive | [`RedrawShell::keepalive`] | (the first frame's shader/model preparation blocks the event loop) | every redraw |
//! | R2 gate | [`ClientCore::redraw_gate`], [`RedrawShell::message_box`] | the message box in the rebuild states; nothing during a map transaction | every redraw |
//! | R3 environment | [`ClientCore::environment_frame`], [`RedrawShell::environment`] | the environment update the scene draw runs (P9 runs the game-update call) | full redraw |
//! | R5 overlays | [`ClientCore::cross_state`], [`RedrawShell::scene_overlays`] | the 2D entity elements, cover markers, scene text, the minimap (reads [`ClientCore::minimap`], set at P9) | full redraw |
//! | R6 particles | [`RedrawShell::tick_particles`] | the particle systems' tick | full redraw |
//! | R7 interfaces | [`RedrawShell::paint_interfaces`] | the interface pass, the message box in states 14/19 | full redraw |
//! | R8 positioned sound | [`ClientCore::positioned_sound_frame`] | positioned (world) sound update | full redraw |
//! | R9 view | [`RedrawShell::update_view`] | the follow camera, on the redraw's elapsed time | full redraw |
//! | drawScene | [`ClientCore::begin_draw_scene`] | the scene-cycle increment and the minimap flag's arrival | full redraw |
//! | R10 actors | [`RedrawShell::place_actors`] | actor placement in the scene | full redraw |
//! | R11 entities | [`RedrawShell::build_scene_entities`] | the entity models the scene draws; the NPC bounds and draw-cycle stamp | full redraw |
//! | R12 bodies | [`RedrawShell::refresh_player_bodies`] | the drawn scene's picking | full redraw |
//! | R13 frame resources | [`RedrawShell::prepare_frame_resources`] | material animation, particle update, skybox | full redraw |
//! | R14 present | [`RedrawShell::present`] | the scene's toolkit handoff | every redraw |
//!
//! The zone loc changes and the minimap flag are logic (P9,
//! `update_session_logic` and [`ClientCore::update_game`]); the environment
//! runs at P9 and R3.
use super::*;

/// Positioned-sound inputs: a finished map load's loc sounds and the varp
/// serial of the last positioned-sound refresh.
#[derive(Default)]
pub struct PositionedAudio {
    /// A finished map load's loc sounds, installed into
    /// the positioned-sound owner on the next drawn frame.
    pub pending_loc_sounds: Option<crate::positioned_sound::LocSoundScene>,
    /// `Game::varp_serial` at the last positioned-sound refresh.
    pub varp_serial: u64,
}

/// What one redraw is ([`ClientCore::redraw`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedrawKind {
    /// The first redraw after a frame with logic cycles runs the
    /// state-writing steps and draws their frame.
    Full,
    /// Any other redraw: the last full redraw's frame drawn again, nothing
    /// written.
    Present,
}

/// The redraw's steps, as [`RedrawShell::step`] observes them (the table
/// above).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedrawStep {
    Loading,
    Keepalive,
    Gate,
    Environment,
    SceneOverlays,
    Particles,
    Interfaces,
    PositionalSound,
    View,
    DrawScene,
    Actors,
    SceneEntities,
    PlayerBodies,
    FrameResources,
    Present,
}

/// The redraw's state switch for the scene frame (R2).
pub enum RedrawGate<'a> {
    /// The rebuild states draw the message-box loading text (world
    /// progress).
    Rebuild(&'a crate::login_state::RebuildProgress),
    /// A pending map transaction outside them presents nothing: rebased
    /// actors do not belong to the old scene.
    MapTransaction,
    /// The frame draws.
    Draw,
}

/// The shell's half of the redraw: the steps that draw, or that write the
/// shell's scene, particle, camera and GPU owners (the table above). The
/// windowed client (`client910::app`) implements every step; the headless
/// client none (the defaults). A step keeps what a later one needs (the
/// environment frame, the overlays) itself.
pub trait RedrawShell {
    /// The core the redraw reads.
    fn core(&mut self) -> &mut ClientCore;
    /// Observation hook before each step (tests); nothing by default.
    fn step(&mut self, _step: RedrawStep) {}
    /// R0: the loading states draw the loading screen; true when it did
    /// (the redraw ends).
    fn loading_frame(&mut self) -> bool {
        false
    }
    /// R1: keep the connection alive through a blocking first frame until
    /// [`RedrawShell::end_frame`].
    fn keepalive(&mut self) {}
    /// R2: the message box over the retained frame in the rebuild states.
    fn message_box(&mut self, _progress: crate::login_state::RebuildProgress) {}
    /// R3: the camera's window tile, the environment target's position
    /// in the title/lobby states and offline ([`ClientCore::environment_tile`]).
    fn camera_tile(&mut self) -> [i32; 2] {
        [-8, -8]
    }
    /// R3: the frame's sun and fog from `currentEnv` and its sun direction
    /// ([`ClientCore::environment_frame`]'s result).
    fn environment(&mut self, _current: (rs910_scene::env::Environment, [f32; 3])) {}
    /// R5: the 2D scene elements, cover markers, scene text and minimap.
    fn scene_overlays(&mut self) {}
    /// R6: the particle systems' tick.
    fn tick_particles(&mut self) {}
    /// R7: the interface pass; false when a UI error stops the frame.
    fn paint_interfaces(&mut self) -> bool {
        true
    }
    /// R9: the lens profile, viewport, console layer and follow camera.
    fn update_view(&mut self) {}
    /// R10: actor placement in the scene.
    fn place_actors(&mut self) {}
    /// R11: the entity models and their write-back.
    fn build_scene_entities(&mut self) {}
    /// R12: the player/NPC bodies and the pick frame the next UI tick reads.
    fn refresh_player_bodies(&mut self) {}
    /// R13: material animation, particle update, skybox.
    fn prepare_frame_resources(&mut self) {}
    /// R14: draw this full redraw's frame, or ([`RedrawKind::Present`]) the
    /// last one again.
    fn present(&mut self, _kind: RedrawKind) {}
    /// The redraw's end (the R1 keep-alive drops).
    fn end_frame(&mut self) {}
}

impl ClientCore {
    /// True when the next redraw is a full redraw (a frame granted logic
    /// cycles since the last one): the fps ring and the cursor sync run then.
    pub fn redraw_due(&self) -> bool {
        self.redraw_due
    }

    /// Mark the next redraw as a full redraw ([`ClientCore::frame`] after
    /// logic cycles; tests).
    pub fn request_full_redraw(&mut self) {
        self.redraw_due = true;
    }

    /// One redraw over `shell` (the table above): a full redraw when one is
    /// due, else a present.
    pub fn redraw<S: RedrawShell + ?Sized>(shell: &mut S) -> RedrawKind {
        let kind = if std::mem::take(&mut shell.core().redraw_due) {
            RedrawKind::Full
        } else {
            RedrawKind::Present
        };
        // While a cutscene draws (scene state 0, no map transaction) its
        // actors are the drawn ones for the full redraw (the current player
        // level stays the local player's); a present reads no actors.
        let cutscene = kind == RedrawKind::Full && shell.core().swap_cutscene_actors(true);
        shell.core().drawing_cutscene = cutscene;
        Self::redraw_steps(shell, kind);
        if cutscene {
            shell.core().swap_cutscene_actors(false);
        }
        shell.core().drawing_cutscene = false;
        shell.end_frame();
        kind
    }

    fn redraw_steps<S: RedrawShell + ?Sized>(shell: &mut S, kind: RedrawKind) {
        shell.step(RedrawStep::Loading);
        if rs910_core::profile::scope!("R0 loading", shell.loading_frame()) {
            return;
        }
        shell.step(RedrawStep::Keepalive);
        rs910_core::profile::scope!("R1 keep-alive", shell.keepalive());
        shell.step(RedrawStep::Gate);
        let progress = match rs910_core::profile::scope!("R2 gate", shell.core().redraw_gate()) {
            RedrawGate::Rebuild(progress) => Some(*progress),
            RedrawGate::MapTransaction => return,
            RedrawGate::Draw => None,
        };
        if let Some(progress) = progress {
            rs910_core::profile::scope!("R2 message box", shell.message_box(progress));
            return;
        }
        if kind == RedrawKind::Full
            && !rs910_core::profile::scope!("full redraw", Self::full_redraw_state(shell))
        {
            return;
        }
        shell.step(RedrawStep::Present);
        rs910_core::profile::scope!("R14 present", shell.present(kind));
    }

    /// The full redraw's state-writing steps, R3-R13 (the table above),
    /// once per frame with logic cycles; false when the interface pass
    /// stopped the frame (nothing presents).
    pub fn full_redraw_state<S: RedrawShell + ?Sized>(shell: &mut S) -> bool {
        shell.step(RedrawStep::Environment);
        let camera_tile = shell.camera_tile();
        let current = rs910_core::profile::scope!(
            "R3 environment core",
            shell.core().environment_frame(camera_tile)
        );
        rs910_core::profile::scope!("R3 environment", shell.environment(current));
        shell.step(RedrawStep::SceneOverlays);
        rs910_core::profile::scope!("R5 cross state", shell.core().cross_state());
        rs910_core::profile::scope!("R5 scene overlays", shell.scene_overlays());
        shell.step(RedrawStep::Particles);
        rs910_core::profile::scope!("R6 particle tick", shell.tick_particles());
        shell.step(RedrawStep::Interfaces);
        if !rs910_core::profile::scope!("R7 interfaces", shell.paint_interfaces()) {
            return false;
        }
        shell.step(RedrawStep::PositionalSound);
        rs910_core::profile::scope!("R8 positioned sound", shell.core().positioned_sound_frame());
        shell.step(RedrawStep::View);
        rs910_core::profile::scope!("R9 view", shell.update_view());
        shell.step(RedrawStep::DrawScene);
        rs910_core::profile::scope!("R10 begin draw scene", shell.core().begin_draw_scene());
        shell.step(RedrawStep::Actors);
        rs910_core::profile::scope!("R10 actor placement", shell.place_actors());
        shell.step(RedrawStep::SceneEntities);
        rs910_core::profile::scope!("R11 scene entities", shell.build_scene_entities());
        shell.step(RedrawStep::PlayerBodies);
        rs910_core::profile::scope!("R12 player bodies", shell.refresh_player_bodies());
        shell.step(RedrawStep::FrameResources);
        rs910_core::profile::scope!("R13 frame resources", shell.prepare_frame_resources());
        true
    }

    fn swap_cutscene_actors(&mut self, entering: bool) -> bool {
        let Some(game) = self.session.as_mut().and_then(|s| s.game.as_mut()) else {
            return false;
        };
        if entering
            && (game.game.cutscene.scene_state != 0 || game.game.runtime.map_request.is_some())
        {
            return false;
        }
        let state = &mut game.game.runtime.feed.state;
        if entering {
            // The current player level stays the local player's.
            game.game.cutscene.players.current_level = state.players.current_level;
        }
        std::mem::swap(&mut state.players, &mut game.game.cutscene.players);
        std::mem::swap(&mut state.npcs, &mut game.game.cutscene.npcs);
        true
    }

    /// R2: the rebuild states draw the message-box loading text over the
    /// retained frame instead of the game scene; a
    /// pending map transaction outside them presents nothing.
    pub fn redraw_gate(&self) -> RedrawGate<'_> {
        if let Some(session) = self
            .session
            .as_ref()
            .filter(|s| crate::login_state::is_rebuild(s.machine.state))
        {
            return RedrawGate::Rebuild(&session.rebuild);
        }
        let transaction = self
            .session
            .as_ref()
            .and_then(|s| s.game.as_ref())
            .is_some_and(|g| g.runtime.map_request.is_some());
        if transaction {
            RedrawGate::MapTransaction
        } else {
            RedrawGate::Draw
        }
    }

    /// R5's core half: the walk click's cross state for the cross sprites
    /// (the paint target's copy of the minimenu's).
    pub fn cross_state(&mut self) {
        if let Some(session) = self.session.as_mut() {
            let ui = &mut session.ui;
            ui.target.cross = ui.engine.menu.cross;
        }
    }

    /// P9's tail: the minimap flag at click time (the flag the UI tick
    /// queued becomes the minimap's scene-tile flag and the map flag clears)
    /// and the minimap toggle (`MINIMAP_TOGGLE`) as the packets left it.
    pub fn take_minimap_flag(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let ui = &mut session.ui;
        self.minimap.toggle = ui.engine.minimap.toggle;
        if let Some(tile) = ui.engine.minimap.flag.take() {
            self.minimap.set(tile);
        }
    }

    /// The scene draw's start: the scene-cycle increment (the entities'
    /// draw-cycle stamp, the 2D element test) and the minimap flag's arrival
    /// test for the local player. It also runs when the scene is blacked out
    /// (the entity steps do too).
    pub fn begin_draw_scene(&mut self) {
        self.scene_cycle = self.scene_cycle.wrapping_add(1);
        if let Some(session) = self.session.as_mut() {
            // The connection's silence only counts after a few draws of the state.
            session.machine.state_ticks = session.machine.state_ticks.saturating_add(1);
        }
        let Some(game) = self.session.as_ref().and_then(|s| s.game.as_ref()) else {
            return;
        };
        let players = if self.drawing_cutscene {
            &game.cutscene.players
        } else {
            &game.runtime.feed.state.players
        };
        if let Some(local) = players
            .players
            .get(game.runtime.map.local)
            .and_then(Option::as_ref)
        {
            self.minimap
                .arrive([local.fine_x as i32, local.fine_z as i32], local.size);
        }
    }

    /// The positioned-sound update for the local player's level, run after
    /// the interfaces and minimenu are drawn, preceded by the packet/map-load
    /// hooks derived from the retained state: a finished map load's ground
    /// loc sounds, loc change requests, the NPC list, player appearances and
    /// the client-variable refresh.
    pub fn positioned_sound_frame(&mut self) {
        use crate::positioned_sound::{
            motion, BasMotion, FrameInput, LocRequest, NpcSample, NpcSound, PlayerSample,
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let ui = &mut session.ui;
        let Some(game) = session.game.as_ref() else {
            return;
        };
        let state = &game.runtime.feed.state;
        if !state.initialized || game.runtime.map_request.is_some() {
            return;
        }
        let Some(local) = state
            .players
            .players
            .get(game.runtime.map.local)
            .and_then(Option::as_ref)
        else {
            return;
        };
        // The local player's variable state (multi-NPC and BAS resolution).
        let read = |bit: bool, id: i32| -> Option<i32> {
            let varps = state.varps.as_ref()?;
            if bit {
                varps.get_bit(&game.inputs.bits.get(id, false).ok()?).ok()
            } else {
                varps.get(id).ok()
            }
        };
        // Multi-loc domain: the cutscene var overrides while the scene state is 0.
        let overrides = (game.cutscene.scene_state == 0).then_some(&game.cutscene.var_overrides);
        let read_loc = |bit: bool, id: i32| -> Option<i32> {
            let key = if bit {
                i64::from(id) | 0x1_0000_0000
            } else {
                i64::from(id)
            };
            if let Some(&value) = overrides.and_then(|o| o.get(&key)) {
                return Some(value);
            }
            read(bit, id)
        };
        let bas = |id: i32| {
            game.inputs
                .bas
                .get(&id)
                .map_or(BasMotion::DEFAULT, BasMotion::from_bas)
        };
        let types = &game.inputs.appearance.types.npcs;
        let mut npcs = Vec::new();
        for index in &state.npcs.slots {
            let Some(npc) = state.npcs.entities.get(index) else {
                continue;
            };
            let Some(base) = types.get(&npc.type_id) else {
                continue;
            };
            // Multi-NPC resolution; a missing file lists as the default type.
            let resolved = match &base.multinpc {
                None => Some(NpcSound::from_type(base)),
                Some(list) => {
                    crate::config::select_multi(base.multivarbit, base.multivarp, list, &read).map(
                        |id| {
                            types.get(&(id as i32)).map_or_else(
                                || {
                                    NpcSound::from_type(
                                        &crate::protocol910::config_types::Npc::empty(id as i32),
                                    )
                                },
                                NpcSound::from_type,
                            )
                        },
                    )
                }
            };
            // The NPC's BAS id.
            let bas_id = if npc.bas_override != -1 {
                npc.bas_override
            } else {
                base.multinpc
                    .as_ref()
                    .and_then(|list| {
                        crate::config::select_multi(base.multivarbit, base.multivarp, list, &read)
                    })
                    .and_then(|id| types.get(&(id as i32)))
                    .map(|t| t.bas)
                    .filter(|&bas| bas != -1)
                    .unwrap_or(base.bas)
            };
            let walk = &npc.path.actor.walk;
            npcs.push(NpcSample {
                index: *index,
                type_id: npc.type_id,
                has_background_sound: base.has_background_sound(types),
                multinpc: base.multinpc.is_some(),
                resolved,
                level: npc.path.level,
                tile: [npc.path.x[0], npc.path.z[0]],
                trans: [npc.path.fine_x, npc.path.fine_z],
                size: npc.path.size,
                motion: motion(&bas(bas_id), walk.node.id(), walk.idle),
            });
        }
        let mut players = Vec::new();
        for (index, player) in state.players.players.iter().enumerate() {
            let Some(player) = player else {
                continue;
            };
            let walk = &player.actor.walk;
            players.push(PlayerSample {
                index,
                range: player.appearance.sound_range,
                sounds: player.appearance.sound_ids,
                volume: player.appearance.sound_volume,
                level: player.level,
                tile: [player.x[0], player.z[0]],
                trans: [player.fine_x, player.fine_z],
                size: player.size,
                motion: motion(&bas(player.appearance.bas), walk.node.id(), walk.idle),
            });
        }
        let requests: Vec<LocRequest> = state
            .zones
            .locations
            .iter()
            .map(|r| LocRequest {
                level: r.level,
                layer: r.layer,
                x: r.x,
                z: r.z,
                old_id: r.old_id,
                old_angle: r.old_angle,
                id: r.id,
                angle: r.angle,
                remove: r.remove,
            })
            .collect();
        let rebuilt = self.audio.pending_loc_sounds.take();
        let varps_changed = game.varp_serial != self.audio.varp_serial;
        self.audio.varp_serial = game.varp_serial;
        let textures = game
            .ui_variables
            .queries
            .preferences
            .options
            .get("textures")
            .unwrap_or(1)
            != 0;
        ui.audio.positioned_frame(&FrameInput {
            rebuilt: rebuilt.as_ref(),
            loc_requests: &requests,
            map_size: [game.runtime.map.width, game.runtime.map.height],
            textures,
            varps_changed,
            read_loc: &read_loc,
            level: local.level,
            scene_delta: ui.drawn_scene_delta,
            npcs: &npcs,
            players: &players,
        });
        if let Some(scene) = &rebuilt {
            log::info!(
                "[client910] positioned sounds: map load added {} loc sounds ({} loc types with sound)",
                ui.audio.positioned.locs.len(),
                scene.table.len()
            );
        }
        let trace = crate::client_debug_flags::flags().posaudio_trace_every;
        if let Some(every) = trace {
            if self.cycle % every == 0 {
                ui.audio
                    .trace_positioned(self.cycle, local.level, [local.fine_x, local.fine_z]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell with no owners that logs the redraw steps and what it drew.
    struct Logged {
        core: ClientCore,
        log: Vec<String>,
        loading: bool,
        ui_error: bool,
    }

    impl Logged {
        fn new() -> Self {
            Self {
                core: ClientCore::new(None),
                log: Vec::new(),
                loading: false,
                ui_error: false,
            }
        }
        fn take(&mut self) -> Vec<String> {
            std::mem::take(&mut self.log)
        }
    }

    impl Shell for Logged {
        fn core(&mut self) -> &mut ClientCore {
            &mut self.core
        }
        fn live_read(&mut self, _io: &mut dyn Io) -> bool {
            false
        }
        fn update_game(&mut self, _io: &mut dyn Io) -> Vec<ClientEffect> {
            Vec::new()
        }
        fn apply_effects(&mut self, _effects: Vec<ClientEffect>) -> bool {
            false
        }
        fn title_and_preferences(&mut self, _after_title: Vec<ClientEffect>) {}
    }

    impl RedrawShell for Logged {
        fn core(&mut self) -> &mut ClientCore {
            &mut self.core
        }
        fn step(&mut self, step: RedrawStep) {
            self.log.push(format!("{step:?}"));
        }
        fn loading_frame(&mut self) -> bool {
            self.loading
        }
        fn paint_interfaces(&mut self) -> bool {
            !self.ui_error
        }
        fn present(&mut self, kind: RedrawKind) {
            self.log.push(format!("present:{kind:?}"));
        }
    }

    const FULL_REDRAW: [&str; 16] = [
        "Loading",
        "Keepalive",
        "Gate",
        "Environment",
        "SceneOverlays",
        "Particles",
        "Interfaces",
        "PositionalSound",
        "View",
        "DrawScene",
        "Actors",
        "SceneEntities",
        "PlayerBodies",
        "FrameResources",
        "Present",
        "present:Full",
    ];
    const PRESENT: [&str; 5] = ["Loading", "Keepalive", "Gate", "Present", "present:Present"];

    /// Pins the redraw cadence (one redraw after each turn's logic cycles):
    /// the first redraw after a frame that granted logic cycles runs the whole
    /// step list once (scene cycle +1), however many cycles the frame ran; every
    /// other redraw — before the first cycle, a second redraw of the same
    /// frame, a frame without cycles — is a present (R0-R2, then the kept
    /// frame). The fixed clock grants one cycle per frame, so there every
    /// redraw is a full redraw, as before the split.
    #[test]
    fn one_full_redraw_follows_each_frame_with_logic_cycles() {
        let mut shell = Logged::new();
        let mut io = ReplayIo::new(Vec::new());
        crate::logic_clock::set_test_now(Some(0));
        assert_eq!(ClientCore::redraw(&mut shell), RedrawKind::Present);
        assert_eq!(shell.take(), PRESENT, "no logic cycle yet");
        assert_eq!(ClientCore::frame(&mut shell, &mut io, 2), Cycle::Next);
        assert!(shell.core.redraw_due());
        assert_eq!(ClientCore::redraw(&mut shell), RedrawKind::Full);
        assert_eq!(shell.take(), FULL_REDRAW);
        assert_eq!(shell.core.scene_cycle, 1);
        for _ in 0..2 {
            assert_eq!(ClientCore::redraw(&mut shell), RedrawKind::Present);
            assert_eq!(shell.take(), PRESENT, "a second redraw of the frame");
        }
        assert_eq!(ClientCore::frame(&mut shell, &mut io, 0), Cycle::Next);
        assert_eq!(ClientCore::redraw(&mut shell), RedrawKind::Present);
        assert_eq!(shell.take(), PRESENT, "a frame without cycles");
        // The fixed clock: one cycle per frame, one full redraw each.
        for scene_cycle in 2..=4 {
            assert_eq!(ClientCore::frame(&mut shell, &mut io, 1), Cycle::Next);
            assert_eq!(ClientCore::redraw(&mut shell), RedrawKind::Full);
            assert_eq!(shell.take(), FULL_REDRAW);
            assert_eq!(shell.core.scene_cycle, scene_cycle);
        }
        assert_eq!(shell.core.cycle, 5);
        crate::logic_clock::set_test_now(None);
    }

    /// A full redraw that ends early still uses up the frame's redraw: the
    /// loading states draw only the loading screen (R0), and a UI error at R7
    /// stops the frame before the scene draw (no scene-cycle increment, nothing
    /// presented); the next redraw of the frame is a present.
    #[test]
    fn an_early_exit_still_ends_the_full_redraw() {
        let mut shell = Logged::new();
        let mut io = ReplayIo::new(Vec::new());
        crate::logic_clock::set_test_now(Some(0));
        shell.loading = true;
        ClientCore::frame(&mut shell, &mut io, 1);
        assert_eq!(ClientCore::redraw(&mut shell), RedrawKind::Full);
        assert_eq!(shell.take(), ["Loading"]);
        assert!(!shell.core.redraw_due());
        shell.loading = false;
        shell.ui_error = true;
        ClientCore::frame(&mut shell, &mut io, 1);
        assert_eq!(ClientCore::redraw(&mut shell), RedrawKind::Full);
        assert_eq!(shell.take(), FULL_REDRAW[..7]);
        assert_eq!(shell.core.scene_cycle, 0, "drawScene did not start");
        assert_eq!(ClientCore::redraw(&mut shell), RedrawKind::Present);
        assert_eq!(shell.take(), PRESENT);
        crate::logic_clock::set_test_now(None);
    }
}
