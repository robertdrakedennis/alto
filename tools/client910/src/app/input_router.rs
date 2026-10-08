//! `ViewerApp::input` (code-quality programme Phase 4.4).
use super::*;

/// Window input the shell routes to the interface runtime, the developer
/// console and the free camera: the `Cursor` owner, the mouse wheel
/// accumulator, held modifiers and keys, focus, the free-camera drag and
/// the pending native resize.
pub(super) struct InputRouter {
    pub(super) cursor_resources: Option<crate::ui_cursor::Resources>,
    pub(super) cursor_state: crate::ui_cursor::State,
    pub(super) mouse_wheel: crate::ui_mouse::Wheel,
    pub(super) modifiers: ModifiersState,
    pub(super) focused: bool,
    pub(super) pressed: HashSet<KeyCode>,
    pub(super) dragging: bool,
    pub(super) last_cursor: Option<(f64, f64)>,
    pub(super) pending_resize: crate::ui_window::PendingResize,
}

impl ViewerApp {
    pub(super) fn sync_cursor(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
        let (Some(window), Some(session)) = (&self.window, &self.core.session) else {
            return Ok(());
        };
        let (Some(game), ui) = (&session.game, &session.ui) else {
            return Ok(());
        };
        let enabled = game
            .ui_variables
            .queries
            .preferences
            .options
            .live()
            .custom_cursors;
        let mouse = ui.engine.platform.pending_mouse;
        let canvas = ui.state.layout.canvas;
        // The custom cursor applies to the canvas, not the outer frame's
        // margins. Winit has one inner view, so choose the system cursor there.
        let requested =
            if mouse[0] >= 0 && mouse[1] >= 0 && mouse[0] < canvas[0] && mouse[1] < canvas[1] {
                ui.cursor_id()
            } else {
                -1
            };
        if self.input.cursor_state.current == if enabled { requested } else { -1 } {
            return Ok(());
        }
        if self.input.cursor_resources.is_none() {
            self.scene
                .pack_root
                .as_ref()
                .context("cursor pack missing")?;
            self.input.cursor_resources =
                Some(crate::ui_cursor::Resources::new(self.pack.clone())?);
        }
        self.input.cursor_state.update(
            requested,
            enabled,
            self.input.cursor_resources.as_mut().unwrap(),
            &mut crate::ui_cursor_winit::Native { event_loop, window },
        )
    }
}
