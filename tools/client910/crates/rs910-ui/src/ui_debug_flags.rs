//! The diagnostic `CLIENT910_*` environment variables only the retained UI
//! reads (and the app above it: `chat_trace`), parsed once. Split out of
//! client910's `DebugFlags` in Phase 3.2 with the same parse expressions
//! (client910's `debug_flags` keeps the rest; `main` forces every set at
//! startup); readers name `crate::ui_debug_flags::flags().field`.

use std::ffi::OsString;
use std::sync::OnceLock;

/// Parsed UI diagnostics. See the module docs.
#[derive(Debug, Default)]
pub struct DebugFlags {
    // --- presence flags: `std::env::var_os(NAME).is_some()` ---
    pub chat_trace: bool,
    pub ui_trace_hide: bool,
    pub ui_trace_input: bool,
    /// Read only by the `#[cfg(test)]` visibility-write trace.
    #[cfg(any(test, feature = "test-hooks"))]
    pub ui_write_trace: bool,
    pub hook_trace: bool,
    pub ui_cam2_trace: bool,
    pub members_trace: bool,
    pub inv_trace: bool,
    pub camera_trace: bool,
    pub minimap_nomask: bool,

    // --- values ---
    /// `CLIENT910_CS2_CMD_TRACE=cmd,cmd` (lossy, split on `,`).
    pub cs2_cmd_trace: Option<Vec<String>>,
    /// `CLIENT910_PNG_HOST` (`env::var(..).ok()`).
    pub png_host: Option<String>,
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
            chat_trace: has("CLIENT910_CHAT_TRACE"),
            ui_trace_hide: has("CLIENT910_UI_TRACE_HIDE"),
            ui_trace_input: has("CLIENT910_UI_TRACE_INPUT"),
            #[cfg(any(test, feature = "test-hooks"))]
            ui_write_trace: has("CLIENT910_UI_WRITE_TRACE"),
            hook_trace: has("CLIENT910_HOOK_TRACE"),
            ui_cam2_trace: has("CLIENT910_UI_CAM2_TRACE"),
            members_trace: has("CLIENT910_MEMBERS_TRACE"),
            inv_trace: has("CLIENT910_INV_TRACE"),
            camera_trace: has("CLIENT910_CAMERA_TRACE"),
            minimap_nomask: has("CLIENT910_MINIMAP_NOMASK"),
            cs2_cmd_trace: var_os("CLIENT910_CS2_CMD_TRACE").map(|list| {
                list.to_string_lossy()
                    .split(',')
                    .map(str::to_string)
                    .collect()
            }),
            png_host: var("CLIENT910_PNG_HOST"),
        }
    }

    /// `CLIENT910_CS2_CMD_TRACE` lists `command`.
    pub fn traces_cs2_command(&self, command: &str) -> bool {
        self.cs2_cmd_trace
            .as_ref()
            .is_some_and(|list| list.iter().any(|name| name == command))
    }
}
