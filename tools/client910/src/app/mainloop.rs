//! The shell's half of the logic loop (code-quality programme Phase 4.4,
//! Phase 5): `ViewerApp::about_to_wait` is the logic half of the frame loop,
//! the frame clock (F0), `rs910_client::client_core::ClientCore::frame` over
//! [`Windowed`] (P0-P11: the phase table and its order are the core's,
//! `client_core/phases.rs`), then the frame tail (F1).
//!
//! [`Windowed`] is the windowed client's `Shell`: the phases that write the
//! window, the toolkit, the scene's GPU owners, the console or the login
//! flow, each around the core call it wraps.
//!
//! | phase | shell method | writes | borrows |
//! |---|---|---|---|
//! | F0 frame clock | [`FrameClock::begin_frame`] | logic steps | `clock` |
//! | P0 resize | [`ViewerApp::resize_phase`] | the pending native resize | `input`, `view`, `renderer`, `window`, session prefs |
//! | P1 JS5 | `js5_mainloop` | the JS5 connection pump and the JS5 client update | `js5`, session UI, `lifecycle` |
//! | P2 loading | `update_loading` | the loading sequence update | the whole app |
//! | P3 (shell half) | [`ViewerApp::cycle_inputs_phase`] | the `CLIENT910_WINDOW_RESIZES` injector and the console input update | `window`, `console` |
//! | P4 rebuild | [`ViewerApp::rebuild_phase`] | the scene rebuild | session, `js5_map_wait`, scene owners |
//! | P5 session cycle | [`ViewerApp::session_cycle_phase`] | lends `renderer.allocated_bytes` | `core`, `renderer` |
//! | P7 connections | [`ViewerApp::connections_phase`] | the login connection update, the lobby read's events | session and the login flow |
//! | P8 live read | `poll_live_rebuild` | the packet effects (scene, environment, minimap, login flow) | session and the effects' owners |
//! | P9 update game | [`ViewerApp::update_game_phase`] | lends focus, console, texture formats, cursor and the renderer's pick refresh | `core`, `entities.players`, `scene`, `input`, `console`, `renderer` |
//! | P10 requests | `apply_effects` | the requests the tick queued | per effect |
//! | P11 title, prefs, console | [`ViewerApp::title_and_preferences_phase`] | `updateTitleScreen`, preference/toolkit sync, console replay, server commands | per step |
//! | F1 frame tail | [`ViewerApp::frame_tail`] | free-camera update, window title, `--screenshot` exit, `requestRedraw` | `view`, `clock`, `diag`, `window` |
//!
//! Before P0, a frame with logic cycles ends the last full redraw's
//! presentation ([`ViewerApp::end_presentation`]: its temporary scene
//! entities leave the scene the logic reads), and `ClientCore::frame` marks
//! the next redraw a full redraw.
//!
//! The core's own phases (P3's `loopCycle`, P6 input script, P8's
//! read/dispatch and the cutscene fixture, P9's logic) are
//! `ClientCore` methods. The redraw (`mainredraw`) is
//! `ViewerApp::render_frame` (`ClientCore::redraw`, app/redraw.rs).
use super::*;

/// The windowed client's `Shell` for one frame: the app and its event loop.
pub(super) struct Windowed<'a> {
    pub(super) app: &'a mut ViewerApp,
    pub(super) event_loop: &'a ActiveEventLoop,
}

impl Shell for Windowed<'_> {
    fn core(&mut self) -> &mut ClientCore {
        &mut self.app.core
    }
    fn resize(&mut self) {
        self.app.resize_phase();
    }
    fn js5_mainloop(&mut self) {
        self.app.js5_mainloop(self.event_loop);
    }
    fn update_loading(&mut self) -> bool {
        if self.app.lifecycle.loading.is_none() {
            return false;
        }
        self.app.update_loading(self.event_loop);
        true
    }
    fn cycle_inputs(&mut self) {
        self.app.cycle_inputs_phase();
    }
    fn rebuild(&mut self) {
        self.app.rebuild_phase();
    }
    fn session_cycle(&mut self) {
        self.app.session_cycle_phase();
    }
    fn connections(&mut self, io: &mut dyn Io) {
        self.app.connections_phase(io);
    }
    fn live_read(&mut self, io: &mut dyn Io) -> bool {
        self.app.live_read_phase(io)
    }
    fn update_game(&mut self, io: &mut dyn Io) -> Vec<ClientEffect> {
        self.app.update_game_phase(io)
    }
    fn apply_effects(&mut self, effects: Vec<ClientEffect>) -> bool {
        self.app.apply_effects(effects)
    }
    fn title_and_preferences(&mut self, after_title: Vec<ClientEffect>) {
        self.app.title_and_preferences_phase(after_title);
    }
}

impl FrameClock {
    /// F0: the wall-clock frame delta (for the free camera, capped at 0.1 s)
    /// and the logic cycles the clock grants this update.
    pub(super) fn begin_frame(&mut self) -> (f32, i32) {
        let now = crate::logic_clock::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        (dt, self.logic.poll())
    }
}

impl ViewerApp {
    /// The shell's `Io` (outside `about_to_wait`, which lends it to the
    /// core).
    pub(super) fn io(&mut self) -> &mut dyn Io {
        self.io
            .as_deref_mut()
            .expect("the shell's Io is lent only during a frame")
    }

    /// Accepted events are recorded before they enter the retained owners.
    pub(super) fn accept_input(&mut self, input: InputEvent) {
        if self.core.session.is_none() {
            return;
        }
        if let Err(error) = self.try_accept_input(input) {
            log::warn!("[client910] input refused: {error:#}");
        }
    }

    fn try_accept_input(&mut self, input: InputEvent) -> anyhow::Result<()> {
        anyhow::ensure!(self.core.session.is_some(), "input has no session owner");
        input.encode()?;
        self.io()
            .window_event(WindowRecord::AcceptedInput(input.clone()));
        input.apply(self.core.session.as_mut().expect("checked session"))
    }

    #[cfg(unix)]
    fn poll_live_control(&mut self) {
        let Some(mut control) = self.control.take() else {
            return;
        };
        let Some(session) = self.core.session.as_mut() else {
            self.control = Some(control);
            return;
        };
        let cycle = self.core.cycle;
        let pending = control.poll(
            session,
            cycle,
            self.input.focused,
            crate::logic_clock::monotonic_millis(),
        );
        let result = pending.and_then(|pending| {
            if let Some(pending) = pending {
                let outcome = match self.try_accept_input(pending.event.clone()) {
                    Ok(()) => crate::live_control::ActionOutcome::Applied,
                    Err(error) => crate::live_control::ActionOutcome::Refused(format!("{error:#}")),
                };
                control.complete(&pending, cycle, outcome)?;
            }
            Ok(())
        });
        match result {
            Ok(()) => self.control = Some(control),
            Err(error) => log::warn!("[client910] live control stopped: {error:#}"),
        }
    }

    /// P0: maximize/live-resize can deliver dozens of native events before
    /// the next game update. Apply only their final dimensions and drain
    /// the resulting cache hooks in this update.
    pub(super) fn resize_phase(&mut self) {
        let Some([width, height]) = self.input.pending_resize.take() else {
            return;
        };
        self.view.camera.viewport = (width, height);
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(width, height);
        }
        if let Some(game) = self.core.session.game_mut() {
            game.ui_variables.queries.preferences.window.observe_size(
                [width, height],
                self.window.as_ref().unwrap().scale_factor(),
            );
            game.ui_variables.queries.preferences.window.changed = true;
        }
        if let Err(error) = self.sync_window_settings() {
            log::warn!("[client910] resize canvas: {error:#}");
        }
    }

    /// P3's shell half: the native-resize injector and the console input
    /// update (which runs before rebuild/game work), after the core's
    /// logic-cycle increment.
    pub(super) fn cycle_inputs_phase(&mut self) {
        // Native resize acceptance: let winit deliver the resulting real
        // Resized event through the same path as a title-bar maximize.
        // (`CLIENT910_WINDOW_RESIZES=cycle,w,h;...`, input_script.rs.)
        let script = &self.core.script;
        for (width, height) in script.window_resizes(self.core.cycle) {
            if let Some(window) = &self.window {
                let _ = window.request_inner_size(winit::dpi::LogicalSize::new(width, height));
            }
        }
        self.update_console();
    }

    /// P4: the scene rebuild completes before the game update
    /// reads the following PLAYER_INFO and updates actors in this cycle.
    /// Installing after the read exposed old-region coordinates to roofs
    /// and actor bounds checks for one frame of the new scene.
    pub(super) fn rebuild_phase(&mut self) {
        self.poll_js5_map_wait();
        self.poll_prefetch_responses();
        self.rebuild_scene_tick();
    }

    /// P5: `ClientCore::session_cycle` with the renderer's off-heap memory
    /// query. The per-cycle toolkit clock only scrolled the removed software
    /// toolkit's textures; the GPU renderers animate materials on the frame
    /// clock (`floor_render::begin_material_frame`).
    pub(super) fn session_cycle_phase(&mut self) {
        let renderer = self.renderer.as_ref();
        self.core
            .session_cycle(&|| renderer.and_then(|r| r.allocated_bytes()));
        self.update_pings();
    }

    /// The overlay's ping values: the game and lobby hosts' pingers follow
    /// the session's current addresses.
    fn update_pings(&mut self) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        let pings = self.pings.get_or_insert_with(Default::default);
        pings.game.set_host(Some(&session.current_world.host));
        pings.lobby.set_host(Some(&session.current_lobby.host));
        let stats = &mut session.ui.state.debug_stats;
        stats.game_ping = pings.game.rtt();
        stats.lobby_ping = pings.lobby.rtt();
    }

    /// P7: the login workers (world reconnect, host resolution), the
    /// connection-drop injector and the lobby connection's read batch
    /// (`ClientCore::poll_lobby`), whose events and failures drive the login
    /// flow.
    pub(super) fn connections_phase(&mut self, io: &mut dyn Io) {
        self.poll_reconnect();
        self.poll_host_resolution();
        self.core.inject_connection_drop();
        match self.core.poll_lobby(io) {
            Ok(events) => self.apply_session_events(&events),
            Err(error) if error.downcast_ref::<ConnectionLost>().is_some() => {
                let lobby = self
                    .core
                    .session
                    .as_ref()
                    .is_some_and(|s| crate::login_state::is_lobby(s.machine.state));
                if lobby {
                    self.client_try_reconnect(&format!("{error:#}"));
                } else if let Some(session) = self.core.session.as_mut() {
                    // Title account-creation connection: it drops its own socket
                    // without leaving the title.
                    log::warn!("[client910] account-creation connection closed: {error:#}");
                    session.io.lobby.stream = None;
                }
            }
            Err(error) => {
                log::warn!("[client910] lobby packet paused: {error:#}");
                if let Some(session) = self.core.session.as_mut() {
                    session.io.lobby.stream = None;
                }
            }
        }
    }

    /// P8: the world connection's read batch and its packet effects
    /// (`poll_live_rebuild`); true when a fatal client cheat asked to shut
    /// down.
    pub(super) fn live_read_phase(&mut self, io: &mut dyn Io) -> bool {
        self.poll_live_rebuild(io);
        self.lifecycle.shutdown_requested
    }

    /// P9: `ClientCore::update_game` (the game update and the interface update) with
    /// the shell's inputs and the renderer-owned scene picking; returns the
    /// requests for P10/P11.
    pub(super) fn update_game_phase(&mut self, io: &mut dyn Io) -> Vec<ClientEffect> {
        let player_renderer = &mut self.entities.players;
        let material_store = &self.scene.material_store;
        let scene_graph = &mut self.scene.graph;
        let mut refresh_picks =
            |game: &mut crate::client_game::ClientGame, ui: &mut crate::ui_runtime::Runtime| {
                if let (Some(players), Some(materials), Some(frame)) = (
                    player_renderer.as_mut(),
                    material_store.as_ref(),
                    ui.engine.scene.player_picks.as_mut(),
                ) {
                    let mut input = crate::ui_cam2::SceneInput::new_with_objects(
                        &game.runtime.map,
                        &game.runtime.feed.state.players,
                        Some(&game.runtime.feed.state.zones.objects),
                        game.runtime.terrain.as_ref(),
                        game.runtime.terrain_generation,
                    );
                    input.npcs = Some(&game.runtime.feed.state.npcs);
                    let mouse = ui.input.click.unwrap_or(ui.engine.platform.mouse);
                    if !ui
                        .state
                        .viewport
                        .is_some_and(|(viewport, _)| frame.matches(&input, viewport))
                    {
                        frame.picks.clear();
                        frame.npc_picks.clear();
                        frame.loc_picks.clear();
                        frame.obj_picks.clear();
                        frame.order.clear();
                    } else {
                        if let Err(error) = players.refresh_picks(
                            game,
                            materials,
                            frame,
                            mouse,
                            ui.engine.configs.npcs.as_deref(),
                        ) {
                            frame.picks.clear();
                            frame.npc_picks.clear();
                            crate::logging::warn_repeated!("[client910] player picking: {error:#}");
                        }
                        if let Some(scene) = scene_graph.as_mut() {
                            frame.refresh_locs(scene, mouse);
                        }
                    }
                }
            };
        let gl_texture_formats = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.compressed_texture_formats().to_vec());
        // The active renderer, lent to the interface tick for the commands
        // that profile the device (`detailget_performance_metric` and the
        // auto-setup): they measure on it synchronously, as the original does.
        let frame = self.lifecycle.client_frame;
        let cycle = self.core.cycle;
        let diag = &mut self.diag;
        let mut take_shot = || diag.next_series_shot(cycle);
        let mut probe = self
            .renderer
            .as_mut()
            .map(|toolkit| crate::active_toolkit::Probe {
                toolkit,
                frame,
                shot: &mut take_shot,
            });
        self.core.update_game(
            &self.pack,
            io,
            InputFrame {
                console_open: self.console.developer.open,
                focused: self.input.focused,
                gl_texture_formats,
                cursor: self.input.cursor_state.current,
                refresh_picks: &mut refresh_picks,
                probe: probe
                    .as_mut()
                    .map(|probe| probe as &mut dyn crate::ui_preferences::metric::RendererProbe),
            },
        )
    }

    /// P11: the title/lobby world, the account-creation connect, then the
    /// preference owners
    /// (toolkit, graphics device, canvas, scene build), the console replay
    /// and the queued server commands.
    pub(super) fn title_and_preferences_phase(&mut self, after_title: Vec<ClientEffect>) {
        self.update_title_world();
        self.apply_effects(after_title);
        if let Err(error) = self.apply_toolkit_preferences() {
            log::warn!("[client910] toolkit settings: {error:#}");
        }
        if let Err(error) = self.sync_graphics_settings() {
            log::warn!("[client910] graphics device settings: {error:#}");
        }
        if let Err(error) = self.sync_window_settings() {
            log::warn!("[client910] canvas settings: {error:#}");
        }
        if let Err(error) = self.apply_scene_preferences() {
            log::warn!("[client910] graphics settings: {error:#}");
        }
        if let Err(error) = self.replay_console_command() {
            log::warn!("[client910] console replay failed: {error:#}");
        }
        if let Err(error) = self.dispatch_server_command() {
            log::warn!("[client910] command failed: {error:#}");
        }
    }

    /// F1: the free camera on the wall-clock `dt`, the window title, the
    /// `--screenshot` exit and the redraw request; false when the headless
    /// capture is done and the event loop stops.
    pub(super) fn frame_tail(&mut self, dt: f32) -> bool {
        #[cfg(unix)]
        self.poll_live_control();
        self.update_camera(dt);
        self.refresh_title();
        if self.diag.screenshot.is_some()
            && self.lifecycle.loading.is_none()
            && self
                .renderer
                .as_ref()
                .is_some_and(|r| r.screenshot_written())
            && screenshot_series().is_none_or(|c| self.diag.screenshot_series_next >= c.len())
        {
            return false;
        }
        if self.core.redraw_due() {
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }
        true
    }
}
