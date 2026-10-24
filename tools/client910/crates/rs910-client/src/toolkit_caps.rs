//! `ToolkitCaps`: the device capabilities the session's toolkit installation
//! reads (`ViewerApp::install_session_toolkit`, `install_toolkit_preferences`)
//! and the session recorder writes as `TOOL` (`session_record::toolkit`). Split
//! out of client910's app.rs in Phase 4 (lane Q-CLIENT), whole, so
//! `session_record` no longer names the shell.

/// What the device's hardware toolkits answer, whichever toolkit is active
/// while the session installs: the installation applies toolkit 0's answers
/// (none) itself once it knows the saved toolkit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolkitCaps {
    pub antialiasing: bool,
    pub bloom: bool,
}
