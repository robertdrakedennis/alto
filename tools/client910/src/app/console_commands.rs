//! The developer console's local commands: the ones the client answers
//! itself (memory, camera position, window mode, toolkit, connection drops,
//! login shortcuts, the output file and scripts). Any other line goes to the
//! server as a cheat (`ConsoleCommands::command`).
//!
//! The commands that change client state are gated: they run when the
//! client is not a live-world client, when the account is staff level 2 or
//! more, or when [`COMMANDS_ANYWHERE`] opens them to everyone.
use super::*;
use crate::console::{text, Console, Host as _};
use rs910_core::texts::Msg;

/// This build opens the state-changing console commands to every account. A
/// build that sets it to false closes them to live-world accounts below
/// staff level 2 (see [`commands_allowed`]).
pub(super) const COMMANDS_ANYWHERE: bool = true;

/// Whether the gated console commands run: outside the live world mode
/// (`mode_where` 0), for staff level 2 and above, or when `anywhere` is set.
pub(super) fn commands_allowed(mode_where: Option<i32>, staff_level: i32, anywhere: bool) -> bool {
    mode_where.unwrap_or(0) != 0 || staff_level >= 2 || anywhere
}

/// The most memory the client may use, in megabytes, as the console and a
/// crash report print it: this process has no heap limit, so the peak
/// resident set stands in.
pub(super) fn max_memory_mb() -> i64 {
    crate::debug_overlay::process_memory_kb().map_or(0, |(_, peak)| peak / 1024 + 1)
}

impl ConsoleCommands<'_> {
    fn say(&mut self, c: &mut Console, line: &str) {
        c.add(self, &text(line));
    }

    /// The client state, 0 without a session.
    fn state(&self) -> i32 {
        self.app
            .core
            .session
            .as_ref()
            .map_or(0, |s| s.machine.state)
    }

    /// `Success` or `Failure` for a command that asked for a change.
    fn report(&mut self, c: &mut Console, done: bool) {
        self.say(c, if done { "Success" } else { "Failure" });
    }

    /// The commands that read client state and run for everyone: `heap` and
    /// `getcamerapos`. True when `command` was one.
    pub(super) fn read_only_command(
        &mut self,
        c: &mut Console,
        command: &str,
    ) -> anyhow::Result<bool> {
        if command == "heap" {
            self.say(c, &format!("Heap: {}MB", max_memory_mb()));
            return Ok(true);
        }
        if command.eq_ignore_ascii_case("getcamerapos") {
            for line in self.camera_position_lines()? {
                self.say(c, &line);
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// `Pos:` and `Look:` lines of the camera in use: the scripted camera's
    /// eye and focus, else the classic camera's pose and look target. The
    /// look height of the scripted camera is read at the eye's depth, as the
    /// client's own report does.
    fn camera_position_lines(&self) -> anyhow::Result<[String; 2]> {
        let session = self.app.core.session.as_ref().context("no session")?;
        let game = session.game.as_ref().context("not in a world")?;
        let map = &game.runtime.map;
        let level = game.runtime.feed.state.players.current_level;
        let terrain = game.runtime.terrain.as_ref().context("no terrain")?;
        let height = |x: i32, z: i32| terrain.fine_height(x, z, level).unwrap_or(0);
        let cam2 = &session.ui.engine.camera.cam2;
        let coord = |x: i32, z: i32| format!("{level},{},{},{},{}", x >> 6, z >> 6, x & 63, z & 63);
        if cam2.camera_state == 3 {
            let eye = cam2.eye().context("the camera has no position")?;
            let look = cam2.lookat_point().context("the camera has no focus")?;
            let (ex, ey, ez) = (eye.x as i32, eye.y as i32, eye.z as i32);
            let (lx, lz) = (look.x as i32, look.z as i32);
            let local = |v: i32, base: i32| v - (base << 9);
            return Ok([
                format!(
                    "Pos: {} Height: {}",
                    coord(ex >> 9, ez >> 9),
                    height(local(ex, map.base_x), local(ez, map.base_z)) + ey
                ),
                format!(
                    "Look: {} Height: {}",
                    coord(lx >> 9, lz >> 9),
                    height(local(lx, map.base_x), local(ez, map.base_z)) + ey
                ),
            ]);
        }
        let pose = cam2.legacy.pose;
        let look = cam2.legacy.look_at.unwrap_or_default();
        let (cx, cz) = ((pose.x >> 9) + map.base_x, (pose.z >> 9) + map.base_z);
        Ok([
            format!(
                "Pos: {} Height: {}",
                coord(cx, cz),
                height(pose.x, pose.z) - pose.y
            ),
            format!(
                "Look: {} Height: {}",
                coord(look.x + map.base_x, look.z + map.base_z),
                height(look.x, look.z) - look.height
            ),
        ])
    }

    /// The commands that change client state. True when `command` was one;
    /// a command that runs but is refused answers `Failure`.
    pub(super) fn gated_command(&mut self, c: &mut Console, command: &str) -> anyhow::Result<bool> {
        let lower = command.to_ascii_lowercase();
        match lower.as_str() {
            "wm1" | "wm2" => {
                let mode = i32::from(lower.as_bytes()[2] - b'0');
                let done = self.set_window_mode(mode)?;
                self.report(c, done);
                return Ok(true);
            }
            "wm3" if self.fullscreen_allowed() => {
                let done = self.set_window_mode(3)?;
                self.report(c, done);
                return Ok(true);
            }
            "tk0" | "tk1" | "tk3" | "tk5" => {
                let id = i32::from(lower.as_bytes()[2] - b'0');
                let game = self.app.core.session.game_mut().context("no game")?;
                let done = game
                    .ui_variables
                    .queries
                    .preferences
                    .console_set_toolkit(id);
                self.report(c, done);
                return Ok(true);
            }
            "clientdrop" => {
                match self.state() {
                    crate::login_state::GAME => self.app.client_try_reconnect("clientdrop"),
                    crate::login_state::REBUILD_GAME => {
                        if let Some(stream) = self
                            .app
                            .core
                            .session
                            .as_ref()
                            .and_then(|s| s.io.world.stream.as_ref())
                        {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                        }
                    }
                    _ => {}
                }
                return Ok(true);
            }
            "breakcon" => {
                if let Some(session) = self.app.core.session.as_ref() {
                    for stream in [&session.io.world.stream, &session.io.lobby.stream]
                        .into_iter()
                        .flatten()
                    {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                }
                self.app.js5.tcp.close_forcefully();
                return Ok(true);
            }
            "closeoutput" => {
                self.app.console.output = None;
                return Ok(true);
            }
            _ => {}
        }
        if let Some(rest) = command.strip_prefix("setlobby ") {
            self.set_lobby(c, rest);
            return Ok(true);
        }
        if let Some(rest) = command.strip_prefix("getclientvarpbit") {
            let line = self.client_var(rest.get(1..).context("variable id")?.parse()?, true)?;
            self.say(c, &line);
            return Ok(true);
        }
        if let Some(rest) = command.strip_prefix("getclientvarp") {
            let line = self.client_var(rest.get(1..).context("variable id")?.parse()?, false)?;
            self.say(c, &line);
            return Ok(true);
        }
        if command.starts_with("directlogin") {
            self.direct_login(command.get(12..).context("login details")?);
            return Ok(true);
        }
        if let Some(rest) = command.strip_prefix("snlogin ") {
            self.social_login(rest)?;
            return Ok(true);
        }
        if let Some(path) = command.strip_prefix("setoutput ") {
            self.set_output(c, path);
            return Ok(true);
        }
        if let Some(path) = command.strip_prefix("runscript ") {
            self.run_script(c, path);
            // The line is sent to the server as well, as for any command.
            return Ok(false);
        }
        Ok(false)
    }

    /// Whether the exclusive fullscreen mode can be entered: there is a
    /// native window to do it with.
    fn fullscreen_allowed(&self) -> bool {
        self.app.core.session.game().is_some_and(|game| {
            game.ui_variables
                .queries
                .preferences
                .window
                .native
                .is_some()
        })
    }

    /// Switch the window to `mode` (1 and 2 windowed, 3 fullscreen at
    /// 1024x768) and say whether it took.
    fn set_window_mode(&mut self, mode: i32) -> anyhow::Result<bool> {
        let game = self.app.core.session.game_mut().context("no game")?;
        let p = &mut game.ui_variables.queries.preferences;
        let (command, mut ints) = if mode == 3 {
            ("fullscreen_enter", vec![1024, 768])
        } else {
            ("setwindowmode", vec![mode])
        };
        p.window
            .dispatch(command, &mut ints, &mut p.options, &mut p.dirty)
            .context("window command")?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        Ok(p.window.mode == mode)
    }

    /// `getclientvarp`, `getclientvarpbit`: one player variable of the
    /// logged-in game.
    fn client_var(&mut self, id: i32, bit: bool) -> anyhow::Result<String> {
        let game = self.app.core.session.game().context("game not ready")?;
        let vars = game
            .runtime
            .feed
            .state
            .varps
            .as_ref()
            .context("varps unavailable")?;
        let value = if bit {
            let def = game
                .inputs
                .bits
                .get(id, false)
                .map_err(|e| anyhow::anyhow!("varbit: {e:?}"))?;
            vars.get_bit(&def)
        } else {
            vars.get(id)
        }
        .map_err(|e| anyhow::anyhow!("varp: {e:?}"))?;
        Ok(format!("{}={value}", if bit { "varpbit" } else { "varp" }))
    }

    /// `setlobby <node> <host>`: on the title screen, point the lobby at
    /// `<host>` of the site's domain, as lobby number `<node>`.
    fn set_lobby(&mut self, c: &mut Console, rest: &str) {
        let parsed = rest
            .split_once(' ')
            .and_then(|(id, host)| Some((id.parse::<i32>().ok()?, host.trim())));
        let Some((id, host)) = parsed.filter(|_| self.state() == crate::login_state::LOGIN) else {
            self.report(c, false);
            return;
        };
        let domain = rs910_core::applet_params::get().site_domain().to_owned();
        if let Some(session) = self.app.core.session.as_mut() {
            session.current_lobby.host = format!("{host}.{domain}");
            session.current_lobby.node = id.wrapping_add(1099);
        }
        self.report(c, true);
    }

    /// `directlogin <name> <password> [code]`: log in to the world as the
    /// title screen's login button would, cancelling a login in progress.
    fn direct_login(&mut self, details: &str) {
        let parts: Vec<&str> = details.split(' ').collect();
        if !(2..=3).contains(&parts.len()) {
            return;
        }
        // A name beyond 320 characters is not a login.
        if parts[0].encode_utf16().count() > 320 {
            return;
        }
        if self
            .app
            .core
            .session
            .as_ref()
            .is_some_and(|s| s.reconnect_started)
        {
            self.app.cancel_login_request();
        }
        self.app
            .start_login_request(crate::ui_runtime::LoginRequest {
                username: parts[0].to_owned(),
                password: parts[1].to_owned(),
                new_auth_preference: parts.get(2).copied().unwrap_or("").to_owned(),
                auth_dont_trust: true,
                lobby: false,
                sso: None,
            });
    }

    /// `snlogin <network> [token]`: log in to the world through a social
    /// network.
    fn social_login(&mut self, details: &str) -> anyhow::Result<()> {
        let parts: Vec<&str> = details.split(' ').collect();
        let network: i32 = parts[0].parse()?;
        self.app
            .start_login_request(crate::ui_runtime::LoginRequest {
                username: String::new(),
                password: String::new(),
                new_auth_preference: parts.get(1).copied().unwrap_or("").to_owned(),
                auth_dont_trust: true,
                lobby: false,
                sso: Some(network),
            });
        Ok(())
    }

    /// `setoutput <file>`: write every console line to a file; a file that
    /// exists is not overwritten but replaced by a numbered log next to it.
    fn set_output(&mut self, c: &mut Console, path: &str) {
        use std::path::PathBuf;
        let mut target = PathBuf::from(path);
        if target.exists() {
            let stamp = self.now();
            target = PathBuf::from(format!("{path}.{stamp}.log"));
            if target.exists() {
                self.say(c, "file already exists!");
                return;
            }
        }
        self.app.console.output = None;
        let name = target
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        match std::fs::File::create(&target) {
            Ok(file) => self.app.console.output = Some(file),
            Err(_) => self.say(c, &format!("Could not create {name}")),
        }
    }

    /// `runscript <file>`: run the lines of a file as console input, pausing
    /// where it says `pause <seconds>`.
    fn run_script(&mut self, c: &mut Console, path: &str) {
        let path = std::path::Path::new(path);
        if !path.exists() {
            self.say(c, "No such file");
            return;
        }
        let Ok(bytes) = std::fs::read(path) else {
            self.say(c, "Failed to read file");
            return;
        };
        let content: String = bytes
            .iter()
            .map(|&b| rs910_core::cp1252::cp1252_decode_byte(b))
            .filter(|&ch| ch != '\r')
            .collect();
        let lines = content.split('\n').map(text).collect();
        c.paste_lines(self, lines);
    }

    /// A line the console printed, written to the output file.
    pub(super) fn write_output(&mut self, line: &[u16]) {
        use std::io::Write as _;
        let Some(file) = self.app.console.output.as_mut() else {
            return;
        };
        let bytes: Vec<u8> = line
            .iter()
            .map(|&unit| rs910_core::cp1252::cp1252_encode_unit(unit))
            .collect();
        if file.write_all(&bytes).is_err() {
            self.app.console.output = None;
        }
    }

    /// The `Unknown developer command:` line for `command`.
    pub(super) fn unknown_command(&mut self, c: &mut Console, command: &str) {
        self.say(
            c,
            &format!("{}{command}", Msg::DebugConsoleUnknownCommand.get()),
        );
    }
}
