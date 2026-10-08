//! `ViewerApp::lifecycle` (code-quality programme Phase 4.4).
use super::*;

/// The client lifecycle around the session: the loading owner (states
/// 5/11/1), whether the login UI was shown, the title/lobby world, the
/// canvas size, the window-title username and a fatal cheat's shutdown
/// request.
pub(super) struct Lifecycle {
    /// The loading sequence (states 5/11/1) with the app side of its stages;
    /// `None` outside the loading states.
    pub(super) loading: Option<loading_host::LoadingOwner>,
    pub(super) login_ui_shown: bool,
    /// The world as the title/lobby states rebuild it.
    pub(super) title_world: crate::title_world::TitleWorld,
    /// The canvas size, replaced from the graphics defaults by the
    /// `DOWNLOAD_STUFF` loading stage.
    pub(super) client_frame: [i32; 2],
    pub(super) username: String,
    /// Crash report and shutdown requested by a fatal client cheat; the
    /// frame loop exits.
    pub(super) shutdown_requested: bool,
}
