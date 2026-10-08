//! The screen fade (start/end cycle and start/end colour) as the interface
//! draw reads it.
//! The cutscene logic writes them and the retained UI
//! draws the colour, so the type lives below both. Moved whole out of
//! client910's `ui_draw` (Phase 3.2), which re-exports it.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fade {
    pub start_cycle: i32,
    pub end_cycle: i32,
    pub start: [i32; 4],
    pub end: [i32; 4],
}
impl Fade {
    /// The fade colour at `cycle`; f32 arithmetic order and int casts are kept exactly, no clamping.
    pub fn colour(&self, cycle: i32) -> Option<i32> {
        let mut c = self.end;
        if cycle < self.end_cycle {
            let t = cycle.wrapping_sub(self.start_cycle) as f32 * 1.0
                / self.end_cycle.wrapping_sub(self.start_cycle) as f32;
            for (i, v) in c.iter_mut().enumerate() {
                *v = ((1.0 - t) * self.start[i] as f32 + self.end[i] as f32 * t) as i32;
            }
        }
        (c[0] > 0)
            .then_some(c[0].wrapping_shl(24) | c[1].wrapping_shl(16) | c[2].wrapping_shl(8) | c[3])
    }
}
