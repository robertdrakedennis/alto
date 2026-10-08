//! The diagnostic `CLIENT910_*` environment variables the client layer
//! owns, parsed once: the session recorder's output file (`session_record`),
//! the session core's outgoing-packet trace and persisted-file paths
//! (Phase 5) and the deterministic input injectors ([`InputScript`], which the app
//! applies at their logic-cycle points). Split out of client910's
//! `DebugFlags` in Phase 4 (lane Q-CLIENT) with the same parse expressions
//! (`main` forces every set at startup); readers name
//! `crate::client_debug_flags::flags().field`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::input_script::InputScript;

/// Parsed client-layer diagnostics. See the module docs.
#[derive(Debug, Default)]
pub struct DebugFlags {
    /// `CLIENT910_RECORD=<file>`: record the world session (`session_record`).
    pub record: Option<PathBuf>,
    /// Interactive recordings accept and retain native keyboard and pointer input.
    pub record_interactive: bool,
    /// `CLIENT910_OUT_TRACE`: print each queued outgoing client packet.
    pub out_trace: bool,
    /// `CLIENT910_UID192_FILE`, `CLIENT910_PREFERENCES_FILE`,
    /// `CLIENT910_VARC_FILE`: the persisted client files.
    pub uid192_file: Option<PathBuf>,
    pub preferences_file: Option<PathBuf>,
    pub varc_file: Option<PathBuf>,
    /// `CLIENT910_POSAUDIO_TRACE`: period, only when it parses and is `> 0`.
    pub posaudio_trace_every: Option<i32>,

    /// Deterministic input injectors and fixtures.
    pub input: InputScript,
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
        let path = |name: &str| var_os(name).map(PathBuf::from);
        Self {
            record: path("CLIENT910_RECORD"),
            record_interactive: var_os("CLIENT910_RECORD_MODE")
                .is_some_and(|mode| mode == "interactive"),
            out_trace: has("CLIENT910_OUT_TRACE"),
            uid192_file: path("CLIENT910_UID192_FILE"),
            preferences_file: path("CLIENT910_PREFERENCES_FILE"),
            varc_file: path("CLIENT910_VARC_FILE"),
            posaudio_trace_every: var_os("CLIENT910_POSAUDIO_TRACE")
                .and_then(|v| v.into_string().ok())
                .and_then(|v| v.parse::<i32>().ok())
                .filter(|&n| n > 0),
            input: InputScript::from_lookup(var_os),
        }
    }
}
