//! `ViewerApp::diag` (code-quality programme Phase 4.4).
use super::*;

/// Headless verification state (client-only): the `--screenshot` target
/// and frame and the `CLIENT910_SCREENSHOT_SERIES` position (the
/// `CLIENT910_CUTSCENE` fixture's return tile is the core's).
pub(super) struct Diagnostics {
    /// `--screenshot` target + frame (headless verification).
    pub(super) screenshot: Option<(PathBuf, u32)>,
    /// The next `CLIENT910_SCREENSHOT_SERIES` cycle to capture.
    pub(super) screenshot_series_next: usize,
}

pub(super) fn screenshot_series() -> Option<&'static [i32]> {
    crate::debug_flags::flags().screenshot_series.as_deref()
}

impl Diagnostics {
    /// The next `CLIENT910_SCREENSHOT_SERIES` capture when logic cycle
    /// `cycle` has reached it (and advances the series): the shot a frame
    /// the series cannot reach, like the profiling message box drawn inside
    /// a logic cycle, asks the renderer for.
    pub(super) fn next_series_shot(&mut self, cycle: i32) -> Option<PathBuf> {
        let cycles = screenshot_series()?;
        let (path, _) = self.screenshot.as_ref()?;
        let at = *cycles
            .get(self.screenshot_series_next)
            .filter(|&&c| cycle >= c)?;
        self.screenshot_series_next += 1;
        let stem = path
            .file_stem()
            .map_or("frame".into(), |s| s.to_string_lossy().into_owned());
        let shot = path.with_file_name(format!("{stem}_{at:05}.png"));
        log::info!(
            "[client910] screenshot series {} at logic cycle {cycle} (inside the cycle)",
            shot.display()
        );
        Some(shot)
    }
}
