//! The fullscreen and window-mode commands.
//! Saved values belong exclusively to ClientOptions; this owner holds the native
//! window and current canvas, never a second preference copy.
use crate::client_options::ClientOptions;
use native910::vm::{Value, VmError, VmResult};
use std::sync::Arc;

/// Samples the latest native dimensions during an update,
/// rather than queuing a relayout for every intermediate native resize event.
#[derive(Default)]
pub struct PendingResize(Option<[u32; 2]>);
impl PendingResize {
    pub fn receive(&mut self, size: [u32; 2]) {
        if size.iter().all(|&v| v > 0) {
            self.0 = Some(size);
        }
    }
    pub fn take(&mut self) -> Option<[u32; 2]> {
        self.0.take()
    }
}
pub use crate::game_canvas::*;

/// The usable fullscreen modes, sorted and de-duplicated, including the
/// equal-area ordering.
pub fn filter_modes(
    raw: &[crate::ui_runtime::FullscreenMode],
    screen_size: i32,
) -> Vec<crate::ui_runtime::FullscreenMode> {
    let mut kept: Vec<crate::ui_runtime::FullscreenMode> = Vec::new();
    for &m in raw {
        if !(m.bit_depth <= 0 || m.bit_depth >= 24)
            || m.width < 800
            || m.height < 600
            || (screen_size == 2 && (m.width > 800 || m.height > 600))
            || (screen_size == 1 && (m.width > 1024 || m.height > 768))
        {
            continue;
        }
        if let Some(old) = kept
            .iter_mut()
            .find(|k| k.width == m.width && k.height == m.height)
        {
            if m.bit_depth > old.bit_depth {
                *old = m;
            }
        } else {
            kept.push(m);
        }
    }
    fn sort(a: &mut [crate::ui_runtime::FullscreenMode], lo: i32, hi: i32) {
        if lo >= hi {
            return;
        }
        let mid = ((lo + hi) / 2) as usize;
        let pivot = a[mid].height.wrapping_mul(a[mid].width);
        a.swap(mid, hi as usize);
        let parity = if pivot == i32::MAX { 0 } else { 1 };
        let mut next = lo;
        for j in lo..hi {
            let m = a[j as usize];
            if m.height.wrapping_mul(m.width) < pivot.wrapping_add(j & parity) {
                a.swap(j as usize, next as usize);
                next += 1;
            }
        }
        a.swap(next as usize, hi as usize);
        sort(a, lo, next - 1);
        sort(a, next + 1, hi);
    }
    let hi = kept.len() as i32 - 1;
    sort(&mut kept, 0, hi);
    kept
}

/// Chooses the deepest colour, then the first refresh rate closest to the
/// desktop's current rate.
pub fn select_fullscreen_mode(
    modes: &[crate::ui_runtime::FullscreenMode],
    size: [i32; 2],
    refresh: i32,
) -> Option<usize> {
    let depth = modes
        .iter()
        .filter(|m| [m.width, m.height] == size)
        .map(|m| m.bit_depth)
        .max()?;
    let mut best = None;
    for (i, m) in modes
        .iter()
        .enumerate()
        .filter(|(_, m)| [m.width, m.height] == size && m.bit_depth == depth)
    {
        if best.is_none_or(|j: usize| {
            m.refresh.wrapping_sub(refresh).wrapping_abs()
                < modes[j].refresh.wrapping_sub(refresh).wrapping_abs()
        }) {
            best = Some(i);
        }
    }
    best
}

/// The native frame the window commands drive (the frame and its
/// display-mode calls). The shell implements it for its window
/// (client910 `ui_window_winit`); each method is one native call sequence
/// the commands make, so their order and results are the native window's.
pub trait NativeWindow: Send + Sync {
    /// The video modes of the frame's monitor (the primary monitor, else the
    /// current one, else the first available); none without a monitor.
    fn video_modes(&self) -> Vec<crate::ui_runtime::FullscreenMode>;
    /// `setFullscreen(null)`: leave exclusive fullscreen.
    fn set_windowed(&self);
    /// Sets whether the frame can be resized.
    fn set_resizable(&self, resizable: bool);
    /// Enter exclusive fullscreen in the video mode of that monitor that
    /// [`select_fullscreen_mode`] picks for `size` at the monitor's current
    /// refresh rate: `None` when it picks none (nothing changed), else
    /// whether the frame is now exclusive fullscreen.
    fn enter_fullscreen(&self, size: [i32; 2]) -> Option<bool>;
}

pub struct WindowState {
    pub native: Option<Arc<dyn NativeWindow>>,
    frame: Option<[i32; 2]>,
    pub mode: i32,
    pub changed: bool,
    pub last_fullscreen: [i32; 2],
    modes: Option<Vec<crate::ui_runtime::FullscreenMode>>,
    /// A canvas position set outside the layout (`EXECUTE_CLIENT_CHEAT` 26,
    /// a developer console command), kept until the next layout places the
    /// canvas at the left and top margins. The port has no frame insets.
    pub location: Option<[i32; 2]>,
}
impl Default for WindowState {
    fn default() -> Self {
        Self {
            native: None,
            frame: None,
            mode: 2,
            changed: false,
            last_fullscreen: [0; 2],
            modes: None,
            location: None,
        }
    }
}
impl WindowState {
    /// Install the window's size (`ViewerApp::install_session_toolkit` then
    /// stores the native window in [`Self::native`]; the headless session
    /// replay has none).
    pub fn install_size(&mut self, physical: [u32; 2], scale: f64) {
        self.observe_size(physical, scale);
        self.changed = true;
    }
    /// Commit the dimensions sampled for this update. Reading the native
    /// window again mid-update can mix a new canvas with old layout/GPU data.
    pub fn observe_size(&mut self, physical: [u32; 2], scale: f64) {
        let size = dpi::PhysicalSize::new(physical[0], physical[1]).to_logical::<i32>(scale);
        let frame = Some([size.width, size.height]);
        if self.frame != frame {
            // A resize lays the canvas out again.
            self.location = None;
        }
        self.frame = frame;
    }
    pub fn canvas(&self, screen_size: i32) -> Option<Canvas> {
        let mut canvas = Canvas::calculate(self.frame?, self.mode, screen_size);
        if let Some(location) = self.location {
            canvas.offset = location;
        }
        Some(canvas)
    }
    fn modes(&mut self, screen_size: i32) -> &[crate::ui_runtime::FullscreenMode] {
        if self.modes.is_none() {
            let raw = self
                .native
                .as_ref()
                .map(|w| w.video_modes())
                .unwrap_or_default();
            self.modes = Some(filter_modes(&raw, screen_size));
        }
        self.modes.as_deref().unwrap()
    }
    /// Forgets the cached modes: the next query re-reads the monitor's modes.
    pub fn forget_modes(&mut self) {
        self.modes = None;
    }
    /// A fullscreen frame that loses focus
    /// returns to the saved window mode (only in states 4/13/15/18).
    pub fn focus_lost(&mut self, options: &ClientOptions, state: i32) -> VmResult<bool> {
        if self.mode != 3 || !matches!(state, 4 | 13 | 15 | 18) {
            return Ok(false);
        }
        let fallback = options.get("windowMode").unwrap();
        if !(1..=2).contains(&fallback) {
            return Ok(false);
        }
        self.windowed(fallback)?;
        Ok(true)
    }
    /// The `URL_OPEN` packet and `openurl_nologin`: with a fullscreen frame
    /// up, the saved window mode is restored before the browser opens. The
    /// caller checks that fullscreen is allowed.
    pub fn leave_fullscreen(&mut self, options: &ClientOptions) -> VmResult<bool> {
        if self.mode != 3 {
            return Ok(false);
        }
        let fallback = options.get("windowMode").unwrap();
        if !(1..=2).contains(&fallback) {
            return Ok(false);
        }
        self.windowed(fallback)?;
        Ok(true)
    }
    fn windowed(&mut self, mode: i32) -> VmResult<()> {
        let native = self.native.as_ref().ok_or_else(|| VmError::TrapFailed {
            command: "setwindowmode".into(),
            reason: "native window is not installed".into(),
        })?;
        native.set_windowed();
        // The frame remains resizable in fixed canvas mode too.
        native.set_resizable(true);
        self.mode = mode;
        self.changed = true;
        Ok(())
    }
    pub fn dispatch(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        options: &mut ClientOptions,
        dirty: &mut bool,
    ) -> Option<VmResult<Option<Value>>> {
        let pop = |s: &mut Vec<i32>| s.pop().ok_or(VmError::StackUnderflow { stack: "int" });
        if self.native.is_some()
            && matches!(
                command,
                "fullscreen_modecount" | "fullscreen_getmode" | "fullscreen_lastmode"
            )
        {
            return Some((|| {
                let last = self.last_fullscreen;
                let modes = self.modes(options.live().screen_size);
                match command {
                    "fullscreen_modecount" => Ok(Some(Value::Int(modes.len() as i32))),
                    "fullscreen_lastmode" => Ok(Some(Value::Int(
                        modes
                            .iter()
                            .position(|m| [m.width, m.height] == last)
                            .map_or(-1, |n| n as i32),
                    ))),
                    _ => {
                        let index = pop(ints)?;
                        // A monitor with no mode inside the size limit (the
                        // macOS list starts at the panel's native size) is
                        // answered like a platform without fullscreen; the
                        // settings panel's resolution list is built from the
                        // same query and would otherwise stop half-way.
                        if modes.is_empty() {
                            ints.extend([0, 0]);
                            return Ok(None);
                        }
                        let m = modes
                            .get(index as usize)
                            .ok_or_else(|| VmError::TrapFailed {
                                command: command.into(),
                                reason: format!("invalid fullscreen mode {index}"),
                            })?;
                        ints.extend([m.width, m.height]);
                        Ok(None)
                    }
                }
            })());
        }
        match command {
            "setdefaultwindowmode" => Some((|| {
                let mode = pop(ints)?;
                if (1..=2).contains(&mode) {
                    options.set_field("windowMode", mode);
                    options.set_field("maxScreenSize2", mode);
                    *dirty = true;
                }
                Ok(None)
            })()),
            "setwindowmode" => Some((|| {
                let mode = pop(ints)?;
                if (1..=2).contains(&mode) {
                    self.windowed(mode)?;
                }
                Ok(None)
            })()),
            // Queries without a native owner use the explicit replay Engine
            // profile. Live queries read this same owner immediately after writes.
            "getwindowmode" if self.native.is_some() => Some(Ok(Some(Value::Int(self.mode)))),
            "fullscreen_enter" => Some((|| {
                let h = pop(ints)?;
                let w = pop(ints)?;
                let native = self.native.as_ref().ok_or_else(|| VmError::TrapFailed {
                    command: command.into(),
                    reason: "native window is not installed".into(),
                })?;
                let Some(entered) = native.enter_fullscreen([w, h]) else {
                    let fallback = options.get("windowMode").unwrap();
                    if (1..=2).contains(&fallback) {
                        self.windowed(fallback)?;
                    }
                    return Ok(Some(Value::Int(0)));
                };
                if entered {
                    self.mode = 3;
                    self.last_fullscreen = [w, h];
                    self.changed = true;
                    *dirty = true;
                } else {
                    let fallback = options.get("windowMode").unwrap();
                    if (1..=2).contains(&fallback) {
                        self.windowed(fallback)?;
                    }
                }
                Ok(Some(Value::Int(i32::from(entered))))
            })()),
            "fullscreen_exit" => Some((|| {
                if self.mode == 3 {
                    let fallback = options.get("windowMode").unwrap();
                    if !(1..=2).contains(&fallback) {
                        return Err(VmError::TrapFailed {
                            command: command.into(),
                            reason: "saved window mode has no windowed fallback".into(),
                        });
                    }
                    self.windowed(fallback)?;
                }
                Ok(None)
            })()),
            _ => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canvas_limits_and_fixed_clipping() {
        assert_eq!(
            Canvas::calculate([1920, 1080], 2, 0),
            Canvas {
                size: [1920, 1080],
                offset: [0, 0]
            }
        );
        assert_eq!(
            Canvas::calculate([1920, 1080], 2, 1),
            Canvas {
                size: [1024, 768],
                offset: [448, 0]
            }
        );
        assert_eq!(
            Canvas::calculate([1920, 1080], 3, 2),
            Canvas {
                size: [800, 600],
                offset: [560, 0]
            }
        );
        assert_eq!(
            Canvas::calculate([640, 480], 2, 1),
            Canvas {
                size: [640, 480],
                offset: [0, 0]
            }
        );
        assert_eq!(
            Canvas::calculate([640, 480], 1, 0),
            Canvas {
                size: [765, 553],
                offset: [-62, 0]
            }
        );
        let c = Canvas::calculate([1280, 720], 2, 2);
        assert_eq!(c.physical_rect(2.), [480, 0, 1600, 1200]);
        assert_eq!(c.mouse([500., 40.], 2.), [10, 20]);
        assert_eq!(c.mouse([400., 40.], 2.), [-40, 20]);
    }
    #[test]
    fn default_mode_uses_canonical_fields_without_changing_active_mode() {
        let mut w = WindowState::default();
        let mut options = ClientOptions::new(crate::client_options::Profile {
            arm: false,
            ..Default::default()
        });
        let mut dirty = false;
        w.dispatch(
            "setdefaultwindowmode",
            &mut vec![1],
            &mut options,
            &mut dirty,
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.get("windowMode"), Some(1));
        assert_eq!(options.get("maxScreenSize2"), Some(1));
        assert_eq!(w.mode, 2);
        assert!(dirty);
        dirty = false;
        w.dispatch(
            "setdefaultwindowmode",
            &mut vec![3],
            &mut options,
            &mut dirty,
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.get("windowMode"), Some(1));
        assert!(!dirty);
    }
}

#[cfg(test)]
#[test]
fn canvas_dimensions_match_the_recording() -> anyhow::Result<()> {
    for line in rs910_core::test_support::frozen::text("graphics-settings/canvas.csv").lines() {
        let a: Vec<i32> = line.split(',').map(|n| n.parse().unwrap()).collect();
        for mode in [2, 3] {
            assert_eq!(
                Canvas::calculate([a[0], a[1]], mode, a[2]),
                Canvas {
                    size: [a[3], a[4]],
                    offset: [a[5], a[6]]
                },
                "{line}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn fullscreen_selects_depth_then_nearest_refresh() {
    use crate::ui_runtime::FullscreenMode as M;
    let modes = [
        M {
            width: 800,
            height: 600,
            bit_depth: 16,
            refresh: 120,
        },
        M {
            width: 800,
            height: 600,
            bit_depth: 24,
            refresh: 60,
        },
        M {
            width: 800,
            height: 600,
            bit_depth: 24,
            refresh: 144,
        },
        M {
            width: 800,
            height: 600,
            bit_depth: 24,
            refresh: 144,
        },
    ];
    assert_eq!(select_fullscreen_mode(&modes, [800, 600], 120), Some(2));
    assert_eq!(select_fullscreen_mode(&modes, [800, 600], 60), Some(1));
    assert_eq!(select_fullscreen_mode(&modes, [1024, 768], 60), None);
}
