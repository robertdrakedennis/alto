//! The diagnostic `CLIENT910_*` environment variable only the game layer
//! reads, parsed once (client910's `debug_flags` keeps the rest; `main`
//! forces every set at startup). Split out of client910's `DebugFlags` in
//! Phase 3.2 with the same parse expression; readers name
//! `crate::game_debug_flags::flags().cutscene_trace`.

use std::sync::OnceLock;

/// Parsed game-layer diagnostics. See the module docs.
#[derive(Debug, Default)]
pub struct DebugFlags {
    /// `CLIENT910_CUTSCENE_TRACE` (presence): log each cutscene action as
    /// it runs (`Game::update_scene_state`) and the cutscene camera and
    /// cancel-binding state (client910's app and `ui_host_game`).
    pub cutscene_trace: bool,
}

/// The process-wide flags, parsed from the environment on first use.
pub fn flags() -> &'static DebugFlags {
    static FLAGS: OnceLock<DebugFlags> = OnceLock::new();
    FLAGS.get_or_init(|| {
        let var_os = |name: &str| std::env::var_os(name);
        let has = |name: &str| var_os(name).is_some();
        DebugFlags {
            cutscene_trace: has("CLIENT910_CUTSCENE_TRACE"),
        }
    })
}
