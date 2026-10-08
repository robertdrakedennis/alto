//! The shell's half of the session helpers: the recorder header (it reads
//! the clap `Cli`) and the minimap's hint arrows. The session core itself
//! is `rs910_client::client_core` (Phase 5).
use super::*;

/// `CLIENT910_RECORD`: the world login reply, CLI identity, persisted
/// client files and input injectors a session replay starts from.
pub(super) fn record_session_header(
    cli: &Cli,
    live: &crate::session::LiveState,
    uid192: &[u8; 24],
) {
    if !crate::session_record::active() {
        return;
    }
    let mut head = String::new();
    let mut line = |key: &str, value: String| {
        head.push_str(key);
        head.push('=');
        head.push_str(&value);
        head.push('\n');
    };
    line("username", cli.username.clone());
    line("entity_state", cli.entity_state.to_string());
    line("pid", live.login.pid.map_or(-1, i32::from).to_string());
    line("server_token", live.login.server_token.to_string());
    line(
        "logged_in_members",
        live.login.profile.logged_in_members.to_string(),
    );
    line(
        "player_is_members",
        live.login.profile.player_is_members.to_string(),
    );
    line(
        "player_is_quickchat",
        live.login.profile.player_is_quickchat.to_string(),
    );
    line(
        "logged_in_quickchat",
        live.login.profile.logged_in_quickchat.to_string(),
    );
    line("dob_verified", live.login.profile.dob_verified.to_string());
    line("lobby_dob", live.login.profile.lobby_dob.to_string());
    line(
        "staff_mod_level",
        live.login.profile.staff_mod_level.to_string(),
    );
    line(
        "player_mod_level",
        live.login.profile.player_mod_level.to_string(),
    );
    line(
        "owner",
        live.login.profile.owner.clone().unwrap_or_default(),
    );
    line(
        "uid192",
        uid192.iter().map(|b| format!("{b:02x}")).collect(),
    );
    for command in &cli.server_commands {
        line("server_command", command.clone());
    }
    crate::session_record::record(b"HEAD", head.as_bytes());
    crate::session_record::record(b"SVRC", &live.login.server_varcs);
    let varc = crate::client_debug_flags::flags()
        .varc_file
        .clone()
        .unwrap_or_else(|| {
            cli.pack_root
                .parent()
                .unwrap_or(&cli.pack_root)
                .join("players/client-vars.dat")
        });
    for (tag, path) in [(b"PREF", preferences_path(&cli.pack_root)), (b"VARC", varc)] {
        crate::session_record::record(tag, &std::fs::read(path).unwrap_or_default());
    }
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(name, _)| name.starts_with("CLIENT910_") && name != "CLIENT910_RECORD")
        .collect();
    vars.sort();
    for (name, value) in vars {
        crate::session_record::record(b"ENV ", format!("{name}={value}").as_bytes());
    }
}

/// `HINT_ARROW` packets into the minimap's hint
/// slots at the current scene base, held until a map exists. Shared by
/// `ViewerApp` and the headless session replay.
pub(super) fn apply_hint_arrows(
    minimap: &mut crate::minimap::Minimap,
    pending: &mut Vec<Vec<u8>>,
    base: Option<[i32; 2]>,
    mut updates: Vec<Vec<u8>>,
) {
    let Some(base) = base else {
        pending.append(&mut updates);
        return;
    };
    updates.splice(0..0, std::mem::take(pending));
    // A rebuild at a new base re-anchors the arrows.
    minimap.rebase_hint_arrows(base);
    for bytes in updates {
        if crate::render_debug_flags::flags().hint_trace {
            log::info!("[client910] HINT_ARROW {bytes:02x?} base {base:?}");
        }
        if let Err(error) = minimap.apply_hint_arrow(&bytes, base) {
            log::warn!("[client910] HINT_ARROW update ignored: {error:#}");
        }
    }
}
