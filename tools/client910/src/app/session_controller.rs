//! `ViewerApp`'s session controller: the login flow (login, logout and
//! reconnect), the world switch, the JS5 main loop, the live read (and its
//! session events) and the map transaction (map rebuild -> scene rebuild
//! -> `MAP_BUILD_COMPLETE`).
use super::*;

impl ViewerApp {
    /// The client shutdown, run once from the window owner's exit.
    pub(super) fn mainquit(&mut self) {
        // Close the JS5 TCP client gracefully, shut down the HTTP client's
        // executor and quit the disk cache.
        self.js5.quit();
        // Save the client variables to prefs file "2".
        if let Some(game) = self.core.session.game_mut() {
            let vars = &mut game.ui_variables;
            if let Err(error) = vars.persistence.save_local(
                &mut vars.client,
                crate::logic_clock::monotonic_millis(),
                true,
            ) {
                log::warn!("[client910] save client variables: {error:#}");
            }
        }
        // Closing the game/lobby streams gracefully: a socket stream joins its
        // writer thread before closing, so bytes already flushed to the
        // stream are still written. The unwritten tail of this port's
        // non-blocking writes is that writer buffer.
        if let Some(session) = self.core.session.as_mut() {
            for (stream, pending) in [
                (
                    session.io.world.stream.take(),
                    std::mem::take(&mut session.io.world.pending_writes),
                ),
                (
                    session.io.lobby.stream.take(),
                    std::mem::take(&mut session.io.lobby.pending_writes),
                ),
            ] {
                let Some(mut stream) = stream else {
                    continue;
                };
                use std::io::Write;
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(1)));
                let _ = stream.write_all(&pending);
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
        // The audio stop, the toolkit dispose and the JS5/disk caches are
        // released with their owners when the process exits. The
        // window-closing hook outside local mode opens the "loggedout" URL on
        // the hosting applet page; a standalone process has no applet page,
        // so nothing is opened.
    }

    /// The reload for `JS5_RELOAD`: keep the credentials, log out (not to
    /// the lobby), drop the JS5 client and re-enter loading state 5; the
    /// loading updates then re-run every loading stage under the loading
    /// screens, return to state 4 and request a login with the kept
    /// credentials.
    pub(super) fn js5_reload(&mut self) {
        let Some(session) = self.core.session.as_ref() else {
            return;
        };
        let username = session.username.clone();
        let password = session.password.clone();
        self.client_logout(false);
        // Reset the resource manager and drop the JS5 client.
        self.js5.reload();
        let mut owner = loading_host::LoadingOwner::reload(self);
        owner.loading.reload(username, password);
        self.lifecycle.loading = Some(owner);
        self.set_client_state(crate::login_state::LOADING);
    }

    /// The client state as the JS5 owners read it: the session's, or the
    /// loading owner's before the session exists.
    pub(super) fn js5_client_state(&self) -> i32 {
        match (&self.core.session, &self.lifecycle.loading) {
            (Some(session), _) => session.machine.state,
            (None, Some(loading)) => loading.client_state(),
            (None, None) => crate::login_state::LOADING,
        }
    }

    /// The JS5 TCP pump and client update every cycle; a fatal JS5 error
    /// code stops the client. Also publishes the provider accounting the CS2 `preload_*`
    /// commands and `drawDebug` read.
    pub(super) fn js5_mainloop(&mut self, event_loop: &ActiveEventLoop) {
        let state = self.js5_client_state();
        self.js5.mainloop(state);
        if let Some(code) = self.js5.fatal.take() {
            log::warn!("[client910] fatal error {code}: js5 failure; stopping");
            event_loop.exit();
            return;
        }
        let preload = self.js5.preload_percent();
        let cache = self.js5.cache_stats();
        if let Some(ui) = self.core.session.ui_mut() {
            ui.engine.builtins.preload_progress = preload;
            ui.state.debug_stats.cache = cache;
            // Drain the sprite prefetch.
            self.js5
                .drain_sprite_prefetch(&mut ui.engine.builtins.sprite_prefetch);
        }
        if let Some(every) = crate::debug_flags::flags().js5_trace_every {
            if self.clock.frames.is_multiple_of(every) {
                let (urgent, prefetch) = self.js5.outstanding();
                log::info!(
                    "[client910] js5 trace state {state} in {}B urgent {urgent} prefetch {prefetch} errors {} js5State {} preload {:?} cache {:?}",
                    self.js5.bytes_in(),
                    self.js5.tcp.error_count,
                    self.js5.tcp.js5_state,
                    preload,
                    cache
                );
            }
        }
    }

    /// The pending rebuild's landscape progress: send
    /// it to the map worker once every map square's group is ready.
    pub(super) fn poll_js5_map_wait(&mut self) {
        let Some(event) = self.js5_map_wait.take() else {
            return;
        };
        let missing = self.js5.map_groups_missing(&event.groups);
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        if let Some(loading) = session.prefetch_loading.as_mut() {
            if loading.groups == event.groups {
                loading.loaded = loading
                    .total
                    .saturating_sub(missing.unwrap_or(loading.total));
            }
        }
        if missing != Some(0) {
            self.js5_map_wait = Some(event);
            return;
        }
        let groups = event.groups.clone();
        match session.prefetch_req_tx.send(PrefetchRequest { event }) {
            Ok(()) => log::info!("[client910] prefetch request {groups:?}"),
            Err(_) => {
                log::warn!(
                    "[client910] prefetch request failed (worker gone); keeping rendered terrain"
                );
                session.prefetch_dead = true;
            }
        }
    }

    /// The last loading stage against the native device: safe-mode
    /// bookkeeping, the saved toolkit's creation (which captures the device
    /// AA level and reapplies saved bloom) and the window mode.
    pub(super) fn install_session_toolkit(&mut self) {
        let (Some(session), Some(window)) = (self.core.session.as_mut(), self.window.as_ref())
        else {
            return;
        };
        let window = window.clone();
        let renderer = self.renderer.as_mut().unwrap();
        // The saved toolkit answers from here on, not the toolkit 0 the
        // loading screens ran in until this stage.
        let device = renderer.hardware_answers();
        let caps = ToolkitCaps {
            antialiasing: device.supports_antialiasing(),
            bloom: device.supports_bloom(),
        };
        let size = window.inner_size();
        let scale = window.scale_factor();
        let installed = install_toolkit_preferences(
            session,
            caps,
            &mut |samples| {
                let supported = renderer.supports_scene_samples(samples);
                crate::session_record::toolkit(
                    caps,
                    samples,
                    supported,
                    [size.width, size.height],
                    scale,
                );
                supported
            },
            [size.width, size.height],
            scale,
        );
        let Some((samples, bloom_enabled, toolkit0)) = installed else {
            return;
        };
        if let Some(game) = &session.game {
            let preferences = &game.ui_variables.queries.preferences;
            log::info!(
                "[client910] saved toolkit installed: toolkit0={toolkit0} anti-aliasing available={} bloom available={} bloom on={bloom_enabled} samples={samples}",
                preferences.anti_aliasing,
                preferences.bloom
            );
        }
        if let Err(error) = renderer.set_scene_effects(samples, bloom_enabled) {
            log::warn!("[client910] initial graphics settings: {error:#}");
        }
        // Loading created the saved toolkit: toolkit 0 is game
        // state (its answers), drawn by the GPU renderer.
        renderer.set_toolkit0(toolkit0);
        if let Some(game) = &mut session.game {
            game.ui_variables.queries.preferences.window.native =
                Some(std::sync::Arc::new(crate::ui_window_winit::Native(window)));
        }
    }

    /// The session's canvas, interface layout, display modes, window
    /// status and the initial interface frames received before the window.
    /// Returns the client debug events among those frames.
    pub(super) fn install_session_canvas(&mut self) -> Vec<crate::session::UiEvent> {
        let mut debug_events = Vec::new();
        let mut effects = packet_effects(None, false);
        if let Some(session) = &mut self.core.session {
            let setup = (|| -> anyhow::Result<()> {
                if let Some(game) = &session.game {
                    let p = &game.ui_variables.queries.preferences;
                    if let Some(canvas) = p.window.canvas(p.options.get("screenSize").unwrap()) {
                        self.renderer.as_mut().unwrap().set_game_canvas(canvas)?;
                    }
                }
                let (w, h) = self.renderer.as_ref().unwrap().canvas_size();
                // The raw display-mode list the window system reports.
                let modes: Vec<crate::ui_runtime::FullscreenMode> = self
                    .window
                    .as_ref()
                    .unwrap()
                    .available_monitors()
                    .flat_map(|m| m.video_modes())
                    .map(|v| crate::ui_runtime::FullscreenMode {
                        width: v.size().width as i32,
                        height: v.size().height as i32,
                        bit_depth: i32::from(v.bit_depth()),
                        refresh: (v.refresh_rate_millihertz() / 1000) as i32,
                    })
                    .collect();
                crate::session_record::canvas([w, h], &modes);
                crate::session_record::gl_formats(
                    self.renderer.as_ref().unwrap().compressed_texture_formats(),
                );
                install_canvas_state(session, [w, h], &modes, &mut debug_events)?;
                Ok(())
            })();
            effects = packet_effects(Some(&mut session.ui), false);
            if let Err(error) = setup {
                log::warn!("[client910] initial UI paused: {error:#}");
                session.polling_dead = true;
            }
        }
        self.apply_effects(effects);
        if let Err(error) = self.sync_window_settings() {
            log::warn!("[client910] initial canvas: {error:#}");
        }
        debug_events
    }

    /// The message box draw (text, immediate flush, with the p12 full font and
    /// metrics): painted over the retained last frame and presented now, so a
    /// synchronous build that follows keeps it on screen.
    pub(super) fn present_message_box(&mut self, text: &str) {
        let (Some(session), Some(renderer)) = (self.core.session.as_ref(), self.renderer.as_mut())
        else {
            return;
        };
        let ui = &session.ui;
        let Some(fonts) = ui.state.fonts.as_ref() else {
            return;
        };
        let (w, h) = renderer.canvas_size();
        let mut painter = crate::ui_paint::Painter::new([w, h]);
        if let Err(error) = crate::message_box::draw(
            &mut painter,
            fonts,
            &ui.engine.builtins.message_box,
            text,
            [w as i32, h as i32],
            self.lifecycle.client_frame,
        ) {
            crate::logging::warn_repeated!("[client910] message box: {error:#}");
        }
        // `CLIENT910_SCREENSHOT_SERIES` cycles that fall inside a rebuild.
        if let (Some(cycles), Some((path, _))) = (screenshot_series(), &self.diag.screenshot) {
            let next = self.diag.screenshot_series_next;
            if let Some(&cycle) = cycles.get(next).filter(|&&c| self.core.cycle >= c) {
                let stem = path
                    .file_stem()
                    .map_or("frame".into(), |s| s.to_string_lossy().into_owned());
                let shot = path.with_file_name(format!("{stem}_{cycle:05}.png"));
                log::info!(
                    "[client910] screenshot series {} at logic cycle {} (state {} message box {text:?})",
                    shot.display(),
                    self.core.cycle,
                    session.machine.state
                );
                renderer.request_screenshot(shot);
                self.diag.screenshot_series_next = next + 1;
            }
        }
        if let Err(error) = renderer.frame_message_box(painter.finish()) {
            log::warn!("[client910] message box frame failed: {error:#}");
        }
    }

    /// The non-async normal-map rebuild, the region-map rebuild and the
    /// cutscene rebuild pass state 3: an in-game map transaction enters state 3 and flushes the LOADING box
    /// before its squares load. Login-time rebuilds are installed by the
    /// login owners before state 18.
    pub(super) fn enter_rebuild_state(&mut self) {
        let Some(session) = self.core.session.as_ref() else {
            return;
        };
        if session.machine.state != crate::login_state::GAME
            || session
                .game
                .as_ref()
                .is_none_or(|g| g.runtime.map_request.is_none())
        {
            return;
        }
        self.set_client_state(crate::login_state::REBUILD_GAME);
        let text = loading_host::rebuild_message_text(&self.core.session.as_ref().unwrap().rebuild);
        self.present_message_box(&text);
    }

    /// The scene rebuild each logic cycle of a rebuild state: the map squares
    /// the prefetch worker has not yet ensured are not ready (`LOAD_MAPS`).
    pub(super) fn rebuild_scene_tick(&mut self) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        if !crate::login_state::is_rebuild(session.machine.state)
            || session
                .game
                .as_ref()
                .is_none_or(|g| g.runtime.map_request.is_none())
        {
            return;
        }
        let pending = session
            .prefetch_loading
            .as_ref()
            .map_or(0, |l| l.total.saturating_sub(l.loaded) as i32);
        if !session.rebuild.load_maps(pending) {
            return;
        }
        // The loc stage: every loc model is read from the local pack, so
        // none is ever waited for. (Were one, a thousand cycles of the same
        // count would report the build stuck, with the JS5 state.)
        let cycle = self.core.cycle;
        let Some(stuck) = session
            .rebuild
            .watch_locs(crate::login_state::LocsWaiting::default(), cycle)
        else {
            return;
        };
        let report = crate::net::encode_map_build_stuck(
            stuck.loc,
            stuck.model,
            stuck.count,
            &crate::net::Js5Stall {
                connect_state: self.js5.connect_state(),
                error_count: self.js5.tcp.error_count,
                js5_state: self.js5.tcp.js5_state,
                urgents_full: self.js5.tcp.is_urgents_full(),
                prefetches_full: self.js5.tcp.is_prefetches_full(),
                pending_requests: self.js5.disk.pending_requests(),
            },
        );
        if let Some(game) = self.core.session.game_mut() {
            game.ui_variables
                .queries
                .preferences
                .queue_graphics_packet(report);
        }
    }

    /// Once every square is loaded: the
    /// `(100%)` box is flushed when a wait stage ran, then the synchronous
    /// build follows. Every loc model is read from the local pack, so
    /// `testReadLocs` finds nothing missing (`LOAD_LOCS` never waits).
    pub(super) fn begin_rebuild_build(&mut self) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        if !crate::login_state::is_rebuild(session.machine.state) {
            return;
        }
        session.rebuild.load_maps(0);
        session.rebuild.load_locs(0);
        if session.rebuild.begin_build() {
            let language = crate::loading::client_language();
            let text = format!(
                "{}{}(100%)",
                crate::loading::Text::Loading.display(language),
                crate::message_box::BR
            );
            self.present_message_box(&text);
        }
    }

    /// Leave the rebuild state; a pending
    /// `EXECUTE_CLIENT_CHEAT` 17 timer prints the rebuild time.
    pub(super) fn leave_rebuild_state(&mut self) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        let state = session.machine.state;
        let timer = session.rebuild_timer.take();
        if crate::login_state::is_rebuild(state) {
            self.set_client_state(crate::login_state::rebuilt_state(state));
        }
        if let Some(start) = timer {
            let line = (crate::logic_clock::monotonic_millis() - start).to_string();
            if let Err(error) = self.console_addline(&line) {
                log::warn!("[client910] rebuild timer console: {error:#}");
            }
        }
    }

    /// The world rebuild in game: the current map
    /// rebuilds through the preference-rebuild owner
    /// ([`Self::apply_scene_preferences`]) on this logic cycle.
    pub(super) fn world_rebuild(&mut self) {
        if let Some(game) = self.core.session.game_mut() {
            game.ui_variables
                .queries
                .preferences
                .pending_effects
                .push(crate::ui_preferences::PreferenceEffect::SceneRebuild);
        }
    }

    /// The scene rebuild installs the scene before completing the map
    /// transaction. Failed uploads leave PreparedMap owned for an explicit retry.
    pub(super) fn finish_game_map(&mut self) -> anyhow::Result<()> {
        rs910_core::profile::scope!("region finish map");
        if self
            .core
            .session
            .as_ref()
            .is_none_or(|s| s.prepared_map.is_none())
        {
            return Ok(());
        }
        let ready = if self.faithful_scene_required() {
            !self.scene.meshes.scene_upload_failed
                && self.scene.meshes.floor_meshes.len() == 4
                && self.scene.meshes.floor_meshes.iter().all(Option::is_some)
        } else {
            self.scene.graph.as_ref().is_some_and(|scene| {
                self.scene.floors.len() == scene.max_level
                    && self.scene.floors.iter().all(Option::is_some)
            }) && self.scene.live.is_some()
                && self.scene.material_store.is_some()
        };
        anyhow::ensure!(ready, "scene renderer resources incomplete");
        acknowledge_game_map(
            self.core.session.as_mut().unwrap(),
            crate::logic_clock::monotonic_millis,
        )?;
        self.clock.logic.reset();
        Ok(())
    }
    /// Diagnostic adapter into the remote-command writer. The live console
    /// host shares this packet path; actor positions only change on server input.
    pub(super) fn dispatch_server_command(&mut self) -> anyhow::Result<()> {
        match self.core.session.as_mut() {
            Some(session) => dispatch_server_command(session),
            None => Ok(()),
        }
    }

    /// A client state change through the session's state machine.
    pub(super) fn set_client_state(&mut self, next: i32) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        let ctx = session.login_context();
        let from = session.machine.state;
        let mut effects = Vec::new();
        session.machine.set_state(next, &ctx, &mut effects);
        log::info!(
            "[client910] cycle {} client state {from} -> {} {effects:?}",
            self.core.cycle,
            session.machine.state
        );
        self.apply_login_effects(effects);
    }

    /// Logout (to the lobby or not).
    pub(super) fn client_logout(&mut self, to_lobby: bool) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        let ctx = session.login_context();
        let from = session.machine.state;
        let mut effects = Vec::new();
        session.machine.logout(to_lobby, &ctx, &mut effects);
        // Logout resets the caches (without the full reset).
        {
            let ui = &mut session.ui;
            ui.reset_caches();
        }
        log::info!(
            "[client910] cycle {} logout({to_lobby}): client state {from} -> {} {effects:?}",
            self.core.cycle,
            session.machine.state
        );
        self.apply_login_effects(effects);
    }

    /// The reconnect attempt after an I/O failure on the active connection.
    pub(super) fn client_try_reconnect(&mut self, reason: &str) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        let ctx = session.login_context();
        let from = session.machine.state;
        let mut effects = Vec::new();
        session.machine.try_reconnect(&ctx, &mut effects);
        log::warn!(
            "[client910] connection lost ({reason}): client state {from} -> {} {effects:?}",
            session.machine.state
        );
        if session.machine.state == crate::login_state::RECONNECT {
            // The redraw draws the message box each frame
            // (`render_frame_inner`).
            log::info!("[client910] Connection lost - attempting to reestablish");
        }
        self.apply_login_effects(effects);
    }

    /// A terminal login reply from the worker: the login state update
    /// for title/lobby logins, or the state-14/19 mainloop arm.
    pub(super) fn client_login_failed(&mut self, reply: i32) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        let ctx = session.login_context();
        let from = session.machine.state;
        let mut effects = Vec::new();
        session
            .machine
            .game_login_failed(reply, session.transfer_cancellable, &ctx, &mut effects);
        log::info!(
            "[client910] login reply {reply}: client state {from} -> {} {effects:?}",
            session.machine.state
        );
        self.apply_login_effects(effects);
    }

    pub(super) fn apply_login_effects(&mut self, effects: Vec<crate::login_state::Effect>) {
        use crate::login_state::Effect;
        for effect in effects {
            match effect {
                Effect::ShowLogin { reopen } => self.show_login(reopen),
                Effect::ShowLobby { reopen } => self.show_lobby(reopen),
                Effect::RequestLobbyLogin => self.request_lobby_login(),
                Effect::RequestGameLogin { reconnect } => self.request_game_login(reconnect),
                Effect::RejectEmptyCredentials { lobby } => {
                    if let Some(ui) = self.core.session.ui_mut() {
                        // Empty credentials reject the login with reply 3.
                        ui.engine.login.in_progress = false;
                        if lobby {
                            ui.engine.login.lobby_reply = 3;
                        } else {
                            ui.engine.login.reply = 3;
                        }
                    }
                }
                Effect::CloseConnections => self.close_connections(),
                Effect::CompleteRebuild => {
                    if let Some(session) = self.core.session.as_mut() {
                        session.rebuild.complete();
                    }
                }
                Effect::RestorePreviousWorld => {
                    if let Some(session) = self.core.session.as_mut() {
                        if let Some(previous) = session.previous_world.clone() {
                            session.set_world(previous);
                        }
                        if let Some(stream) = session.io.world.stream.take() {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                        }
                    }
                }
            }
        }
    }

    /// Replace the retained interface tree (`showLogin`/`showLobby` with
    /// `reopen`), through the logged-out runtime's variable domains.
    pub(super) fn replace_top_level(&mut self, top: i32) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        let (ui, Some(game)) = (&mut session.ui, session.game.as_mut()) else {
            return;
        };
        if let Err(error) = crate::client_game::with_game(game, |v| ui.show_top_level(v, top)) {
            log::warn!("[client910] top level {top} failed: {error:#}");
        }
    }

    /// Show the login screen (optionally reopening its interface).
    pub(super) fn show_login(&mut self, reopen: bool) {
        if reopen {
            let top = self.core.session.as_ref().map_or(-1, |s| s.login_interface);
            self.replace_top_level(top);
        }
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        // A fresh unnamed placeholder local player.
        {
            let ui = &mut session.ui;
            ui.engine.login.lobby_player_name.clear();
        }
        // Reset the credentials.
        session.username.clear();
        session.password.clear();
        session.auth = crate::net::AuthOptions {
            new_auth_preference: String::new(),
            auth_dont_trust: true,
            reconnect: false,
        };
        {
            let ui = &mut session.ui;
            // Restore the cam2 client defaults.
            ui.engine.camera.cam2.restore_client_defaults();
            // Create the cancel option, reset the default cursor to -1 and set
            // the login cursor; the cursor owner reads the reset default
            // cursor (then the login cursor) on the next redraw.
            ui.state.minimenu.create_cancel_option();
            ui.state.minimenu.default_cursor = -1;
            // The camera x/z reset to 0, then the cutscene move-to
            // position (cameraState 5) or the camera move-along, which the
            // retained per-tick spline owner performs while splines are set.
            let legacy = &mut ui.engine.camera.cam2.legacy;
            legacy.pose.x = 0;
            legacy.pose.z = 0;
            if ui.engine.camera.cam2.camera_state == 5 {
                if let Some(target) = ui.engine.camera.cam2.legacy.move_to {
                    ui.engine.camera.cam2.legacy.pose.x = target.x << 9;
                    ui.engine.camera.cam2.legacy.pose.z = target.z << 9;
                }
            }
        }
        // a fresh unnamed local player centred in the (released)
        // world. The logged-out runtime `close_connections` installs holds
        // no player snapshots (`playerSnapshots.clear()`), and its world is
        // unbuilt, so the centre is (0, 0) until a title rebuild.
        // Reset the environment fade.
        self.core.environment.fade = None;
        self.core.environment.fade_reset = true;
    }

    /// Show the lobby screen (optionally reopening its interface).
    pub(super) fn show_lobby(&mut self, reopen: bool) {
        if reopen {
            let top = self.core.session.as_ref().map_or(-1, |s| s.lobby_interface);
            self.replace_top_level(top);
        }
    }

    /// The lobby login request: the asynchronous worker performs login steps
    /// 14..7 against the current lobby.
    pub(super) fn request_lobby_login(&mut self) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        if let Some(stream) = session.io.lobby.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        session.io.lobby.pending.clear();
        session.io.lobby.pending_writes.clear();
        if let Some(cancel) = session.reconnect_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        let (generation, cancel) = begin_reconnect_attempt(session);
        let lobby = session.current_lobby.clone();
        // The relogin after a logout waits for nothing: a device check ends it.
        let relogin = session.machine.state == crate::login_state::LOBBY_RELOGIN;
        spawn_lobby_login_worker(
            generation,
            cancel,
            crate::net::LoginParams {
                host: lobby.host.clone(),
                port: lobby.socket_port(),
                username: session.username.clone(),
                password: session.password.clone(),
                progress: session.login_progress.clone(),
                auth: crate::net::AuthOptions {
                    reconnect: false,
                    ..session.auth.clone()
                },
                site_settings: session.ui.engine.login.site_settings.clone(),
                uid192: session.ui.engine.login.uid192,
                // The login reports an all-unknown machine: the probe stays local.
                hardware: rs910_core::hardware::Hardware::default(),
                client: login_client_report(session),
                launcher: launcher_report(session),
                verify_id: session.ui.state.life.verify,
                archive_checksums: self.js5.archive_checksums(),
                sso: session.sso.login(),
                switched_world: true,
                relogin,
                crypto: session.login_crypto.clone(),
            },
            session.reconnect_resp_tx.clone(),
        );
        session.reconnect_started = true;
        {
            let ui = &mut session.ui;
            ui.engine.login.in_progress = true;
            ui.engine.login.lobby_reply = -3;
        }
        log::info!(
            "[client910] lobby login started: {}:{}",
            lobby.host,
            lobby.port
        );
    }

    /// The game login request against the current world. Setting the
    /// stream replaces any previous game stream.
    pub(super) fn request_game_login(&mut self, reconnect: bool) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        if let Some(stream) = session.io.world.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        session.io.world.pending.clear();
        session.io.world.pending_writes.clear();
        session.polling_dead = true;
        if let Some(cancel) = session.reconnect_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        let (generation, cancel) = begin_reconnect_attempt(session);
        let world = session.current_world.clone();
        // The login block says whether this is the world the lobby advertised.
        let switched_world = session
            .target_world
            .as_ref()
            .is_none_or(|target| target.node != world.node);
        spawn_reconnect_worker(
            generation,
            cancel,
            crate::net::LoginParams {
                host: world.host.clone(),
                port: world.socket_port(),
                username: session.username.clone(),
                password: session.password.clone(),
                progress: session.login_progress.clone(),
                auth: crate::net::AuthOptions {
                    reconnect,
                    ..session.auth.clone()
                },
                site_settings: session.ui.engine.login.site_settings.clone(),
                uid192: session.ui.engine.login.uid192,
                // The login reports an all-unknown machine: the probe stays local.
                hardware: rs910_core::hardware::Hardware::default(),
                client: login_client_report(session),
                launcher: launcher_report(session),
                verify_id: session.ui.state.life.verify,
                archive_checksums: self.js5.archive_checksums(),
                sso: session.sso.login(),
                switched_world,
                relogin: false,
                crypto: session.login_crypto.clone(),
            },
            session.strict_entities,
            session.reconnect_resp_tx.clone(),
        );
        session.reconnect_started = true;
        {
            let ui = &mut session.ui;
            ui.engine.login.in_progress = true;
            ui.engine.login.reply = -3;
        }
        log::info!(
            "[client910] game login started: world {} {}:{} reconnect={reconnect}",
            world.node,
            world.host,
            world.port
        );
    }

    /// Logout: close every connection, reset the login owner and release the
    /// world. The logged-out runtime keeps the process-lifetime client
    /// variables and preferences.
    pub(super) fn close_connections(&mut self) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        if let Some(cancel) = session.reconnect_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        session.reconnect_generation = session.reconnect_generation.wrapping_add(1);
        session.reconnect_started = false;
        session.login_progress.reset();
        if let Some(stream) = session.io.world.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        if let Some(stream) = session.io.lobby.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        session.io.world.pending.clear();
        session.io.world.pending_writes.clear();
        session.io.world.resync = Default::default();
        session.io.lobby.pending.clear();
        session.io.lobby.pending_writes.clear();
        session.io.lobby.resync = Default::default();
        session.io.idle_connection = Default::default();
        session.io.incoming_idle = Default::default();
        session.io.ping.reset();
        session.polling_dead = true;
        // Reset the login state.
        {
            let ui = &mut session.ui;
            // Reset the positioned sounds.
            ui.audio.reset_positioned(true);
            // Reset the world map.
            ui.engine.world_map.borrow_mut().logout();
            ui.engine.login.in_progress = false;
            ui.engine.login.reply = -2;
            ui.engine.login.lobby_reply = -2;
            ui.engine.login.queue_position = -1;
            // Reset the transmit counters.
            ui.state.life.cycles.reset_transmit_nums();
        }
        // Reset the world base.
        self.lifecycle.title_world.reset_base();
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        // Release the scene: a fresh
        // logged-out entity runtime; client variables carry over.
        let pack = self.pack.clone();
        match logged_out_game(&pack, session.game.take()) {
            Ok(game) => session.game = Some(game),
            Err(error) => log::warn!("[client910] logged-out runtime: {error:#}"),
        }
        session.prepared_map = None;
        session.groups.clear();
        session.prefetch_loading = None;
    }

    /// `worldlist_switch` accepted in lobby state 13: sets the world only;
    /// `lobby_entergame` later connects to this world.
    pub(super) fn start_world_switch(&mut self, request: crate::ui_runtime::WorldSwitchRequest) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        // The server ports for the world id.
        let port = session.world_port_base.saturating_add(request.world_id);
        let world = ServerAddress {
            node: i32::from(request.world_id),
            host: request.host,
            port,
            port2: port,
            use_secondary_port: true,
            use_proxy: false,
        };
        log::info!(
            "[client910] worldlist_switch: currentWorld = {} {}:{}",
            world.node,
            world.host,
            world.port
        );
        session.set_world(world);
    }

    /// The login request / lobby entry from the title screen: retain the typed
    /// credentials and enter state 7 (game) or 17 (lobby). Entering the lobby
    /// first closes the lobby connection.
    pub(super) fn start_login_request(&mut self, request: crate::ui_runtime::LoginRequest) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        let ready =
            session.machine.state == crate::login_state::LOGIN && !session.reconnect_started;
        if !ready {
            log::info!(
                "[client910] login request ignored in state {}",
                session.machine.state
            );
            if !session.reconnect_started {
                {
                    let ui = &mut session.ui;
                    ui.engine.login.in_progress = false;
                }
            }
            return;
        }
        session.username = request.username;
        session.password = request.password;
        // A login by username and password drops any social sign-on; picking a
        // network keeps its key only while it is the same network.
        match request.sso {
            Some(network) => session.sso.choose(network),
            None => session.sso.clear(),
        }
        session.auth = crate::net::AuthOptions {
            new_auth_preference: request.new_auth_preference,
            auth_dont_trust: request.auth_dont_trust,
            reconnect: false,
        };
        if request.lobby {
            if let Some(stream) = session.io.lobby.stream.take() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            self.set_client_state(crate::login_state::LOBBY_LOGIN);
        } else {
            self.set_client_state(crate::login_state::LOGIN_GAME);
        }
    }

    /// Start the dedicated account-creation transport from the retained
    /// title tree. The worker owns the fresh socket until the connect reply;
    /// ordinary create_* packets remain queued on the installed lobby owner.
    pub(super) fn start_create_connect(&mut self) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        if session.machine.state != crate::login_state::LOGIN || session.reconnect_started {
            return;
        }
        let lobby = session.current_lobby.clone();
        if let Some(stream) = session.io.lobby.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        session.io.lobby.pending.clear();
        session.io.lobby.pending_writes.clear();
        let (generation, _cancel) = begin_reconnect_attempt(session);
        let info = crate::net::CreateConnectInfo {
            uid192: session.ui.engine.login.uid192,
            launcher: launcher_report(session),
            // The login reports an all-unknown machine: the probe stays local.
            hardware: rs910_core::hardware::Hardware::default(),
            crypto: session.login_crypto.clone(),
        };
        spawn_create_connect_worker(
            generation,
            lobby.host.clone(),
            lobby.socket_port(),
            info,
            session.reconnect_resp_tx.clone(),
        );
        session.reconnect_started = true;
        log::info!(
            "[client910] account-creation connect started: {}:{}",
            lobby.host,
            lobby.port
        );
        self.set_client_state(crate::login_state::ACCOUNT_CREATION);
    }

    /// Entering the game: lobby state 13 with the login owners idle enters
    /// state 15, which logs into the current world.
    pub(super) fn start_game_from_lobby(
        &mut self,
        request: crate::ui_runtime::LobbyEnterGameRequest,
    ) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        if session.machine.state != crate::login_state::LOBBY || session.reconnect_started {
            return;
        }
        session.auth = crate::net::AuthOptions {
            new_auth_preference: request.new_auth_preference,
            auth_dont_trust: request.auth_dont_trust,
            reconnect: false,
        };
        self.set_client_state(crate::login_state::LOBBY_ENTER_GAME);
    }

    /// Apply a login cancel at the
    /// session owner boundary. The worker owns its socket future, so flipping
    /// its token drops that future and closes the in-flight connection; the
    /// generation bump also makes any already-queued result harmless.
    pub(super) fn cancel_login_request(&mut self) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        let in_progress = session.reconnect_started;
        if let Some(cancel) = session.reconnect_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        session.reconnect_generation = session.reconnect_generation.wrapping_add(1);
        session.login_progress.reset();
        session.reconnect_started = false;
        {
            let ui = &mut session.ui;
            ui.engine.login.in_progress = false;
            ui.engine.login.reply = 0;
            ui.engine.login.hoptime = 0;
            ui.engine.login.ban_duration = 0;
            ui.engine.login.queue_position = -1;
        }
        log::info!("[client910] retained login canceled");
        if in_progress {
            let ctx = session.login_context();
            let mut effects = Vec::new();
            session.machine.update_login_state(&ctx, &mut effects);
            self.apply_login_effects(effects);
        }
    }
    /// Advance the world host resolution one hostname at a time.
    pub(super) fn poll_host_resolution(&mut self) {
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        while let Ok(response) = session.host_resolve_resp_rx.try_recv() {
            {
                let ui = &mut session.ui;
                ui.engine
                    .world_list
                    .set_hostpacked(response.world_id, response.hostpacked);
            }
        }
        let request = session
            .ui
            .engine
            .world_list
            .next_host_resolution()
            .map(|(world_id, hostname)| HostResolveRequest { world_id, hostname });
        if let Some(request) = request {
            let _ = session.host_resolve_req_tx.send(request);
        }
    }

    pub(super) fn poll_reconnect(&mut self) {
        let response = {
            let Some(session) = self.core.session.as_ref() else {
                return;
            };
            match session.reconnect_resp_rx.try_recv() {
                Ok(response) => Some(response),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    log::warn!("[client910] world reconnect worker disconnected");
                    None
                }
            }
        };
        let Some(response) = response else {
            return;
        };
        let generation = response.generation();
        let accepted = self.core.session.as_ref().is_some_and(|session| {
            session.reconnect_started && session.reconnect_generation == generation
        });
        if !accepted {
            if let ReconnectResponse::LobbyReady { stream, .. }
            | ReconnectResponse::CreateReady { stream, .. } = response
            {
                let _ = stream.into_std();
            }
            return;
        }
        if let Some(session) = self.core.session.as_mut() {
            session.reconnect_cancel = None;
            // A social sign-on's key outlives the login that negotiated it,
            // whether or not that login went on to succeed.
            if let Some((key, name)) = session.login_progress.social() {
                session.sso.learn(key, name);
            }
        }
        match response {
            ReconnectResponse::Failed {
                error,
                transfer,
                reply_state,
                exhausted,
                late,
                ..
            } => {
                log::warn!("[client910] login failed: {error}");
                // Exhausted attempts report -5 (timeout) or -4 (I/O error);
                // any other worker failure is the I/O error path.
                let mut reply = exhausted.unwrap_or(-4);
                if let Some(session) = self.core.session.as_mut() {
                    let transfer_active = session.machine.state == crate::login_state::TRANSFER;
                    // The reply applies to request state 132 (lobby) or 211.
                    let lobby_request = matches!(
                        session.machine.state,
                        crate::login_state::LOBBY_LOGIN | crate::login_state::LOBBY_RELOGIN
                    );
                    session.reconnect_started = false;
                    {
                        let ui = &mut session.ui;
                        if let Some(outcome) = &transfer {
                            ui.engine.login.disallow_result = outcome.disallow_result;
                            ui.engine.login.disallow_trigger = outcome.disallow_trigger;
                            reply = outcome.reply;
                        }
                        if let Some(reply_state) = &reply_state {
                            reply = reply_state.reply;
                            ui.engine.login.hoptime = reply_state.hoptime;
                            ui.engine.login.ban_duration = reply_state.ban_duration;
                        }
                        // The transfer outcome is copied before leaving state 19.
                        if transfer_active {
                            ui.engine.login.last_transfer_reply = reply;
                            ui.engine.login.last_transfer_disallow_result =
                                ui.engine.login.disallow_result;
                            ui.engine.login.last_transfer_disallow_trigger =
                                ui.engine.login.disallow_trigger;
                        }
                        // Attempts exhausted without a server reply report -4.
                        ui.engine.login.in_progress = false;
                        if lobby_request {
                            ui.engine.login.lobby_reply = reply;
                        } else {
                            ui.engine.login.reply = reply;
                        }
                    }
                }
                self.client_login_failed(reply);
                // A world the lobby entered that fails after accepting the
                // login sends the client back to the lobby by a logout, and
                // the reply stays for the lobby's screens.
                let in_lobby = self
                    .core
                    .session
                    .as_ref()
                    .is_some_and(|session| crate::login_state::is_lobby(session.machine.state));
                if late && in_lobby {
                    self.client_logout(true);
                    if let Some(ui) = self.core.session.ui_mut() {
                        ui.engine.login.reply = reply;
                    }
                }
            }
            ReconnectResponse::Ready { live, .. } if live.login.resume.is_some() => {
                let result = match self.core.session.as_mut() {
                    Some(session) => resume_world_session(session, *live),
                    None => Err(anyhow::anyhow!("missing session")),
                };
                if let Err(error) = result {
                    log::warn!("[client910] in-place reconnect failed: {error:#}");
                    if let Some(session) = self.core.session.as_mut() {
                        session.reconnect_started = false;
                    }
                    self.client_login_failed(-4);
                } else {
                    log::info!("[client910] reconnect resumed in place");
                    self.set_client_state(crate::login_state::GAME);
                }
            }
            ReconnectResponse::Ready { live, .. } => {
                if let Err(error) = self.install_reconnected_world(*live) {
                    log::warn!("[client910] world login install failed: {error:#}");
                    if let Some(session) = self.core.session.as_mut() {
                        session.reconnect_started = false;
                    }
                    self.client_login_failed(-4);
                } else {
                    self.set_client_state(crate::login_state::GAME);
                }
            }
            ReconnectResponse::LobbyReady { stream, login, .. } => {
                if let Err(error) = self.install_lobby_connection(stream, *login) {
                    log::warn!("[client910] lobby connection install failed: {error:#}");
                    if let Some(session) = self.core.session.as_mut() {
                        session.reconnect_started = false;
                    }
                    self.client_login_failed(-4);
                } else {
                    // Enter the lobby state (13).
                    self.set_client_state(crate::login_state::LOBBY);
                }
            }
            ReconnectResponse::CreateReady { stream, reply, .. } => {
                if let Err(error) = self.install_create_connection(stream, reply) {
                    log::warn!("[client910] account-creation connection install failed: {error:#}");
                    if let Some(session) = self.core.session.as_mut() {
                        session.reconnect_started = false;
                    }
                }
            }
            ReconnectResponse::CreateFailed { error, .. } => {
                log::warn!("[client910] account-creation connection failed: {error}");
                if let Some(session) = self.core.session.as_mut() {
                    session.reconnect_started = false;
                    {
                        let ui = &mut session.ui;
                        ui.engine.creation.connect_in_progress = false;
                        ui.engine.creation.connect_reply = -4;
                    }
                }
                // Retry the account-creation connect.
                self.set_client_state(crate::login_state::LOGIN);
            }
        }
    }

    /// Install the lobby reply profile and the lobby connection, then the
    /// target world / restored world.
    pub(super) fn install_lobby_connection(
        &mut self,
        stream: WorkerStream,
        login: crate::net::LoginOk,
    ) -> anyhow::Result<()> {
        let stream = detach_world_socket(stream)?;
        let session = self.core.session.as_mut().context("missing session")?;
        session.io.lobby.stream = Some(stream);
        session.io.lobby.pending.clear();
        session.io.lobby.pending_writes.clear();
        session.io.lobby.resync = Default::default();
        session.reconnect_started = false;
        session.polling_dead = true;
        {
            let ui = &mut session.ui;
            apply_lobby_profile(ui, &login, crate::logic_clock::monotonic_millis());
        }
        // The target world (node 65535 is -1), then the world is restored
        // while the current world is still the startup world.
        session.target_world = login.lobby.world_id.and_then(|node| {
            (node != u16::MAX && !login.lobby.world_host.is_empty()).then(|| ServerAddress {
                node: i32::from(node),
                host: login.lobby.world_host.clone(),
                port: login.lobby.world_port,
                port2: login.lobby.world_port2,
                use_secondary_port: true,
                use_proxy: false,
            })
        });
        {
            let ui = &mut session.ui;
            ui.engine.login.target_world =
                session
                    .target_world
                    .as_ref()
                    .map(|target| crate::ui_runtime::WorldSwitchRequest {
                        world_id: target.node as u16,
                        host: target.host.clone(),
                    });
        }
        if session.current_world == session.default_world {
            if let Some(target) = session.target_world.clone() {
                session.set_world(target);
            }
        }
        log::info!(
            "[client910] lobby login installed; currentWorld {} {}:{}",
            session.current_world.node,
            session.current_world.host,
            session.current_world.port
        );
        Ok(())
    }

    /// Install the socket returned by CREATE_ACCOUNT_CONNECT. A successful
    /// reply enters the ordinary lobby client-protocol stream; rejected
    /// replies leave the existing login tree alive with the raw enum id.
    pub(super) fn install_create_connection(
        &mut self,
        stream: WorkerStream,
        reply: i32,
    ) -> anyhow::Result<()> {
        let stream = detach_world_socket(stream)?;
        let session = self.core.session.as_mut().context("missing session")?;
        session.reconnect_started = false;
        {
            let ui = &mut session.ui;
            ui.engine.creation.connect_in_progress = false;
            ui.engine.creation.connect_reply = reply;
        }
        if reply != 2 {
            // Close the socket and stay in state 12.
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return Ok(());
        }
        if let Some(previous) = session.io.lobby.stream.replace(stream) {
            let _ = previous.shutdown(std::net::Shutdown::Both);
        }
        session.io.lobby.pending.clear();
        session.io.lobby.pending_writes.clear();
        session.io.lobby.resync = Default::default();
        session.polling_dead = true;
        // A successful connect reply enters state 0 on the lobby stream.
        self.set_client_state(crate::login_state::ACCOUNT_CREATION_CONNECTED);
        Ok(())
    }

    /// Replace the logged-out/lobby owner with a successful world login
    /// (preparing for the map closes the lobby connection), then send its first rebuild through the existing JS5/map
    /// transaction owner.
    pub(super) fn install_reconnected_world(
        &mut self,
        mut live: crate::session::LiveState,
    ) -> anyhow::Result<()> {
        let pack_root = self
            .core
            .session
            .as_ref()
            .context("missing session")?
            .pack_root
            .clone();
        let pack = self.pack.clone();
        let local = usize::from(
            live.login
                .pid
                .context("world login omitted local player index")?,
        );
        let mut game = crate::client_game::ClientGame::login(
            &pack,
            local,
            std::mem::take(&mut live.drain.entities),
            live.login.server_token as u64,
            live.login.profile.logged_in_members,
        )?;
        let previous = self.core.session.as_mut().and_then(|s| s.game.take());
        if !adopt_client_variables(previous, &mut game, live.login.profile.logged_in_members)? {
            game.ui_variables
                .queries
                .preferences
                .apply_hardware(rs910_core::hardware::probe());
            install_client_persistence(&mut game, &pack_root)?;
        }
        restore_server_varcs(&mut game, &live.login.server_varcs)?;
        let stream = detach_world_socket(live.stream)?;
        let rebuild = live
            .drain
            .rebuild
            .clone()
            .context("world login omitted REBUILD_NORMAL")?;
        let event = crate::session::RebuildEvent::new(rebuild);
        let ui_events = std::mem::take(&mut live.drain.ui_events);
        let session = self.core.session.as_mut().context("missing session")?;
        session.io.world.stream = Some(stream);
        // Close the lobby connection gracefully.
        if let Some(lobby) = session.io.lobby.stream.take() {
            let _ = lobby.shutdown(std::net::Shutdown::Both);
        }
        session.io.lobby.pending.clear();
        session.io.lobby.pending_writes.clear();
        session.io.lobby.resync = Default::default();
        session.entities = live.drain.entities;
        session.io.world.pending = live.drain.pending;
        session.io.world.pending_writes.clear();
        session.io.idle_connection = Default::default();
        session.io.incoming_idle = Default::default();
        // A new game session: the interface-change count and the ping report
        // start over.
        session.io.ping.reset();
        session.ui.state.life.verify = 0;
        session.ui.state.life.verify_changed = false;
        session.io.world.resync = Default::default();
        let logged_pid = live.login.pid;
        let logged_token = live.login.server_token;
        session.game = Some(game);
        session.prepared_map = None;
        session.groups.clear();
        session.initial_ui = ui_events;
        session.polling_dead = false;
        session.reconnect_started = false;
        let world_id = session.current_world.node;
        {
            let ui = &mut session.ui;
            ui.engine.login.world = world_id;
            ui.engine.login.in_progress = false;
            ui.engine.login.reply = 2;
            ui.engine.login.hoptime = 0;
            ui.engine.login.ban_duration = 0;
            ui.engine.login.queue_position = -1;
            ui.engine.account.logged_in_members = live.login.profile.logged_in_members;
            // The members flag from the login reply.
            ui.set_allow_members(live.login.profile.logged_in_members);
            // The client watch resets with the session.
            ui.client_watch.reset();
            ui.engine.account.player_is_members = live.login.profile.player_is_members;
            ui.engine.account.player_is_quickchat = live.login.profile.player_is_quickchat;
            ui.engine.account.logged_in_quickchat = live.login.profile.logged_in_quickchat;
            ui.engine.account.dob_verified = live.login.profile.dob_verified;
            ui.engine.account.dob = live.login.profile.lobby_dob;
            ui.engine.account.staff_mod_level = live.login.profile.staff_mod_level;
            ui.engine.account.player_mod_level = live.login.profile.player_mod_level;
            if let Some(owner) = &live.login.profile.owner {
                ui.engine.game_host.owner = Some(owner.clone());
            }
            if let Some(clock) = live.login.profile.server_clock {
                // The membership offset is the server clock minus the local clock.
                ui.engine.lobby.membership_offset = clock - crate::logic_clock::monotonic_millis();
            }
        }
        // The packets of the login's first read belong to the game state.
        session.ui.engine.login.client_state = crate::login_state::GAME;
        let mut initial_ui = std::mem::take(&mut session.initial_ui);
        answer_reflection_checks(&mut initial_ui, true, &mut session.io.world.pending_writes);
        let mut debug_events = Vec::new();
        for event in &initial_ui {
            if is_client_debug_event(event) {
                debug_events.push(event.clone());
                continue;
            }
            ui_packet(
                &mut session.ui,
                &session.pack_root,
                session.game.as_mut(),
                event,
            )?;
        }
        let effects = packet_effects(Some(&mut session.ui), false);
        self.apply_effects(effects);
        log::info!(
            "[client910] world login ok: world {world_id} pid={logged_pid:?} server_token={logged_token}"
        );
        self.reload_for_event(event);
        self.apply_session_events(&debug_events);
        Ok(())
    }

    /// Session packets after the retained UI recorded them: `CHANGE_LOBBY`,
    /// `LOGOUT_TRANSFER`, `LOGOUT` and `LOGOUT_FULL`.
    pub(super) fn apply_session_events(&mut self, events: &[crate::session::UiEvent]) {
        for event in events {
            match event {
                crate::session::UiEvent::ChangeLobby {
                    host,
                    node,
                    port,
                    port2,
                } => {
                    if let Some(session) = self.core.session.as_mut() {
                        session.current_lobby = ServerAddress {
                            node: i32::from(*node),
                            host: host.clone(),
                            port: *port,
                            port2: *port2,
                            use_secondary_port: true,
                            use_proxy: false,
                        };
                        log::info!(
                            "[client910] currentLobby updated: {host} node={node} ports={port}/{port2}"
                        );
                    }
                }
                crate::session::UiEvent::LogoutTransfer {
                    world_id,
                    host,
                    port,
                    port2,
                    cancellable,
                } => {
                    if let Some(session) = self.core.session.as_mut() {
                        session.previous_world = Some(session.current_world.clone());
                        session.transfer_cancellable = *cancellable;
                        let port = if *port != 0 {
                            *port
                        } else {
                            session.world_port_base.saturating_add(*world_id)
                        };
                        session.set_world(ServerAddress {
                            node: i32::from(*world_id),
                            host: host.clone(),
                            port,
                            port2: *port2,
                            use_secondary_port: true,
                            use_proxy: false,
                        });
                        log::info!(
                            "[client910] LOGOUT_TRANSFER -> world {world_id} {host}:{port} cancellable={cancellable}"
                        );
                    }
                    self.set_client_state(crate::login_state::TRANSFER);
                    return;
                }
                crate::session::UiEvent::Logout { full, .. } => {
                    let to_lobby = *full
                        && self
                            .core
                            .session
                            .as_ref()
                            .is_some_and(|session| session.machine.logged_in);
                    self.client_logout(to_lobby);
                    return;
                }
                // Only lobby/title reads reach here (the game reader answers
                // them in `poll_live_once`); they stay queued until
                // preparing for the map discards them.
                crate::session::UiEvent::ReflectionProbe(_) => {}
                crate::session::UiEvent::DoCheat { command } => {
                    // A console cheat from the server.
                    if let Err(error) = self.script_cheat(command) {
                        log::warn!("[client910] DO_CHEAT failed: {error:#}");
                    }
                }
                crate::session::UiEvent::ExecuteClientCheat { id } => {
                    self.execute_client_cheat(*id);
                }
                crate::session::UiEvent::Js5Reload => {
                    self.js5_reload();
                    return;
                }
                crate::session::UiEvent::MalformedPacket {
                    opcode,
                    size,
                    reason,
                } => {
                    // A packet that does not decode: report its ids and size,
                    // then log out (not to the lobby).
                    log::warn!(
                        "[client910] malformed server packet {} (opcode {opcode}, {size} bytes: {reason}); logout",
                        crate::proto::server::name(*opcode)
                    );
                    self.client_logout(false);
                    return;
                }
                crate::session::UiEvent::UnhandledPacket { opcode, size } => {
                    // Report the packet ids and size, then log out (not to the
                    // lobby).
                    log::warn!(
                        "[client910] unhandled server packet {} (opcode {opcode}, {size} bytes); logout",
                        crate::proto::server::name(*opcode)
                    );
                    self.client_logout(false);
                    return;
                }
                _ => {}
            }
        }
    }

    /// Per-frame live poll (online only, never blocks, never `block_on`s).
    /// 1. Non-blocking socket drain (`try_read`/`try_write` +
    ///    `drain_pending_sync`): collects `REBUILD_NORMAL` + live interface /
    ///    script frames without touching a runtime.
    /// 2. Applies interface / script frames to `OpenInterfaces` + `VmAdapter`
    ///    (real VM; `onLoad`s run once per batch).
    /// 3. Map completion runs before this method, in the logic-loop order.
    /// 4. On a rebuild whose groups differ (`should_reload`) sends a
    ///    [`PrefetchRequest`] to the worker (non-blocking `send`).
    ///
    /// Same-group repeats are no-ops; any failure keeps the rendered terrain.
    /// An I/O failure triggers the reconnect attempt.
    pub(super) fn poll_live_rebuild(&mut self, io: &mut dyn Io) {
        if self.renderer.is_none() {
            return;
        }
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        let LivePoll {
            rebuild: rebuild_opt,
            session_events,
            effects,
        } = match poll_live_session(session, io) {
            LivePollOutcome::Idle => return,
            LivePollOutcome::Lost(reason) => {
                self.client_try_reconnect(&reason);
                return;
            }
            LivePollOutcome::Polled(poll) => poll,
        };
        self.apply_effects(effects);
        if !session_events.is_empty() {
            let state = self.core.session.as_ref().map(|s| s.machine.state);
            self.apply_session_events(&session_events);
            if self.core.session.as_ref().map(|s| s.machine.state) != state {
                log::info!("[client910] session transition received; live polling stopped");
                return;
            }
        }
        let Some(event) = rebuild_opt else {
            return;
        };
        let current = match self.core.session.as_ref() {
            Some(session) => session.groups.clone(),
            None => return,
        };
        if self.core.session.as_ref().is_none_or(|s| s.game.is_none())
            && !crate::session::should_reload(&current, &event.groups)
        {
            return;
        }
        log::info!(
            "[client910] live reload: zone=({},{}) groups {current:?} -> {:?}",
            event.rebuild.zone_x,
            event.rebuild.zone_z,
            event.groups,
        );
        self.reload_for_event(event);
    }

    /// Queue one [`crate::session::RebuildEvent`]: it waits on
    /// the maps JS5 groups being ready ([`ViewerApp::poll_js5_map_wait`]), then goes
    /// to the background worker (non-blocking `send`, no runtime here).
    /// A failed `load_groups`/asset build later keeps the current terrain
    /// (the consumed rebuild is NOT marked rendered, so a later identical
    /// rebuild retries the load). Duplicate requests for the in-flight groups
    /// are no-ops.
    pub(super) fn reload_for_event(&mut self, event: crate::session::RebuildEvent) {
        self.enter_rebuild_state();
        // A fresh environment manager (default fade, reset), the override
        // carried over.
        self.core.environment.fade_reset = true;
        self.core.environment.fade = None;
        self.core.environment.fade_target = None;
        let Some(session) = self.core.session.as_mut() else {
            return;
        };
        if let Some(game) = &mut session.game {
            game.rebuild_started_ms
                .get_or_insert_with(crate::logic_clock::monotonic_millis);
        }
        if session.prefetch_dead {
            log::warn!("[client910] prefetch worker gone; keeping rendered terrain");
            return;
        }
        if let Some(loading) = &session.prefetch_loading {
            if !crate::session::should_reload(&loading.groups, &event.groups) {
                return;
            }
        }
        let groups = event.groups.clone();
        let total = groups.len();
        session.prefetch_loading = Some(PrefetchLoading {
            groups: groups.clone(),
            loaded: 0,
            total,
        });
        log::info!("[client910] rebuild waits on mapsJs5 for {groups:?}");
        self.js5_map_wait = Some(event);
        self.poll_js5_map_wait();
    }

    /// Drain all ready worker responses without blocking (`try_recv` loop):
    /// progress updates the title state, done builds `WorldAssets` on this
    /// (winit) thread — CPU-only, no network — and swaps via
    /// [`ViewerApp::apply_reloaded_assets`], error logs and clears.
    pub(super) fn poll_prefetch_responses(&mut self) {
        loop {
            let response = {
                let Some(session) = self.core.session.as_ref() else {
                    return;
                };
                if session.prefetch_dead {
                    return;
                }
                match session.prefetch_resp_rx.try_recv() {
                    Ok(response) => response,
                    Err(TryRecvError::Empty) => return,
                    Err(TryRecvError::Disconnected) => {
                        log::warn!(
                            "[client910] prefetch worker disconnected; keeping rendered terrain"
                        );
                        if let Some(session) = self.core.session.as_mut() {
                            session.prefetch_dead = true;
                        }
                        return;
                    }
                }
            };
            match response {
                PrefetchResponse::Progress(progress) => {
                    log::debug!(
                        "[client910] prefetch {}/{} groups {:?}",
                        progress.loaded,
                        progress.total,
                        progress.groups
                    );
                    if let Some(session) = self.core.session.as_mut() {
                        if let Some(loading) = session.prefetch_loading.as_mut() {
                            if loading.groups == progress.groups {
                                loading.loaded = progress.loaded;
                                loading.total = progress.total;
                            }
                        }
                    }
                }
                PrefetchResponse::Done { event, world: _ } => {
                    rs910_core::profile::scope!("region change");
                    if self.core.session.is_none() {
                        return;
                    }
                    let pack = self.pack.clone();
                    let (spawn_x, spawn_z) = spawn_for_rebuild(&event.rebuild);
                    rs910_core::profile::scope!("region loading box", self.begin_rebuild_build());
                    match build_assets_from_world(&pack, spawn_x, spawn_z) {
                        Ok(mut assets) => {
                            if let Some(game) = self.core.session.game_mut() {
                                match prepare_game_scene(game, &pack, &mut assets) {
                                    Ok(prepared) => {
                                        self.core.session.as_mut().unwrap().prepared_map =
                                            Some(prepared)
                                    }
                                    Err(error) => {
                                        log::warn!("[client910] map installation retained for retry: {error:#}");
                                        return;
                                    }
                                }
                            }
                            self.apply_reloaded_assets(assets, &event);
                            match self.finish_game_map() {
                                Ok(()) => self.leave_rebuild_state(),
                                Err(error) => {
                                    log::warn!("[client910] map not acknowledged: {error:#}");
                                }
                            }
                        }
                        Err(err) => {
                            log::warn!(
                                "[client910] prefetch asset build failed ({err:#}); keeping rendered terrain"
                            );
                        }
                    }
                    if let Some(session) = self.core.session.as_mut() {
                        let done_groups = event.groups.clone();
                        let clear = session
                            .prefetch_loading
                            .as_ref()
                            .is_some_and(|loading| loading.groups == done_groups);
                        if clear {
                            session.prefetch_loading = None;
                        }
                    }
                }
                PrefetchResponse::Error { groups, message } => {
                    log::warn!(
                        "[client910] prefetch load_groups failed ({message}); keeping rendered terrain"
                    );
                    if let Some(session) = self.core.session.as_mut() {
                        let clear = session
                            .prefetch_loading
                            .as_ref()
                            .is_some_and(|loading| loading.groups == groups);
                        if clear {
                            session.prefetch_loading = None;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::active_toolkit::RendererKind;

    /// The real map transaction must finish after CPU scene installation under
    /// modern, while faithful and toolkit 0 still require their uploaded meshes.
    #[test]
    #[ignore = "needs a GPU adapter and the cache"]
    fn a_modern_map_is_acknowledged_without_faithful_scene_meshes() -> anyhow::Result<()> {
        use super::super::session_replay::{Replay, Trace, FIXTURE};
        const DEVICE_SIZE: (u32, u32) = (64, 48);
        let trace = Trace::load(&rs910_core::test_support::client_dir().join(FIXTURE))?;
        let (mut app, _keep) = Replay::start(&trace)?.into_app();
        app.renderer = Some(pollster::block_on(ActiveToolkit::headless(
            DEVICE_SIZE,
            RendererKind::Modern,
        ))?);
        app.scene.pack_root = Some(PathBuf::new());
        let pack = app.pack.clone();
        let game = app.core.session.game_mut().context("game")?;
        let world = game.runtime.installed_world.clone();
        game.runtime.request_map(world, Default::default());
        game.rebuild_started_ms = Some(crate::logic_clock::monotonic_millis());
        let prepared = game.runtime.prepare_map(&pack)?;
        let session = app.core.session.as_mut().context("session")?;
        session.prepared_map = Some(prepared);
        session.io.world.pending_writes.clear();
        app.upload_floors();
        assert!(app.scene.meshes.floor_meshes.iter().all(Option::is_none));
        let material_store = app.scene.material_store.take();
        assert!(app.finish_game_map().is_err(), "CPU scene must be complete");
        app.scene.material_store = material_store;
        app.renderer.as_mut().unwrap().set_toolkit0(true);
        assert!(
            app.finish_game_map().is_err(),
            "toolkit 0 needs faithful meshes"
        );
        app.renderer.as_mut().unwrap().set_toolkit0(false);
        app.finish_game_map()?;
        let session = app.core.session.as_ref().unwrap();
        assert!(session.prepared_map.is_none());
        assert!(session.game.as_ref().unwrap().runtime.map_request.is_none());
        assert_eq!(
            session.io.world.pending_writes,
            crate::net::encode_map_build_complete(0)
        );
        Ok(())
    }
}
