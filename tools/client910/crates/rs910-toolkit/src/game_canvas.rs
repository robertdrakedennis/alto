//! The game canvas inside the native frame: its size
//! and offset for the window mode, and the
//! physical rectangle and mouse mapping the presenter and the input owner
//! use. Split out of client910's `ui_window` in Phase 3.2: the GPU
//! renderer composes the canvas into the surface and must not name the UI.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Canvas {
    pub size: [i32; 2],
    pub offset: [i32; 2],
}
impl Canvas {
    /// The canvas size and offset for a window mode. Division truncates toward
    /// zero, including the negative margin when the fixed canvas exceeds frame.
    pub fn calculate(frame: [i32; 2], mode: i32, screen_size: i32) -> Self {
        let frame = frame.map(|v| v.max(1));
        let size = if mode == 1 {
            [765, 553]
        } else {
            let limit = match screen_size {
                2 => [800, 600],
                1 => [1024, 768],
                _ => frame,
            };
            [frame[0].min(limit[0]), frame[1].min(limit[1])]
        };
        Self {
            size,
            offset: [(frame[0] - size[0]) / 2, 0],
        }
    }
    pub fn physical_rect(self, scale: f64) -> [i32; 4] {
        let x = (f64::from(self.offset[0]) * scale).round() as i32;
        let y = (f64::from(self.offset[1]) * scale).round() as i32;
        let r = (f64::from(self.offset[0] + self.size[0]) * scale).round() as i32;
        let b = (f64::from(self.offset[1] + self.size[1]) * scale).round() as i32;
        [x, y, r - x, b - y]
    }
    pub fn mouse(self, physical: [f64; 2], scale: f64) -> [i32; 2] {
        [
            (physical[0] / scale).round() as i32 - self.offset[0],
            (physical[1] / scale).round() as i32 - self.offset[1],
        ]
    }
}
