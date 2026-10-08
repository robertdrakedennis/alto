//! The diagnostic `CLIENT910_*` environment variable only the game layer
//! reads, parsed once (client910's `debug_flags` keeps the rest; `main`
//! forces every set at startup). Split out of client910's `DebugFlags` in
//! Phase 3.2 with the same parse expression; readers name
//! `crate::game_debug_flags::flags().cutscene_trace`.

use std::ffi::OsString;
use std::sync::OnceLock;

/// Parsed game-layer diagnostics. See the module docs.
#[derive(Debug, Default)]
pub struct DebugFlags {
    /// `CLIENT910_CUTSCENE_TRACE` (presence): log each cutscene action as
    /// it runs (`Game::update_scene_state`) and the cutscene camera and
    /// cancel-binding state (client910's app and `ui_host_game`).
    pub cutscene_trace: bool,
}

static FLAGS: OnceLock<DebugFlags> = OnceLock::new();

/// The process-wide flags, parsed from the environment on first use.
pub fn flags() -> &'static DebugFlags {
    FLAGS.get_or_init(DebugFlags::from_env)
}

impl DebugFlags {
    pub fn from_env() -> Self {
        Self::from_lookup(&|name| std::env::var_os(name))
    }

    /// Parse from any variable source (`std::env::var_os` in production).
    pub fn from_lookup(var_os: &dyn Fn(&str) -> Option<OsString>) -> Self {
        let has = |name: &str| var_os(name).is_some();
        Self {
            cutscene_trace: has("CLIENT910_CUTSCENE_TRACE"),
        }
    }
}
