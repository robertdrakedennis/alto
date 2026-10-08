//! `ViewerApp::console` (code-quality programme Phase 4.4).
use super::*;

/// The developer console and its shell inputs: the keys and wheel the
/// window events queue for the console update, the `--console-command`
/// replay and the lines queued for it.
pub(super) struct ConsoleHost {
    pub(super) developer: crate::console::Console,
    pub(super) keys: Vec<crate::console::Key>,
    pub(super) commands: std::collections::VecDeque<String>,
    pub(super) pending_messages: Vec<String>,
    pub(super) next_cycle: i32,
    pub(super) wheel: i32,
    /// The file `setoutput` writes every console line to.
    pub(super) output: Option<std::fs::File>,
    /// Whether the gated local commands are open to every account
    /// ([`super::console_commands::COMMANDS_ANYWHERE`]).
    pub(super) commands_anywhere: bool,
}

pub(super) struct ConsoleCommands<'a> {
    pub(super) app: &'a mut ViewerApp,
}

impl crate::console::Host for ConsoleCommands<'_> {
    fn now(&mut self) -> i64 {
        crate::logic_clock::monotonic_millis()
    }
    fn paste(&mut self) -> Option<crate::console::Text> {
        Some(crate::clipboard::get_text()?.encode_utf16().collect())
    }
    fn copy(&mut self, text: crate::console::Text) {
        crate::clipboard::set_text(&String::from_utf16_lossy(&text));
    }
    fn log(&mut self, text: crate::console::Text) {
        self.write_output(&text);
    }
    fn command(
        &mut self,
        c: &mut crate::console::Console,
        units: crate::console::Text,
        suggest: bool,
    ) {
        let command = String::from_utf16_lossy(&units);
        if let Err(error) = self.run(c, &command, &units, suggest) {
            log::debug!("[client910] console command {command:?}: {error:#}");
            c.add(
                self,
                &crate::console::text(rs910_core::texts::Msg::DebugConsoleError.get()),
            );
        }
    }
}

impl ConsoleCommands<'_> {
    /// One console line: the commands every client answers, then the gated
    /// local commands, then the server's (`console_commands`).
    fn run(
        &mut self,
        c: &mut crate::console::Console,
        command: &str,
        units: &[u16],
        suggest: bool,
    ) -> anyhow::Result<()> {
        use crate::console::text;
        if command.eq_ignore_ascii_case("cls") {
            c.count = 0;
            c.scroll = 0;
            return Ok(());
        }
        if command.eq_ignore_ascii_case("help") || command.eq_ignore_ascii_case("commands") {
            for line in [
                "commands - This command",
                "cls - Clear console",
                "displayfps - Toggle FPS and other information",
                "renderer - Print graphics renderer information",
                "heap - Print memory information",
                "getcamerapos - Print location and direction of camera for use in bug reports",
            ] {
                c.add(self, &text(line));
            }
            return Ok(());
        }
        if command.eq_ignore_ascii_case("displayfps") {
            let ui = self
                .app
                .core
                .session
                .ui_mut()
                .context("no interface state")?;
            ui.state.debug_visible[0] = !ui.state.debug_visible[0];
            let on = ui.state.debug_visible[0];
            c.add(self, &text(if on { "FPS on" } else { "FPS off" }));
            return Ok(());
        }
        if command == "renderer" {
            let lines = self.renderer_lines()?;
            for line in lines {
                c.add(self, &text(&line));
            }
            return Ok(());
        }
        // The modern renderer's render scale (not an original command): with
        // no argument, the scale in use; `auto` or 50..200 sets it and saves
        // the choice (`crate::modern_display`).
        if command == "renderscale" || command.starts_with("renderscale ") {
            let path = crate::modern_display::path(&self.app.cli.pack_root);
            let argument = command["renderscale".len()..].trim();
            let line = if argument.is_empty() {
                match crate::modern_display::load(&path) {
                    Some(scale) => format!("renderscale {}", scale.as_percent()),
                    None => "renderscale auto".to_string(),
                }
            } else {
                match crate::modern_display::parse_argument(argument) {
                    Ok(scale) => {
                        let mut preferences = self.app.renderer.as_ref().map_or_else(
                            || crate::modern_display::load_preferences(&path),
                            |r| r.modern_preferences(),
                        );
                        preferences.render_scale = scale;
                        match self.app.change_modern_preferences(preferences) {
                            Ok(()) => format!(
                                "renderscale {}",
                                scale.map_or("auto".to_string(), |s| s.as_percent().to_string())
                            ),
                            Err(error) => {
                                log::warn!("[client910] save the render scale: {error:#}");
                                "The render scale could not be saved.".to_string()
                            }
                        }
                    }
                    Err(usage) => usage.to_string(),
                }
            };
            c.add(self, &text(&line));
            return Ok(());
        }
        // The engine profiler's readout (lane E-A4; not an original command).
        // Only a `--features profile` build answers it; the default build
        // treats `prof` like any other command.
        if rs910_core::profile::COMPILED && (command == "prof" || command.starts_with("prof ")) {
            for line in profile_readout(&command[4..]) {
                c.add(self, &text(&line));
            }
            return Ok(());
        }
        if self.read_only_command(c, command)? {
            return Ok(());
        }
        let state = self
            .app
            .core
            .session
            .as_ref()
            .map_or(0, |s| s.machine.state);
        let staff = self
            .app
            .core
            .session
            .as_ref()
            .map_or(0, |s| s.ui.engine.account.staff_mod_level);
        let anywhere = self.app.console.commands_anywhere;
        let sendable = matches!(state, crate::login_state::GAME | crate::login_state::LOBBY);
        let allowed = super::console_commands::commands_allowed(
            rs910_core::applet_params::get().mode_where().ok().flatten(),
            staff,
            anywhere,
        );
        if allowed {
            if self.gated_command(c, command)? {
                return Ok(());
            }
            if sendable {
                self.send_cheat(c, units, suggest);
            }
        }
        // Without a connection to send it on, the line is unknown.
        if !sendable && !anywhere {
            self.unknown_command(c, command);
        }
        Ok(())
    }

    /// The `renderer` command's lines: the toolkit and what the adapter
    /// reports.
    fn renderer_lines(&mut self) -> anyhow::Result<Vec<String>> {
        let report = self
            .app
            .renderer
            .as_ref()
            .context("no renderer")?
            .adapter_report();
        let toolkit = self
            .app
            .core
            .session
            .game()
            .and_then(|g| {
                g.ui_variables
                    .queries
                    .preferences
                    .options
                    .get("displayMode")
            })
            .context("no toolkit id")?;
        Ok(vec![
            format!("Toolkit ID: {toolkit}"),
            format!("Vendor: {}", report.vendor),
            format!("Name: {}", report.name),
            format!("Version: {}", report.version),
            format!("Device: {}", report.device),
            format!("Driver Version: {}", report.driver_version),
        ])
    }

    /// Send the line to the server as a cheat, on the world connection in
    /// the game and on the lobby connection in the lobby.
    fn send_cheat(&mut self, c: &mut crate::console::Console, units: &[u16], suggest: bool) {
        let Some(session) = self.app.core.session.as_mut() else {
            return;
        };
        let lobby = session.machine.state == crate::login_state::LOBBY;
        if !lobby && session.polling_dead {
            return;
        }
        match crate::client_command::remote_units(units, false, suggest) {
            Ok(packet) => {
                let io = &mut session.io;
                let connection = if lobby { &mut io.lobby } else { &mut io.world };
                connection.pending_writes.extend(packet);
                log::info!(
                    "[client910] console command queued: {}",
                    String::from_utf16_lossy(units)
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                );
            }
            Err(_) => c.add(
                self,
                &crate::console::text(rs910_core::texts::Msg::DebugConsoleError.get()),
            ),
        }
    }
}

/// `prof [on|off|gpu on|gpu off|reset|last|worst|N]`: the engine profiler
/// (`rs910_core::profile`) or its GPU pass timing switched on or off, or
/// its readout: the last `N`
/// frames' times and top scopes (default 120), or the scope tree of the
/// last or the slowest frame.
fn profile_readout(args: &str) -> Vec<String> {
    use rs910_core::profile;
    let args = args.trim();
    match args {
        "on" => {
            profile::enable();
            return vec!["profiler on".into()];
        }
        "off" => {
            profile::disable();
            return vec!["profiler off".into()];
        }
        "gpu on" | "gpu off" => {
            profile::set_gpu_timing(args == "gpu on");
            return vec![format!("GPU pass timing {}", &args[4..])];
        }
        _ => {}
    }
    if !profile::enabled() {
        return vec!["profiler off (prof on)".into()];
    }
    profile::with_profiler(|p| match args {
        "reset" => {
            p.reset();
            vec!["profiler reset".into()]
        }
        "last" | "worst" => {
            let frame = if args == "last" {
                p.frames().last()
            } else {
                p.worst()
            };
            frame.map_or_else(
                || vec!["no frame yet".into()],
                |f| profile::frame_lines(f, 50_000),
            )
        }
        _ => p.summary(args.parse().unwrap_or(120)).lines(12),
    })
    .unwrap_or_else(|| vec!["profiler not started on this thread".into()])
}

impl ViewerApp {
    /// Apply one CS2 request from `ui_runtime::host_builtins`; true on `quit`.
    pub(super) fn apply_script_request(
        &mut self,
        request: crate::ui_runtime::host_builtins::Request,
    ) -> bool {
        use crate::ui_runtime::host_builtins::Request;
        match request {
            // The quit script command: the window-close path.
            Request::Quit => return true,
            // Replace the current entry.
            Request::ConsoleEntry(entry) => {
                self.console
                    .developer
                    .set_entry(crate::console::text(&entry));
            }
            // A console cheat through the
            // console command owner (local commands, then CLIENT_CHEAT).
            Request::Cheat(command) => {
                if let Err(error) = self.script_cheat(&command) {
                    log::warn!("[client910] docheat failed: {error:#}");
                }
            }
        }
        false
    }
    pub(super) fn script_cheat(&mut self, command: &str) -> anyhow::Result<()> {
        use crate::console::Host as _;
        let mut console = std::mem::take(&mut self.console.developer);
        let result = (|| {
            if console.lines.is_none() {
                // Adding a line initialises the line buffer.
                self.scene
                    .pack_root
                    .as_ref()
                    .context("console requires the real font cache")?;
                let sizes = self
                    .renderer
                    .as_mut()
                    .context("renderer not ready")?
                    .console_sizes(&self.pack)?;
                console.init(&mut ConsoleCommands { app: self }, sizes);
            }
            ConsoleCommands { app: self }.command(
                &mut console,
                crate::console::text(command),
                false,
            );
            anyhow::Ok(())
        })();
        self.console.developer = console;
        result
    }
    /// Adds a console line, initialising the line buffer on first use.
    pub(super) fn console_addline(&mut self, line: &str) -> anyhow::Result<()> {
        let mut console = std::mem::take(&mut self.console.developer);
        let result = (|| {
            if console.lines.is_none() {
                self.scene
                    .pack_root
                    .as_ref()
                    .context("console requires the real font cache")?;
                let sizes = self
                    .renderer
                    .as_mut()
                    .context("renderer not ready")?
                    .console_sizes(&self.pack)?;
                console.init(&mut ConsoleCommands { app: self }, sizes);
            }
            console.add(
                &mut ConsoleCommands { app: self },
                &crate::console::text(line),
            );
            anyhow::Ok(())
        })();
        self.console.developer = console;
        result
    }

    /// The context string a crash report appends.
    pub(super) fn crash_context(&self) -> String {
        let mut out = String::from(" ");
        let Some(session) = self.core.session.as_ref() else {
            return out;
        };
        let (base, size, level, route) = match session.game.as_ref() {
            Some(game) => {
                let map = &game.runtime.map;
                let player = game.runtime.feed.state.players.players[map.local].as_ref();
                (
                    [map.base_x, map.base_z],
                    [map.width, map.height],
                    player.map_or(0, |p| p.level),
                    player.map(|p| [map.base_x + p.x[0], map.base_z + p.z[0]]),
                )
            }
            None => ([0, 0], [0, 0], 0, None),
        };
        out.push_str(&format!("{},{},{},{} ", base[0], base[1], size[0], size[1]));
        match route {
            Some([x, z]) => out.push_str(&format!("{level},{x},{z} ")),
            None => out.push_str(&format!("{level},{level},{level}, ")),
        }
        let options = session
            .game
            .as_ref()
            .map(|g| &g.ui_variables.queries.preferences.options);
        let get = |name: &str| options.and_then(|o| o.get(name)).unwrap_or(0);
        let (window_mode, canvas) = (
            session.ui.engine.platform.window_mode,
            session.ui.state.layout.canvas,
        );
        out.push_str(&format!(
            "{} {} {} {},{} ",
            get("displayMode"),
            get("antiAliasing"),
            window_mode,
            canvas[0],
            canvas[1]
        ));
        for name in [
            "lightingDetail",
            "sceneryShadows",
            "waterDetail",
            "textures",
            "bloom",
        ] {
            out.push_str(&format!("{} ", get(name)));
        }
        out.push_str("0 ");
        out.push_str(&format!(
            "{} {} ",
            super::console_commands::max_memory_mb(),
            session.machine.state
        ));
        // `hardwarePlatform == null` (no native probe): -1, then the null
        // gamepack's ",".
        out.push_str("-1 ,");
        out
    }

    pub(super) fn execute_client_cheat(&mut self, id: u16) {
        log::info!(
            "[client910] EXECUTE_CLIENT_CHEAT {id} in client state {}",
            self.core.session.as_ref().map_or(0, |s| s.machine.state)
        );
        // These ids raise a fatal error out of the packet read; the frame loop
        // reports it as a crash and shuts down.
        if id == 28 || id == 2 {
            log::warn!(
                "[client910] EXECUTE_CLIENT_CHEAT {id}: fatal error{}; error_game_crash",
                self.crash_context()
            );
            self.lifecycle.shutdown_requested = true;
            return;
        }
        let local = || -> Option<(f32, f32)> {
            let game = self.core.session.as_ref()?.game.as_ref()?;
            let player = game
                .runtime
                .feed
                .state
                .players
                .players
                .get(game.runtime.map.local)?
                .as_ref()?;
            Some((player.fine_x, player.fine_z))
        };
        let line = match id {
            // Close the console and redraw all interfaces.
            29 => {
                self.console.developer.open = false;
                return;
            }
            // Print the fps.
            11 => Some(crate::ui_runtime::host_builtins::game_shell_fps().to_string()),
            // Local player transform in scene tiles.
            19 => local().map(|(x, z)| format!("{} {}", (x as i32) >> 9, (z as i32) >> 9)),
            // The fps display on/off.
            25 | 21 => {
                if let Some(ui) = self.core.session.ui_mut() {
                    ui.state.debug_visible[0] = id == 25;
                }
                return;
            }
            // Toggles the DEBUG component; the hardware toolkits' matching
            // hooks are empty.
            13 => {
                if let Some(ui) = self.core.session.ui_mut() {
                    ui.state.debug_visible[1] = !ui.state.debug_visible[1];
                }
                return;
            }
            // Drops the idle cache entries, then prints the used memory in
            // KiB: this process's resident set (debug_overlay.rs).
            5 => {
                let dropped = self.remove_soft_references();
                log::info!(
                    "[client910] EXECUTE_CLIENT_CHEAT 5: dropped {dropped} idle cache entries"
                );
                crate::debug_overlay::process_memory_kb().map(|(used, _)| used.to_string())
            }
            // Toggles occlusion.
            24 => {
                if let Some(live) = self.scene.live.as_mut() {
                    live.toggle_occlusion();
                }
                return;
            }
            // The level height map tile height at the local player.
            16 => (|| {
                let game = self.core.session.as_ref()?.game.as_ref()?;
                let player = game
                    .runtime
                    .feed
                    .state
                    .players
                    .players
                    .get(game.runtime.map.local)?
                    .as_ref()?;
                let (x, z) = ((player.fine_x as i32) >> 9, (player.fine_z as i32) >> 9);
                game.runtime
                    .terrain
                    .as_ref()?
                    .fine_height(x << 9, z << 9, player.level)
                    .ok()
                    .map(|height| height.to_string())
            })(),
            // Clears the text coordinates.
            10 => {
                if let Some(game) = self.core.session.game_mut() {
                    game.runtime.feed.state.zones.text_coords.clear();
                }
                return;
            }
            // The used heap, then every static loc's model is released once,
            // then the used heap again.
            20 => {
                let before = crate::debug_overlay::process_memory_kb().map(|(used, _)| used);
                if let Some(used) = before {
                    if let Err(error) = self.console_addline(&used.to_string()) {
                        log::warn!("[client910] EXECUTE_CLIENT_CHEAT 20 console: {error:#}");
                    }
                }
                self.remove_soft_references();
                if let Some(session) = self.core.session.as_mut() {
                    if !session.scene_models_released {
                        // TODO(#gap-scene-release-models): releasing a loc model
                        // uploads then drops the CPU arrays that the model
                        // flags do not require; this port's scene
                        // graph keeps them for picking and bounds.
                        session.scene_models_released = true;
                    }
                }
                before
                    .and_then(|_| crate::debug_overlay::process_memory_kb())
                    .map(|(used, _)| used.to_string())
            }
            // Unloading the native libraries succeeds when none are loaded:
            // this port loads no native library, so the table is empty.
            9 => Some("Success".into()),
            // Configure the current world's socket type.
            23 => {
                if let Some(session) = self.core.session.as_mut() {
                    session.current_world.configure_socket_type();
                }
                return;
            }
            // Move the canvas to (50, 50) / back to its margins.
            26 | 22 => {
                if let Some(game) = self.core.session.game_mut() {
                    let window = &mut game.ui_variables.queries.preferences.window;
                    window.location = (id == 26).then_some([50, 50]);
                }
                return;
            }
            // Rebuild the world; 17 first arms the rebuild timer that the
            // scene rebuild prints.
            4 | 17 => {
                if let Some(session) = self.core.session.as_mut() {
                    if id == 17 {
                        session.rebuild_timer = Some(crate::logic_clock::monotonic_millis());
                    }
                }
                // A preference can enqueue the same rebuild asynchronously; this
                // port builds on the logic thread either way.
                self.world_rebuild();
                self.minimap.rebuild();
                return;
            }
            // The local player's tile height on its level, then the
            // component sprite and model caches' remaining and total weight.
            // The model line reports an untouched 50-entry cache: this port
            // draws no raw (type 1) component model through that cache.
            27 => {
                let sprites = match self.core.session.ui() {
                    Some(ui) => ui
                        .state
                        .sprites
                        .as_ref()
                        .map(crate::ui_sprites::Resources::sprite_cache_weights),
                    _ => None,
                };
                match sprites {
                    Some([available, capacity]) => {
                        if let Err(error) = self.console_addline(&format!("{available} {capacity}"))
                        {
                            log::warn!("[client910] EXECUTE_CLIENT_CHEAT 27 console: {error:#}");
                        }
                        Some("50 50".into())
                    }
                    None => None,
                }
            }
            // Reset the caches (without the full reset).
            8 => {
                if let Some(ui) = self.core.session.ui_mut() {
                    ui.reset_caches();
                }
                return;
            }
            // Set the scene debug mode 0/1/2 and rebuild the world.
            1 | 3 | 15 => {
                if let Some(session) = self.core.session.as_mut() {
                    session.scene_debug_mode = match id {
                        1 => 0,
                        3 => 1,
                        _ => 2,
                    };
                }
                self.world_rebuild();
                return;
            }
            // Close the JS5 connection: 6 asks the server to close the
            // stream, 14 closes it once its writer has drained. The JS5
            // client reconnects by itself.
            6 => {
                self.js5.tcp.send_close_stream();
                return;
            }
            14 => {
                self.js5.tcp.close_gracefully();
                return;
            }
            // Every other id has no case and does nothing.
            _ => return,
        };
        // any failure prints DEBUG_CONSOLE_ERROR.
        let line = line.unwrap_or_else(|| rs910_core::texts::Msg::DebugConsoleError.get().into());
        log::info!("[client910] EXECUTE_CLIENT_CHEAT {id}: console {line:?}");
        if let Err(error) = self.console_addline(&line) {
            log::warn!("[client910] EXECUTE_CLIENT_CHEAT {id} console: {error:#}");
        }
    }

    pub(super) fn toggle_console(&mut self) -> anyhow::Result<()> {
        self.scene
            .pack_root
            .as_ref()
            .context("console requires the real font cache")?;
        let sizes = self
            .renderer
            .as_mut()
            .context("renderer not ready")?
            .console_sizes(&self.pack)?;
        let mut console = std::mem::take(&mut self.console.developer);
        let opening = !console.open;
        console.toggle(&mut ConsoleCommands { app: self }, sizes);
        self.console.developer = console;
        if opening && self.console.developer.open && !self.console.pending_messages.is_empty() {
            let messages = std::mem::take(&mut self.console.pending_messages);
            let mut console = std::mem::take(&mut self.console.developer);
            for message in messages {
                console.add(
                    &mut ConsoleCommands { app: self },
                    &crate::console::text(&message),
                );
            }
            self.console.developer = console;
        }
        self.input.pressed.clear();
        self.input.dragging = false;
        self.input.last_cursor = None;
        Ok(())
    }
    /// Winit and the opt-in integration replay use the same key translation.
    pub(super) fn queue_console_key(
        &mut self,
        physical_key: PhysicalKey,
        entry_text: Option<&str>,
    ) {
        let modifiers = (self.input.modifiers.shift_key() as i32)
            | ((self.input.modifiers.alt_key() as i32) << 1)
            | ((self.input.modifiers.control_key() as i32) << 2);
        // The AWT keycode map and its patched entries.
        let code = match physical_key {
            PhysicalKey::Code(k) => match k {
                KeyCode::Enter | KeyCode::NumpadEnter => 84,
                KeyCode::Tab => 80,
                KeyCode::Backspace => 85,
                KeyCode::Delete => 101,
                KeyCode::ArrowLeft => 96,
                KeyCode::ArrowRight => 97,
                KeyCode::Home => 102,
                KeyCode::End => 103,
                KeyCode::PageUp => 104,
                KeyCode::PageDown => 105,
                KeyCode::KeyC if modifiers & 4 != 0 => 66,
                KeyCode::KeyV if modifiers & 4 != 0 => 67,
                _ => 0,
            },
            _ => 0,
        };
        if code != 0 {
            if self.console.keys.len() < 131 {
                self.console.keys.push(crate::console::Key {
                    code,
                    ch: 65535,
                    modifiers,
                });
            }
        } else if let Some(text) = entry_text {
            for ch in text.encode_utf16() {
                if self.console.keys.len() < 131 {
                    self.console.keys.push(crate::console::Key {
                        code: 0,
                        ch,
                        modifiers,
                    });
                }
            }
        }
    }
    pub(super) fn replay_console_command(&mut self) -> anyhow::Result<()> {
        if self.console.commands.is_empty() {
            return Ok(());
        }
        let ready = self.core.session.as_ref().is_some_and(|s| {
            !s.polling_dead
                && s.game.as_ref().is_some_and(|g| {
                    g.runtime.map_request.is_none()
                        && g.runtime.feed.state.initialized
                        && g.cycle >= self.console.next_cycle
                })
        });
        if !ready {
            return Ok(());
        }
        if !self.console.developer.open {
            self.toggle_console()?;
        }
        let command = self.console.commands.pop_front().unwrap();
        self.console.developer.set_entry(vec![]);
        for ch in command.chars() {
            let mut bytes = [0; 4];
            self.queue_console_key(
                PhysicalKey::Code(KeyCode::Space),
                Some(ch.encode_utf8(&mut bytes)),
            );
        }
        self.queue_console_key(PhysicalKey::Code(KeyCode::Enter), None);
        self.console.next_cycle = self.core.cycle.wrapping_add(150);
        Ok(())
    }
    pub(super) fn update_console(&mut self) {
        if !self.console.developer.open {
            self.console.keys.clear();
            self.console.wheel = 0;
            return;
        }
        let keys = std::mem::take(&mut self.console.keys);
        let wheel = std::mem::take(&mut self.console.wheel);
        let mut console = std::mem::take(&mut self.console.developer);
        console.tick(&mut ConsoleCommands { app: self }, &keys, wheel);
        self.console.developer = console;
    }
}
