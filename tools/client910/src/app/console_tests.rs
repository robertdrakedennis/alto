//! The developer console's local commands, through the real console host
//! (`ConsoleCommands`) over a replayed login: each command is typed as a
//! console line and its effect read from the client's own state.
use super::console_commands::commands_allowed;
use super::session_replay::{Replay, Trace, FIXTURE};
use super::*;
use crate::console::{text, Console};
use crate::login_state::{GAME, LOGIN, LOGIN_GAME, RECONNECT};
use crate::ui_goldens::settings_world::ScriptedWindow;

/// The client after the recorded login, in game, with its server socket.
pub(super) fn app_in_game(
) -> anyhow::Result<(ViewerApp, (std::net::TcpStream, impl Sized, impl Sized))> {
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let mut replay = Replay::start(&trace)?;
    for cycle in 1..=trace.last_cycle() {
        replay.cycle(&trace, cycle)?;
    }
    Ok(replay.into_app())
}

/// A console with its line buffer, as the first use makes it.
fn open_console(app: &mut ViewerApp) -> Console {
    let mut console = Console::default();
    console.init(&mut ConsoleCommands { app }, [16, 16]);
    console
}

/// Type `line` and return the lines it printed, oldest first, without their
/// time stamps.
fn run(app: &mut ViewerApp, console: &mut Console, line: &str) -> Vec<String> {
    use crate::console::Host as _;
    let before = console.count;
    ConsoleCommands { app }.command(console, text(line), false);
    let rows = console.lines.as_ref().unwrap();
    (0..console.count.saturating_sub(before))
        .rev()
        .map(|i| String::from_utf16_lossy(&rows[i][10..]))
        .collect()
}

fn session(app: &mut ViewerApp) -> &mut crate::client_core::Session {
    app.core.session.as_mut().unwrap()
}

fn preferences(app: &mut ViewerApp) -> &mut crate::ui_preferences::Preferences {
    &mut app
        .core
        .session
        .game_mut()
        .unwrap()
        .ui_variables
        .queries
        .preferences
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn local_commands_answer_in_game() -> anyhow::Result<()> {
    use std::io::Read as _;
    let (mut app, (mut peer, _dir, _clock)) = app_in_game()?;
    assert_eq!(session(&mut app).machine.state, GAME);
    let mut c = open_console(&mut app);

    // Memory and camera.
    let heap = run(&mut app, &mut c, "heap");
    assert!(
        heap[0].starts_with("Heap: ") && heap[0].ends_with("MB"),
        "{heap:?}"
    );
    let camera = run(&mut app, &mut c, "GetCameraPos");
    assert_eq!(camera.len(), 2, "{camera:?}");
    assert!(camera[0].starts_with("Pos: ") && camera[0].contains(" Height: "));
    assert!(camera[1].starts_with("Look: ") && camera[1].contains(" Height: "));
    assert_eq!(run(&mut app, &mut c, "help").len(), 6);

    // Window mode: windowed modes and exclusive fullscreen.
    assert_eq!(
        run(&mut app, &mut c, "wm1"),
        ["There was an error executing the command."],
        "no native window"
    );
    let window = ScriptedWindow::new(&[[1024, 768]]);
    preferences(&mut app).window.native = Some(window.clone());
    assert_eq!(run(&mut app, &mut c, "wm1"), ["Success"]);
    assert_eq!(preferences(&mut app).window.mode, 1);
    assert_eq!(run(&mut app, &mut c, "WM2"), ["Success"]);
    assert_eq!(preferences(&mut app).window.mode, 2);
    assert_eq!(run(&mut app, &mut c, "wm3"), ["Success"]);
    assert_eq!(preferences(&mut app).window.mode, 3);
    preferences(&mut app).window.native = Some(ScriptedWindow::refusing(&[[1024, 768]]));
    preferences(&mut app).window.mode = 2;
    assert_eq!(run(&mut app, &mut c, "wm3"), ["Failure"]);

    // Toolkit: a toolkit that can be created is the active and the saved
    // one, DirectX is refused.
    assert_eq!(run(&mut app, &mut c, "tk5"), ["Success"]);
    assert_eq!(preferences(&mut app).options.get("displayMode"), Some(5));
    assert_eq!(preferences(&mut app).options.get("toolkit"), Some(5));
    assert_eq!(run(&mut app, &mut c, "tk3"), ["Failure"]);
    assert_eq!(preferences(&mut app).options.get("displayMode"), Some(5));

    // Variables, and a server cheat that is not a local command.
    let varp = run(&mut app, &mut c, "getclientvarp 281");
    assert!(varp[0].starts_with("varp="), "{varp:?}");
    assert!(run(&mut app, &mut c, "getclientvarp x")[0].starts_with("There was an error"));
    let writes = session(&mut app).io.world.pending_writes.len();
    assert!(run(&mut app, &mut c, "tele 3200 3200 0").is_empty());
    let sent = &session(&mut app).io.world.pending_writes[writes..];
    assert_eq!(sent[0], crate::proto::client::CLIENT_CHEAT);
    assert!(sent.windows(4).any(|w| w == b"tele"));

    // The console's output file: every line, until it is closed; a file that
    // exists is kept and a numbered log takes its place.
    let dir = std::env::temp_dir().join(format!("alto-console-output-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("console.log");
    assert!(run(&mut app, &mut c, &format!("setoutput {}", file.display())).is_empty());
    run(&mut app, &mut c, "heap");
    assert!(run(&mut app, &mut c, "closeoutput").is_empty());
    run(&mut app, &mut c, "cls");
    run(&mut app, &mut c, "heap");
    assert!(std::fs::read_to_string(&file)?.contains(": Heap: "));
    assert_eq!(std::fs::read_to_string(&file)?.matches("Heap: ").count(), 1);
    assert!(run(&mut app, &mut c, &format!("setoutput {}", file.display())).is_empty());
    run(&mut app, &mut c, "heap");
    let numbered = std::fs::read_dir(&dir)?
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("console.log.") && n.ends_with(".log"))
        .count();
    assert_eq!(numbered, 1);
    run(&mut app, &mut c, "closeoutput");

    // A script runs its lines as console input.
    let script = dir.join("script.txt");
    std::fs::write(&script, "heap\r\ngetcamerapos\r\n")?;
    let before = c.count;
    run(&mut app, &mut c, &format!("runscript {}", script.display()));
    let rows = c.lines.as_ref().unwrap();
    let said: Vec<String> = (0..c.count - before)
        .rev()
        .map(|i| String::from_utf16_lossy(&rows[i][10..]))
        .collect();
    assert_eq!(said[0], "--> heap");
    assert!(said[1].starts_with("Heap: "));
    assert_eq!(said[2], "--> getcamerapos");
    assert!(said[3].starts_with("Pos: "));
    assert_eq!(
        run(
            &mut app,
            &mut c,
            &format!("runscript {}", dir.join("none").display())
        ),
        ["No such file"]
    );
    std::fs::remove_dir_all(&dir)?;

    // Dropping the connection: the world socket closes and the client
    // reconnects; `breakcon` closes every socket.
    app.console.commands_anywhere = true;
    assert!(run(&mut app, &mut c, "breakcon").is_empty());
    let mut byte = [0u8; 1];
    assert_eq!(peer.read(&mut byte)?, 0, "the server sees the socket close");
    assert!(run(&mut app, &mut c, "clientdrop").is_empty());
    assert_eq!(session(&mut app).machine.state, RECONNECT);
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn title_screen_commands_start_logins_and_set_the_lobby() -> anyhow::Result<()> {
    let (mut app, _keep) = app_in_game()?;
    let mut c = open_console(&mut app);
    // In game the lobby cannot be changed.
    assert_eq!(run(&mut app, &mut c, "setlobby 5 world"), ["Failure"]);

    session(&mut app).machine.state = LOGIN;
    session(&mut app).machine.logged_in = false;
    session(&mut app).reconnect_started = false;
    assert_eq!(run(&mut app, &mut c, "setlobby x world"), ["Failure"]);
    assert_eq!(run(&mut app, &mut c, "setlobby 5 world"), ["Success"]);
    let lobby = session(&mut app).current_lobby.clone();
    assert!(
        lobby.host.starts_with("world.") && lobby.node == 1104,
        "{lobby:?}"
    );

    // Too few details is not a login.
    assert!(run(&mut app, &mut c, "directlogin alice").is_empty());
    assert_eq!(session(&mut app).machine.state, LOGIN);
    assert!(run(&mut app, &mut c, "directlogin alice secret 123456").is_empty());
    let s = session(&mut app);
    assert_eq!(s.machine.state, LOGIN_GAME);
    assert_eq!(
        (s.username.as_str(), s.password.as_str()),
        ("alice", "secret")
    );
    assert_eq!(s.auth.new_auth_preference, "123456");
    assert!(s.auth.auth_dont_trust && s.sso.network.is_none());

    // The first login's worker is in flight: a second request waits.
    assert!(s.reconnect_started);
    s.machine.state = LOGIN;
    s.reconnect_started = false;
    assert!(run(&mut app, &mut c, "snlogin x")[0].starts_with("There was an error"));
    assert!(run(&mut app, &mut c, "snlogin 2 token").is_empty());
    let s = session(&mut app);
    assert_eq!(s.machine.state, LOGIN_GAME);
    assert_eq!(s.sso.network, Some(2));
    assert_eq!(s.auth.new_auth_preference, "token");
    Ok(())
}

/// The gate: with the commands closed to the account, the state-changing
/// commands and the server cheats do nothing, and a line the client cannot
/// send says it is unknown; staff level 2 opens them again.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn closed_gate_refuses_local_commands_and_reports_unknown_ones() -> anyhow::Result<()> {
    let (mut app, _keep) = app_in_game()?;
    let mut c = open_console(&mut app);
    app.console.commands_anywhere = false;
    session(&mut app).ui.engine.account.staff_mod_level = 0;
    let toolkit = preferences(&mut app).options.get("displayMode");
    let writes = session(&mut app).io.world.pending_writes.len();
    // In game: silent, nothing sent, nothing changed. Read-only commands
    // still answer.
    assert!(run(&mut app, &mut c, "tk1").is_empty());
    assert!(run(&mut app, &mut c, "tele 1 1 0").is_empty());
    assert_eq!(session(&mut app).io.world.pending_writes.len(), writes);
    assert_eq!(preferences(&mut app).options.get("displayMode"), toolkit);
    assert!(run(&mut app, &mut c, "heap")[0].starts_with("Heap: "));
    // Out of game: the line is unknown.
    session(&mut app).machine.state = LOGIN;
    assert_eq!(
        run(&mut app, &mut c, "tk1"),
        ["Unknown developer command: tk1"]
    );
    // Staff level 2 passes the gate.
    session(&mut app).machine.state = GAME;
    session(&mut app).ui.engine.account.staff_mod_level = 2;
    assert_eq!(run(&mut app, &mut c, "tk1"), ["Success"]);
    Ok(())
}

#[test]
fn the_gate_opens_for_other_world_modes_staff_and_the_build_switch() {
    assert!(!commands_allowed(Some(0), 0, false));
    assert!(!commands_allowed(Some(0), 1, false));
    assert!(commands_allowed(Some(0), 2, false));
    assert!(commands_allowed(Some(3), 0, false));
    assert!(commands_allowed(Some(0), 0, true));
}
