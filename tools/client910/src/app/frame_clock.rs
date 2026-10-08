//! `ViewerApp::clock` (code-quality programme Phase 4.4).
use super::*;

/// The logic clock (20 ms logic cycles; the logic cycle counter is the
/// core's, `ClientCore::cycle`) and the redraw counters (fps,
/// frames drawn, window-title refresh). The full redraws' scene count
/// (`--screenshot-frame`) is the core's
/// `ClientCore::scene_cycle`.
pub(super) struct FrameClock {
    pub(super) last_frame: Instant,
    pub(super) logic: crate::logic_clock::Clock,
    pub(super) last_title: Instant,
    pub(super) last_render: Instant,
    /// Frames drawn (full redraws and presents).
    pub(super) frames: u32,
    pub(super) fps: f32,
}

impl ViewerApp {
    /// The window title is the game's own; `CLIENT910_TITLE_STATS` swaps in
    /// the developer's line (camera tile, frame rate, player).
    pub(super) fn refresh_title(&mut self) {
        if !crate::debug_flags::flags().title_stats {
            return;
        }
        if self.clock.last_title.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.clock.last_title = Instant::now();
        if let Some(window) = self.window.as_ref() {
            let loading = self
                .core
                .session
                .as_ref()
                .and_then(|session| session.prefetch_loading.as_ref())
                .map(|loading| format!(" | loading {}/{}", loading.loaded, loading.total));
            window.set_title(&format!(
                "client910 x={:.0} z={:.0} lvl{} | {:.0} fps | {}{}",
                self.view.camera.target.x,
                self.view.camera.target.z,
                self.scene.focus_level,
                self.clock.fps,
                self.lifecycle.username,
                loading.unwrap_or_default(),
            ));
        }
    }
}
