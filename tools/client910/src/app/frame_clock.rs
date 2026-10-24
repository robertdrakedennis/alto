//! `ViewerApp::clock` (code-quality programme Phase 4.4).
use super::*;

const PRESENTATIONS_PER_SECOND: u64 = 120;
const NANOS_PER_SECOND: u64 = 1_000_000_000;
pub(super) const PRESENTATION_INTERVAL: Duration =
    Duration::from_nanos(NANOS_PER_SECOND / PRESENTATIONS_PER_SECOND);

/// The logic clock (20 ms logic cycles; the logic cycle counter is the
/// core's, `ClientCore::cycle`) and the redraw counters (fps,
/// frames drawn, window-title refresh). The full redraws' scene count
/// (`--screenshot-frame`) is the core's
/// `ClientCore::scene_cycle`.
pub(super) struct FrameClock {
    pub(super) last_frame: Instant,
    pub(super) logic: rs910_core::logic_clock::Clock,
    pub(super) last_title: Instant,
    pub(super) last_render: Instant,
    pub(super) next_render: Instant,
    pub(super) occluded: bool,
    /// Frames drawn (full redraws and presents).
    pub(super) frames: u32,
    pub(super) fps: f32,
}

impl FrameClock {
    /// Keep the presentation phase when a timer wakes late. Missed slots are
    /// skipped, so neither timer jitter nor a stalled frame creates a backlog.
    pub(super) fn presentation_started(&mut self, now: Instant) -> f32 {
        let elapsed = now.duration_since(self.last_render).as_secs_f32();
        self.last_render = now;
        if now >= self.next_render {
            let late = now.duration_since(self.next_render).as_nanos();
            let phase = Duration::from_nanos((late % PRESENTATION_INTERVAL.as_nanos()) as u64);
            self.next_render = now + (PRESENTATION_INTERVAL - phase);
        }
        elapsed
    }
}

impl ViewerApp {
    pub(super) fn independent_presentation(&self) -> bool {
        !self.clock.occluded
            && self.renderer.as_ref().is_some_and(|renderer| {
                renderer.kind() == crate::active_toolkit::RendererKind::Modern
            })
            && self
                .window
                .as_ref()
                .is_some_and(|window| !window.is_minimized().unwrap_or(false))
    }

    pub(super) fn render_due(&self) -> bool {
        !self.independent_presentation()
            || self.clock.logic.wake_deadline().is_none()
            || Instant::now() >= self.clock.next_render
    }

    pub(super) fn wake_deadline(&self) -> Option<Instant> {
        self.clock.logic.wake_deadline().map(|logic| {
            if self.independent_presentation()
                && (self.core.redraw_due() || self.frame.has_retained_scene())
            {
                logic.min(self.clock.next_render)
            } else {
                logic
            }
        })
    }

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
            if let Some(renderer) = self
                .renderer
                .as_ref()
                .filter(|renderer| renderer.kind() == crate::active_toolkit::RendererKind::Modern)
            {
                let size = window.inner_size();
                log::info!(
                    "[render] modern {:.1} fps | window {}x{} | cycle {}",
                    renderer.presentation_fps(),
                    size.width,
                    size.height,
                    self.core.cycle
                );
            }
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
                self.renderer
                    .as_ref()
                    .filter(|renderer| {
                        renderer.kind() == crate::active_toolkit::RendererKind::Modern
                    })
                    .map_or(self.clock.fps, |renderer| renderer.presentation_fps()),
                self.lifecycle.username,
                loading.unwrap_or_default(),
            ));
        }
    }
}
