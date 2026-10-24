//! The diagnostic `CLIENT910_*` environment variables both sides of the
//! toolkit boundary read (the retained UI that records a frame and the GPU
//! renderer that executes it), parsed once. Split out of client910's
//! `DebugFlags` in Phase 3.2 with the same parse expressions (`main` forces
//! every set at startup); readers name
//! `crate::toolkit_debug_flags::flags().field`.

use std::ffi::OsString;
use std::sync::OnceLock;

/// Parsed toolkit-boundary diagnostics. See the module docs.
#[derive(Debug, Default)]
pub struct DebugFlags {
    // --- presence flags: `std::env::var_os(NAME).is_some()` ---
    /// `CLIENT910_SETTINGS_TRACE`: preference loads and changes (UI) and
    /// the renderer settings they apply.
    pub settings_trace: bool,
    /// `CLIENT910_POSTFX_TRACE`: the post-effect capture region (UI) and
    /// the post-process parameters (renderer).
    pub postfx_trace: bool,

    // --- values ---
    /// `CLIENT910_MINIMAP_DUMP` (presence enables the dump; the value is
    /// the output path).
    pub minimap_dump: Option<OsString>,
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
            settings_trace: has("CLIENT910_SETTINGS_TRACE"),
            postfx_trace: has("CLIENT910_POSTFX_TRACE"),
            minimap_dump: var_os("CLIENT910_MINIMAP_DUMP"),
        }
    }
}
