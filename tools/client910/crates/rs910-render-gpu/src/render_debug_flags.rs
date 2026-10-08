//! The diagnostic `CLIENT910_*` environment variables the GPU renderer
//! reads (the app reads `hint_trace` and `profile` as well), parsed once.
//! Split out of client910's `DebugFlags` in Phase 3.4 with the same parse
//! expressions (`main` forces every set at startup); readers name
//! `crate::render_debug_flags::flags().field`.

use std::ffi::OsString;
use std::sync::OnceLock;

/// Parsed GPU-renderer diagnostics. See the module docs.
#[derive(Debug, Default)]
pub struct DebugFlags {
    // --- presence flags: `std::env::var_os(NAME).is_some()` ---
    pub hint_trace: bool,
    pub billboard_trace: bool,
    /// `CLIENT910_PROFILE` (`FrameProfile::enabled`).
    pub profile: bool,
    pub profile_shared_discard: bool,
    pub profile_fragment_uniform: bool,
    pub profile_pre_alpha: bool,
    pub profile_direct_draws: bool,
    pub profile_redundant_pipelines: bool,
    /// `CLIENT910_PROFILE_PRESENT == "immediate"` (`env::var`).
    pub profile_present_immediate: bool,
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
        // `std::env::var(name).ok()`: absent or non-UTF-8 is `None`.
        let var = |name: &str| var_os(name).and_then(|v| v.into_string().ok());
        Self {
            hint_trace: has("CLIENT910_HINT_TRACE"),
            billboard_trace: has("CLIENT910_BILLBOARD_TRACE"),
            profile: has("CLIENT910_PROFILE"),
            profile_shared_discard: has("CLIENT910_PROFILE_SHARED_DISCARD"),
            profile_fragment_uniform: has("CLIENT910_PROFILE_FRAGMENT_UNIFORM"),
            profile_pre_alpha: has("CLIENT910_PROFILE_PRE_ALPHA"),
            profile_direct_draws: has("CLIENT910_PROFILE_DIRECT_DRAWS"),
            profile_redundant_pipelines: has("CLIENT910_PROFILE_REDUNDANT_PIPELINES"),
            profile_present_immediate: var("CLIENT910_PROFILE_PRESENT").as_deref()
                == Some("immediate"),
        }
    }
}
