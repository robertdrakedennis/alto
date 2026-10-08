//! `ClientCore`: the client's logic state and its phase list
//! (code-quality programme Phase 5, target-architecture.md §3.1-§3.2).
//!
//! [`ClientCore::frame`] is the logic half of one frame and
//! [`ClientCore::mainloop`] one logic cycle,
//! as the named phase list below, in TODAY'S order (programme §1
//! invariant 6: the phase order is fixed by the replays and never changes as
//! part of a refactor). A phase is either the core's own (it touches only core state
//! and the [`Io`]) or a [`Shell`] method: the shell owns the window, the
//! toolkit and the scene's GPU caches, and the phases that write them run
//! there, usually around a core call (e.g. P8's live read is
//! [`ClientCore::poll_live`], its effects are applied by the shell).
//!
//! | phase | runs | covers | core part |
//! |---|---|---|---|
//! | P0 resize | [`Shell::resize`] (once per frame with logic steps) | the window-size update | — |
//! | P1 JS5 | [`Shell::js5_mainloop`] | the JS5 TCP client and its update | — |
//! | P2 loading | [`Shell::update_loading`], then the cycle stops | the loading update | — |
//! | P3 cycle | [`ClientCore::begin_cycle`], then [`Shell::cycle_inputs`] | the cycle counter; the developer console update | `loopCycle`, the cycle clock ([`Io::begin_cycle`]) |
//! | P4 rebuild | [`Shell::rebuild`] | the scene rebuild, before the game update | — |
//! | P5 session cycle | [`Shell::session_cycle`] → [`ClientCore::session_cycle`] | net stats, the debug overlay inputs, the mouse flip, client-watch telemetry | all (the shell lends the toolkit's memory query) |
//! | P6 input script | [`ClientCore::input_script`] | diagnostic injectors, then the game cycle | all |
//! | P7 connections | [`Shell::connections`] (→ [`ClientCore::inject_connection_drop`], [`ClientCore::poll_lobby`]) | the login update, the lobby connection read | the lobby read |
//! | P8 live read | [`Shell::live_read`] (→ [`ClientCore::poll_live`]), then [`ClientCore::cutscene_fixture`] | the packet read | the read and its dispatch; the effects are the shell's |
//! | P9 update game | [`Shell::update_game`] → [`ClientCore::update_game`] | the game update and the interface update | all (the shell lends its inputs and the pick refresh) |
//! | P10 requests | [`Shell::apply_effects`] | the requests the tick queued (`effects`) | produced by P9 |
//! | P11 title, prefs | [`Shell::title_and_preferences`] | the title-screen update, the account-creation update, preference sync, server commands | — |
//!
//! The redraw half is [`ClientCore::redraw`] (the `redraw` module): a frame
//! that granted logic cycles makes the next redraw a full redraw.
use super::*;

/// How one logic cycle ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cycle {
    /// Run the next granted cycle.
    Next,
    /// The loading states run one loading update per redraw.
    Loading,
    /// A fatal cheat or a script `quit` stops the event loop.
    Exit,
}

/// The phases of the list above, as [`Shell::phase`] observes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Resize,
    Js5,
    Loading,
    Cycle,
    Rebuild,
    SessionCycle,
    InputScript,
    Connections,
    LiveRead,
    UpdateGame,
    Effects,
    TitleAndPreferences,
}

/// The shell's half of the phase list: the phases that write the window,
/// the toolkit or the shell's scene owners, and the inputs it lends the
/// core. Implemented by the windowed client (`client910::app`) and the
/// headless client. The defaults are a shell with none of those owners.
pub trait Shell {
    /// The core the phases run on.
    fn core(&mut self) -> &mut ClientCore;
    /// Observation hook before each phase (tests); nothing by default.
    fn phase(&mut self, _phase: Phase) {}
    /// P0: the frame's last native size.
    fn resize(&mut self) {}
    /// P1: the JS5 clients.
    fn js5_mainloop(&mut self) {}
    /// P2: the loading update while a loading state owns the cycle; true when
    /// it ran (the cycle stops).
    fn update_loading(&mut self) -> bool {
        false
    }
    /// P3's shell half: the native-resize injector and
    /// the developer console update.
    fn cycle_inputs(&mut self) {}
    /// P4: the map transaction.
    fn rebuild(&mut self) {}
    /// P5: [`ClientCore::session_cycle`] with the toolkit's off-heap memory
    /// query.
    fn session_cycle(&mut self) {
        self.core().session_cycle(&|| None);
    }
    /// P7: the login workers and the lobby connection.
    fn connections(&mut self, _io: &mut dyn Io) {}
    /// P8: the world read and its effects; true when a fatal client cheat
    /// asked to shut down.
    fn live_read(&mut self, io: &mut dyn Io) -> bool;
    /// P9: [`ClientCore::update_game`] with the shell's inputs.
    fn update_game(&mut self, io: &mut dyn Io) -> Vec<ClientEffect>;
    /// P10: the requests in their drain order; true when a script asked to
    /// quit.
    fn apply_effects(&mut self, effects: Vec<ClientEffect>) -> bool;
    /// P11: the title world, the connect that waits for it, the preference
    /// owners and the queued commands.
    fn title_and_preferences(&mut self, after_title: Vec<ClientEffect>);
}

/// The client core: the state the logic phases own (the session owners and
/// the logic cycle counter), independent of the window, the toolkit and the
/// event loop. The shell holds one and drives it ([`ClientCore::frame`]).
pub struct ClientCore {
    /// Live session owners (`None` offline): the game and retained UI, the
    /// login machine and the connections (`Session`).
    pub session: Option<Session>,
    /// The logic cycle counter: logic cycles run so far.
    pub cycle: i32,
    /// `CLIENT910_CUTSCENE` fixture (diagnostic): the absolute tile the
    /// local player left.
    pub cutscene_fixture_return: Option<[i32; 3]>,
    /// The deterministic input injectors (`input_script.rs`): the process's
    /// `CLIENT910_*` set in the windowed client, a recording's in a replay.
    pub script: crate::input_script::InputScript,
    /// `CLIENT910_PROFILE`: the `[runtime]` line every 100 cycles (the
    /// shell's profiling switch).
    pub profile: bool,
    /// Positioned sound inputs across redraws (R8).
    pub audio: PositionedAudio,
    /// `EnvironmentManager`'s CPU half (P9 and R3).
    pub environment: EnvironmentManager,
    /// The minimap's flag tile, map flag and toggle: set at P9
    /// ([`ClientCore::take_minimap_flag`]), cleared on arrival by the full
    /// redraw ([`ClientCore::begin_draw_scene`]); the minimap draws it.
    pub minimap: rs910_scene::minimap::MinimapFlag,
    /// The scene cycle, incremented by each scene draw: the full redraws'
    /// scene count and the entities' draw-cycle stamp.
    pub scene_cycle: i32,
    /// A frame granted logic cycles since the last redraw: the next redraw
    /// is a full redraw ([`ClientCore::redraw`]).
    pub redraw_due: bool,
    /// A full redraw is drawing the cutscene's actors (they are swapped
    /// into the entity feed; the logic's are the cutscene slot's).
    pub drawing_cutscene: bool,
}

impl ClientCore {
    /// A core over `session`, before its first logic cycle, with the
    /// process's input injectors.
    pub fn new(session: Option<Session>) -> Self {
        Self {
            session,
            cycle: 0,
            cutscene_fixture_return: None,
            script: crate::client_debug_flags::flags().input.clone(),
            profile: false,
            audio: PositionedAudio::default(),
            environment: EnvironmentManager::default(),
            minimap: Default::default(),
            scene_cycle: 0,
            redraw_due: false,
            drawing_cutscene: false,
        }
    }

    /// The logic half of one frame: when the clock granted
    /// cycles, the frame's clock ([`Io::begin_frame`]) and P0, then one
    /// [`ClientCore::mainloop`] per granted cycle until a loading state
    /// takes the cycle or the client exits. A frame with cycles makes the
    /// next redraw a full redraw (one redraw after each turn's logic cycles).
    pub fn frame<S: Shell + ?Sized>(shell: &mut S, io: &mut dyn Io, logic_steps: i32) -> Cycle {
        rs910_core::profile::scope!("logic");
        if logic_steps > 0 {
            shell.core().redraw_due = true;
            let next = shell.core().cycle.wrapping_add(1);
            io.begin_frame(next);
            shell.phase(Phase::Resize);
            rs910_core::profile::scope!("P0 resize", shell.resize());
        }
        for _ in 0..logic_steps {
            match rs910_core::profile::scope!("logic cycle", Self::mainloop(shell, io)) {
                Cycle::Next => {}
                Cycle::Loading => return Cycle::Loading,
                Cycle::Exit => return Cycle::Exit,
            }
        }
        Cycle::Next
    }

    /// One logic cycle (the phase table above).
    pub fn mainloop<S: Shell + ?Sized>(shell: &mut S, io: &mut dyn Io) -> Cycle {
        shell.phase(Phase::Js5);
        rs910_core::profile::scope!("P1 js5", shell.js5_mainloop());
        // The loading states run only the loading update. The loading
        // screen is drawn between updates; here one
        // update per redraw keeps every stage on screen. The diagnostic
        // replay cycles below count from the end of loading.
        shell.phase(Phase::Loading);
        if rs910_core::profile::scope!("P2 loading", shell.update_loading()) {
            return Cycle::Loading;
        }
        shell.phase(Phase::Cycle);
        rs910_core::profile::scope!("P3 cycle", shell.core().begin_cycle(io));
        rs910_core::profile::scope!("P3 cycle inputs", shell.cycle_inputs());
        shell.phase(Phase::Rebuild);
        rs910_core::profile::scope!("P4 rebuild", shell.rebuild());
        shell.phase(Phase::SessionCycle);
        rs910_core::profile::scope!("P5 session cycle", shell.session_cycle());
        shell.phase(Phase::InputScript);
        rs910_core::profile::scope!("P6 input script", shell.core().input_script());
        shell.phase(Phase::Connections);
        rs910_core::profile::scope!("P7 connections", shell.connections(io));
        shell.phase(Phase::LiveRead);
        if rs910_core::profile::scope!("P8 live read", shell.live_read(io)) {
            return Cycle::Exit;
        }
        shell.core().cutscene_fixture();
        shell.phase(Phase::UpdateGame);
        let effects = rs910_core::profile::scope!("P9 update game", shell.update_game(io));
        // P10: the session's requests in their drain order (`effects`);
        // The account-creation update follows the title-screen update, so its
        // connect waits for P11.
        let (after_title, effects): (Vec<_>, Vec<_>) = effects
            .into_iter()
            .partition(ClientEffect::after_title_screen);
        shell.phase(Phase::Effects);
        if rs910_core::profile::scope!("P10 effects", shell.apply_effects(effects)) {
            return Cycle::Exit;
        }
        shell.phase(Phase::TitleAndPreferences);
        rs910_core::profile::scope!("P11 title prefs", shell.title_and_preferences(after_title));
        Cycle::Next
    }

    /// P3's core half: the cycle counter increments, then the
    /// cycle's clock ([`Io::begin_cycle`]: the recorder's cycle mark, a
    /// replay's recorded time).
    pub fn begin_cycle(&mut self, io: &mut dyn Io) {
        self.cycle = self.cycle.wrapping_add(1);
        io.begin_cycle(self.cycle);
    }

    /// P5: net stats, the drawDebug inputs, the mouse flip and the input telemetry
    /// routing ([`begin_session_cycle`]).
    /// `offheap_bytes` is the toolkit's off-heap memory query.
    pub fn session_cycle(&mut self, offheap_bytes: &dyn Fn() -> Option<u64>) {
        if let Some(session) = self.session.as_mut() {
            begin_session_cycle(session, self.cycle, offheap_bytes);
        }
    }

    /// P6: deterministic input injectors (input_script.rs), parsed once at
    /// startup and applied here in their former order: KEY_INPUT,
    /// TYPE_INPUT, WHEEL_INPUT, TOOLKIT_INPUT, UI_OPERATIONS, UI_INPUT,
    /// UI_CLICK, UI_HOVER, UI_CLICKS. They run after the engine's
    /// `mouse = pending_mouse` sample (P5) and before `ui.tick` consumes
    /// `input.event`/`input.click` (cf. `WindowEvent::MouseInput`). Then the
    /// game owner's cycle stamp and the `CLIENT910_PROFILE` runtime line.
    pub fn input_script(&mut self) {
        for injection in self.script.ui_injections(self.cycle) {
            if let Some(session) = self.session.as_mut() {
                apply_ui_injection(session, self.cycle, injection);
            }
        }
        if let Some(game) = self.session.as_mut().and_then(|s| s.game.as_mut()) {
            game.cycle = self.cycle;
            if self.cycle % 100 == 0 && self.profile {
                let base = game
                    .runtime
                    .feed
                    .state
                    .world
                    .as_ref()
                    .map(|w| (w.base_x, w.base_z))
                    .unwrap_or((game.runtime.map.base_x, game.runtime.map.base_z));
                let local = game.runtime.feed.state.players.players[game.runtime.map.local]
                    .as_ref()
                    .map(|p| (base.0 + p.x[0], base.1 + p.z[0], p.level));
                log::info!(
                    "[runtime] cycle={} packets={} players={} npcs={} local={:?} blocked={:?}",
                    game.cycle,
                    game.packets_applied,
                    game.runtime.feed.state.players.high_indices.len(),
                    game.runtime.feed.state.npcs.slots.len(),
                    local,
                    game.runtime.feed.blocked
                );
            }
        }
    }

    /// P7: diagnostic connection drop (`CLIENT910_DROP_CONNECTION=cycle`):
    /// shut down the active game (or lobby) socket at that logic cycle so
    /// the ordinary reader observes the I/O error path.
    pub fn inject_connection_drop(&mut self) {
        let Some(cycle) = self.script.drop_connection() else {
            return;
        };
        if cycle != self.cycle {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let stream = session
            .io
            .world
            .stream
            .as_ref()
            .or(session.io.lobby.stream.as_ref());
        if let Some(stream) = stream {
            log::info!(
                "[client910] drop-connection inject cycle={cycle} state={}",
                session.machine.state
            );
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// P7: the title-screen update: read the lobby connection
    /// through the same read owner as the game connection (varps,
    /// interface, social and world-list packets), in lobby state 13 while no
    /// login is in progress, and on the title account-creation connection.
    /// While the lobby is the connection the player is waiting on
    /// ([`lobby_connection_live`]) its keepalive runs: the interface-change
    /// count in the lobby, and a `NO_TIMEOUT` after fifty idle cycles.
    /// Returns the session events for the shell's login flow.
    pub fn poll_lobby(&mut self, io: &mut dyn Io) -> anyhow::Result<Vec<crate::session::UiEvent>> {
        let Some(session) = self.session.as_mut() else {
            return Ok(Vec::new());
        };
        publish_client_state(session);
        let state = session.machine.state;
        let live = lobby_connection_live(session);
        let readable = (state == crate::login_state::LOBBY && !session.reconnect_started)
            || crate::login_state::is_title(state);
        if !readable && !live {
            return Ok(Vec::new());
        }
        if session.io.lobby.stream.is_none() {
            return Ok(Vec::new());
        }
        if readable {
            session
                .io
                .lobby
                .pending_writes
                .extend(std::mem::take(&mut session.ui.engine.outgoing));
        }
        if live {
            if state == crate::login_state::LOBBY {
                queue_verify_id(&mut session.ui, &mut session.io.lobby.pending_writes);
            }
            session
                .io
                .lobby_idle_connection
                .tick(&mut session.io.lobby.pending_writes);
        }
        let stream = session
            .io
            .lobby
            .stream
            .as_mut()
            .expect("the lobby stream was checked");
        io.flush(
            Conn::Lobby,
            stream,
            &mut session.io.lobby.pending_writes,
            &mut |written| {
                session.io.lobby_idle_connection.wrote();
                session.io.net_stats[1].wrote(written.len());
            },
        )
        .map_err(|error| ConnectionLost(format!("lobby poll write: {error}")))?;
        if !readable {
            return Ok(Vec::new());
        }
        let closed = io
            .read(
                Conn::Lobby,
                stream,
                &mut session.io.lobby.pending,
                &mut |read| {
                    session.io.net_stats[1].read(read.len());
                },
            )
            .map_err(|error| ConnectionLost(format!("lobby poll read: {error}")))?;
        let mut session_events = Vec::new();
        let game = session
            .game
            .as_mut()
            .context("lobby reader needs the client runtime")?;
        drain_game_frames(
            game,
            &mut session.ui,
            &session.pack_root,
            &mut session.io.lobby,
            &mut session_events,
        )?;
        if closed && !session_events.iter().any(ends_connection) {
            return Err(ConnectionLost("lobby poll: server closed the connection".into()).into());
        }
        session
            .io
            .lobby
            .pending_writes
            .extend(std::mem::take(&mut session.ui.engine.outgoing));
        if let Some(stream) = session.io.lobby.stream.as_mut().filter(|_| !closed) {
            io.flush(
                Conn::Lobby,
                stream,
                &mut session.io.lobby.pending_writes,
                &mut |written| {
                    session.io.lobby_idle_connection.wrote();
                    session.io.net_stats[1].wrote(written.len());
                },
            )
            .map_err(|error| ConnectionLost(format!("lobby poll reply: {error}")))?;
        }
        Ok(session_events)
    }

    /// P8: one live poll of the world connection ([`poll_live_session`]);
    /// idle without a session.
    pub fn poll_live(&mut self, io: &mut dyn Io) -> LivePollOutcome {
        match self.session.as_mut() {
            Some(session) => poll_live_session(session, io),
            None => LivePollOutcome::Idle,
        }
    }

    /// P8's tail, diagnostic (`CLIENT910_CUTSCENE=id,cycle[,capacity]`): at
    /// logic cycle `cycle`, decode a CUTSCENE frame carrying the local
    /// player's appearance block through the normal server-packet parser and
    /// the same packet owner as the socket.
    pub fn cutscene_fixture(&mut self) {
        let script = &self.script;
        if let Some(fixture) = script.cutscene_fixture(self.cycle) {
            if let Some(session) = self.session.as_mut() {
                if let Some(game) = session.game.as_mut() {
                    let local = game.runtime.map.local;
                    let appearance = game
                        .runtime
                        .feed
                        .state
                        .players
                        .appearances
                        .get(local)
                        .and_then(Option::as_ref)
                        .map(|c| c.data.clone())
                        .unwrap_or_default();
                    let mut payload = Vec::new();
                    payload.extend((fixture.id as u16).to_be_bytes());
                    payload.extend((fixture.capacity.unwrap_or(64) as u16).to_be_bytes());
                    payload.push(appearance.len().min(255) as u8);
                    payload.extend(&appearance[..appearance.len().min(255)]);
                    if let Some(p) = game.runtime.feed.state.players.players[local].as_ref() {
                        self.cutscene_fixture_return = Some([
                            game.runtime.map.base_x + p.x[0],
                            game.runtime.map.base_z + p.z[0],
                            p.level,
                        ]);
                    }
                    match crate::session::parse_ui_event(crate::proto::server::CUTSCENE, &payload) {
                        Ok(Some(event)) => {
                            log::info!(
                                "[client910] cutscene fixture: CUTSCENE id {} at cycle {}",
                                fixture.id,
                                fixture.cycle
                            );
                            if let Err(error) =
                                ui_packet(&mut session.ui, &session.pack_root, Some(game), &event)
                            {
                                log::warn!("[client910] cutscene fixture packet: {error:#}");
                            }
                        }
                        other => log::info!("[client910] cutscene fixture decode: {other:?}"),
                    }
                }
            }
        }
        // The fixture's server side of a finished cutscene: CAM_RESET and
        // a `tele` cheat to the recorded position, whose REBUILD_NORMAL
        // returns the client to scene state 3.
        if script.cutscene_enabled() {
            if let Some(session) = self.session.as_mut() {
                let finished = session
                    .game
                    .as_ref()
                    .is_some_and(|g| g.cutscene.scene_state == 4);
                if finished {
                    if let Some([x, z, level]) = self.cutscene_fixture_return.take() {
                        log::info!(
                            "[client910] cutscene fixture: CAM_RESET + tele {x} {z} {level}"
                        );
                        if let Some(game) = session.game.as_mut() {
                            if let Err(error) = ui_packet(
                                &mut session.ui,
                                &session.pack_root,
                                Some(game),
                                &crate::session::UiEvent::CameraReset,
                            ) {
                                log::warn!("[client910] cutscene fixture CAM_RESET: {error:#}");
                            }
                        }
                        session
                            .server_commands
                            .push_back(format!("tele {x} {z} {level}"));
                    }
                }
            }
        }
    }

    /// P9: `update_session_logic` (updateGame, with the environment's
    /// `updatePartial`, + updateInterfaces) with the shell's `input`; the
    /// cursor it reads comes through [`Io::cursor`]. Then the minimap flag
    /// the tick's clicks set ([`ClientCore::take_minimap_flag`]). Returns
    /// the requests for P10/P11 (none without a session).
    pub fn update_game(
        &mut self,
        pack: &Pack,
        io: &mut dyn Io,
        input: InputFrame<'_>,
    ) -> Vec<ClientEffect> {
        let Some(session) = self.session.as_mut() else {
            return Vec::new();
        };
        let cursor = io.cursor(input.cursor);
        let effects = update_session_logic(
            session,
            pack,
            InputFrame { cursor, ..input },
            Some(&mut self.environment),
        );
        self.take_minimap_flag();
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell with no owners that logs the phase hook and its own methods.
    struct Logged {
        core: ClientCore,
        log: Vec<String>,
        loading: bool,
        exit_at_live_read: bool,
    }

    impl Logged {
        fn new() -> Self {
            Self {
                core: ClientCore::new(None),
                log: Vec::new(),
                loading: false,
                exit_at_live_read: false,
            }
        }
    }

    impl Shell for Logged {
        fn core(&mut self) -> &mut ClientCore {
            &mut self.core
        }
        fn phase(&mut self, phase: Phase) {
            self.log.push(format!("{phase:?}"));
        }
        fn update_loading(&mut self) -> bool {
            self.loading
        }
        fn live_read(&mut self, _io: &mut dyn Io) -> bool {
            self.exit_at_live_read
        }
        fn update_game(&mut self, _io: &mut dyn Io) -> Vec<ClientEffect> {
            vec![ClientEffect::CreateConnect, ClientEffect::Logout]
        }
        fn apply_effects(&mut self, effects: Vec<ClientEffect>) -> bool {
            self.log.push(format!("p10:{}", effects.len()));
            false
        }
        fn title_and_preferences(&mut self, after_title: Vec<ClientEffect>) {
            let connect = matches!(after_title[..], [ClientEffect::CreateConnect]);
            self.log.push(format!("p11:{connect}"));
        }
    }

    fn clock(cycle: i32, now: i64) -> crate::session_record::Record {
        crate::session_record::Record {
            cycle,
            tag: *b"NOWM",
            bytes: now.to_le_bytes().to_vec(),
        }
    }

    /// `ClientCore::frame` runs P0 once per frame with logic steps, then the
    /// P1-P11 list once per cycle in today's order, counting `loopCycle`,
    /// with each cycle's clock from the `Io`; `CreateConnect` waits for P11.
    #[test]
    fn frame_runs_the_phase_list_in_order_with_the_io_clock() {
        let mut shell = Logged::new();
        let mut io = ReplayIo::new(vec![clock(1, 5000), clock(2, 5020)]);
        assert_eq!(ClientCore::frame(&mut shell, &mut io, 0), Cycle::Next);
        assert!(shell.log.is_empty(), "no granted cycle, no phase");
        assert_eq!(ClientCore::frame(&mut shell, &mut io, 2), Cycle::Next);
        let cycle = [
            "Js5",
            "Loading",
            "Cycle",
            "Rebuild",
            "SessionCycle",
            "InputScript",
            "Connections",
            "LiveRead",
            "UpdateGame",
            "Effects",
            "p10:1",
            "TitleAndPreferences",
            "p11:true",
        ];
        let expected: Vec<&str> = std::iter::once("Resize")
            .chain(cycle)
            .chain(cycle)
            .collect();
        assert_eq!(shell.log, expected);
        assert_eq!(shell.core.cycle, 2);
        assert_eq!(crate::logic_clock::monotonic_millis(), 5020);
        crate::logic_clock::set_test_now(None);
    }

    /// The loading states take the cycle (the loading update, the frame
    /// stops); a fatal cheat at P8 exits before the logic update.
    #[test]
    fn loading_and_exit_stop_the_frame() {
        let mut shell = Logged::new();
        shell.loading = true;
        let mut io = ReplayIo::new(Vec::new());
        crate::logic_clock::set_test_now(Some(0));
        assert_eq!(ClientCore::frame(&mut shell, &mut io, 3), Cycle::Loading);
        assert_eq!(shell.log, ["Resize", "Js5", "Loading"]);
        assert_eq!(
            shell.core.cycle, 0,
            "loopCycle waits for the loading states"
        );
        let mut shell = Logged::new();
        shell.exit_at_live_read = true;
        assert_eq!(ClientCore::frame(&mut shell, &mut io, 3), Cycle::Exit);
        assert_eq!(shell.log.last().map(String::as_str), Some("LiveRead"));
        assert_eq!(shell.core.cycle, 1);
        crate::logic_clock::set_test_now(None);
    }
}
