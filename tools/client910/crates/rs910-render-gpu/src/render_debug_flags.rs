//! The diagnostic `CLIENT910_*` environment variables the GPU toolkit reads
//! (the app reads `profile` as well), parsed once; readers name
//! `crate::render_debug_flags::flags().field`.

use std::ffi::OsString;
use std::sync::OnceLock;

/// Parsed GPU-renderer diagnostics. See the module docs.
#[derive(Debug, Default)]
pub struct DebugFlags {
    // --- presence flags: `std::env::var_os(NAME).is_some()` ---
    /// `CLIENT910_PROFILE` (`frame_profile::enabled`).
    pub profile: bool,
    pub profile_shared_discard: bool,
    pub profile_fragment_uniform: bool,
    pub profile_pre_alpha: bool,
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
            profile: has("CLIENT910_PROFILE"),
            profile_shared_discard: has("CLIENT910_PROFILE_SHARED_DISCARD"),
            profile_fragment_uniform: has("CLIENT910_PROFILE_FRAGMENT_UNIFORM"),
            profile_pre_alpha: has("CLIENT910_PROFILE_PRE_ALPHA"),
            profile_present_immediate: var("CLIENT910_PROFILE_PRESENT").as_deref()
                == Some("immediate"),
        }
    }
}
