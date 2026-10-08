//! The shell's native frame for `ui_window`: [`NativeWindow`] on the winit
//! window, with the native call sequences `ui_window::WindowState` made
//! before Phase 3.2 (winit stays in the shell): the monitor lookup (primary,
//! else current, else the first available), its video modes as
//! `FullscreenMode`s, and `Fullscreen::Exclusive` entry checked by reading
//! the mode back. A platform that cannot enter exclusive fullscreen (Wayland,
//! or a monitor that lists no video modes) still gets fullscreen: the frame
//! goes borderless on that monitor at its current size.
use crate::ui_runtime::FullscreenMode;
use crate::ui_window::{select_fullscreen_mode, NativeWindow};
use std::sync::Arc;
use winit::monitor::{MonitorHandle, VideoModeHandle};
use winit::window::{Fullscreen, Window};

/// The app's window as the preferences' native frame
/// (`WindowState.native`).
pub struct Native(pub Arc<Window>);

/// The monitor the frame's fullscreen commands act on: the primary monitor,
/// else the current one, else the first available.
fn fullscreen_monitor(window: &Window) -> Option<MonitorHandle> {
    window
        .primary_monitor()
        .or_else(|| window.current_monitor())
        .or_else(|| window.available_monitors().next())
}

/// A monitor's mode as the script records carry it.
fn record(mode: &VideoModeHandle) -> FullscreenMode {
    FullscreenMode {
        width: mode.size().width as i32,
        height: mode.size().height as i32,
        bit_depth: i32::from(mode.bit_depth()),
        refresh: (mode.refresh_rate_millihertz() / 1000) as i32,
    }
}

impl NativeWindow for Native {
    fn video_modes(&self) -> Vec<FullscreenMode> {
        let Some(monitor) = fullscreen_monitor(&self.0) else {
            return Vec::new();
        };
        let modes: Vec<_> = monitor.video_modes().map(|m| record(&m)).collect();
        if !modes.is_empty() {
            return modes;
        }
        // No listed modes: offer the monitor's own size, which the borderless
        // fallback of `enter_fullscreen` can honour.
        let size = monitor.size();
        vec![FullscreenMode {
            width: size.width as i32,
            height: size.height as i32,
            bit_depth: 32,
            refresh: (monitor.refresh_rate_millihertz().unwrap_or(60_000) / 1000) as i32,
        }]
    }
    fn set_windowed(&self) {
        self.0.set_fullscreen(None);
    }
    fn set_resizable(&self, resizable: bool) {
        self.0.set_resizable(resizable);
    }
    fn enter_fullscreen(&self, size: [i32; 2]) -> Option<bool> {
        let [w, h] = size;
        let native = &self.0;
        let monitor = fullscreen_monitor(native)?;
        let current = (monitor.refresh_rate_millihertz().unwrap_or(0) / 1000) as i32;
        let modes: Vec<_> = monitor.video_modes().collect();
        let records: Vec<_> = modes.iter().map(record).collect();
        if let Some(best) = select_fullscreen_mode(&records, [w, h], current) {
            native.set_fullscreen(Some(Fullscreen::Exclusive(modes[best].clone())));
            if matches!(native.fullscreen(), Some(Fullscreen::Exclusive(_))) {
                return Some(true);
            }
        } else if !modes.is_empty() {
            // The monitor lists modes and none is this one.
            return None;
        }
        // Exclusive fullscreen is unavailable here.
        native.set_fullscreen(Some(Fullscreen::Borderless(Some(monitor))));
        Some(matches!(
            native.fullscreen(),
            Some(Fullscreen::Borderless(_))
        ))
    }
}
