//! The diagnostic `CLIENT910_*` environment variables only the scene layer
//! reads, parsed once (client910's `debug_flags` keeps the rest; `main`
//! forces both at startup). Split out of client910's `DebugFlags` in Phase
//! 2.8 with the same parse expressions, so call sites keep reading
//! `crate::debug_flags::flags().field`. Test and oracle-only variables stay
//! at their test sites.

use std::ffi::OsString;
use std::sync::OnceLock;

/// Parsed scene diagnostics. See the module docs.
#[derive(Debug, Default)]
pub struct DebugFlags {
    /// `CLIENT910_ICON_MODEL_OUT`: dump the icon model points
    /// (`ui_icon_model`).
    pub icon_model_out: Option<OsString>,
    /// `CLIENT910_ANIMATION_SEED` parsed as `u64`: the offline scene's
    /// dynamic-loc random seed (`live_scene`).
    pub animation_seed: Option<u64>,
    /// `CLIENT910_ANIMATION_CYCLE` parsed as `i32`: a fixed animation cycle
    /// (`LiveScene::animation_cycle`).
    pub animation_cycle: Option<i32>,
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
        // `std::env::var(name).ok()`: absent or non-UTF-8 is `None`.
        let var = |name: &str| var_os(name).and_then(|v| v.into_string().ok());
        Self {
            icon_model_out: var_os("CLIENT910_ICON_MODEL_OUT"),
            animation_seed: var("CLIENT910_ANIMATION_SEED").and_then(|s| s.parse().ok()),
            animation_cycle: var("CLIENT910_ANIMATION_CYCLE").and_then(|s| s.parse().ok()),
        }
    }
}
