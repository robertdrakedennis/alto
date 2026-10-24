//! The client's persisted files: `random.dat`
//! (uid192), `preferences` and the local client-variable file, and the
//! server-permanent varcs of the login block (moved from
//! `client910::app::startup`, Phase 5).
use super::*;

pub fn uid192_path(pack_root: &Path) -> PathBuf {
    crate::client_debug_flags::flags()
        .uid192_file
        .clone()
        .unwrap_or_else(|| {
            pack_root
                .parent()
                .unwrap_or(pack_root)
                .join("players/random.dat")
        })
}

pub fn load_uid192(path: &Path) -> [u8; 24] {
    let Ok(bytes) = std::fs::read(path) else {
        return [0; 24];
    };
    let Ok(value) = <[u8; 24]>::try_from(bytes.as_slice()) else {
        return [0; 24];
    };
    if value.iter().all(|&byte| byte == 0) {
        [0; 24]
    } else {
        value
    }
}

pub fn store_uid192(path: &Path, value: &[u8; 24]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, value)?;
    Ok(())
}

/// Client startup: install the preferences and local client-variable files
/// (prefs file "2") on the client-variable owner. Both are kept across logins.
pub fn install_client_persistence(
    game: &mut crate::client_game::ClientGame,
    pack_root: &Path,
) -> anyhow::Result<()> {
    game.ui_variables
        .queries
        .preferences
        .install(preferences_path(pack_root));
    install_client_varcs(game, pack_root)
}

/// The preferences file.
pub fn preferences_path(pack_root: &Path) -> PathBuf {
    crate::client_debug_flags::flags()
        .preferences_file
        .clone()
        .unwrap_or_else(|| {
            pack_root
                .parent()
                .unwrap_or(pack_root)
                .join("players/preferences.dat")
        })
}

/// The local client variables file ("2").
pub fn install_client_varcs(
    game: &mut crate::client_game::ClientGame,
    pack_root: &Path,
) -> anyhow::Result<()> {
    let defs = game
        .game
        .inputs
        .bits
        .definitions
        .get(&2)
        .context("client variable definitions")?;
    let path = crate::client_debug_flags::flags()
        .varc_file
        .clone()
        .unwrap_or_else(|| {
            pack_root
                .parent()
                .unwrap_or(pack_root)
                .join("players/client-vars.dat")
        });
    if let Err(error) =
        game.ui_variables
            .persistence
            .install(path, &mut game.ui_variables.client, defs)
    {
        log::warn!("[client910] load local client variables: {error:#}");
    }
    Ok(())
}

/// Server-permanent varcs from the world login block.
pub fn restore_server_varcs(
    game: &mut crate::client_game::ClientGame,
    server_varcs: &[u8],
) -> anyhow::Result<()> {
    let defs = game
        .game
        .inputs
        .bits
        .definitions
        .get(&2)
        .context("client variable definitions")?;
    // A game login starts the client variables over: the temporary and
    // server-permanent values of the previous session are dropped, then the
    // server's own values are restored.
    game.ui_variables
        .client
        .reset(&game.game.inputs.bits.definitions[&2]);
    crate::ui_var_store::restore_server(&mut game.ui_variables.client, defs, server_varcs)
}

/// A fresh entity/variable runtime for a new connection that keeps the
/// process-lifetime client owners: the client variable domain, preferences and
/// the stat/quest definition tables. Only the session stats are reset; varps
/// belong to the new entity runtime (`varps.reset()`).
pub fn adopt_client_variables(
    previous: Option<crate::client_game::ClientGame>,
    next: &mut crate::client_game::ClientGame,
    logged_in_members: bool,
) -> anyhow::Result<bool> {
    let Some(previous) = previous else {
        return Ok(false);
    };
    // The variable transmit counter keeps counting through a world change
    // (only a logout starts it over): the interface's components remember
    // the last transmit they saw, and a restarted counter would sit behind
    // them, so the new world's variables would not reach their hooks.
    next.runtime.varp_transmit_num = previous.game.runtime.varp_transmit_num;
    next.runtime.varp_transmitted = previous.game.runtime.varp_transmitted;
    next.ui_variables = previous.ui_variables;
    if let Some(stats) = next.ui_variables.stats.as_mut() {
        stats.logged_in_members = logged_in_members;
        stats.reset_session()?;
    }
    Ok(true)
}
