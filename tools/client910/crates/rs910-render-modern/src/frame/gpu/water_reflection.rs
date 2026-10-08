//! The planar water reflection's draw list (lane P4-GPU): the frame's model
//! draws whose boxes meet the reflected frustum (`models::bounds`), in the
//! frame's order.
//!
//! The reflection pass used to draw every draw of the frame (the modern
//! client draws its own gather of the reflected view; the port drew the main
//! view's list). A draw wholly outside the reflected clip
//! volume (outside the mirrored view's sides or far plane, or below the
//! water: the oblique near plane is the water plane) writes no texel of the
//! reflection, so the culled list gives the same reflection image. Draws
//! without a box (floors or unknown geometry) are always drawn. Far merged
//! containers carry boxes of only the selected models in each compatible run.

use crate::frame::*;

impl ModernRenderer {
    /// This frame's reflection list under the reflected camera's
    /// `clip` (camera-local to clip).
    pub(crate) fn cull_reflection(&mut self, clip: &glam::Mat4) {
        let reflected = &mut self.water.reflected;
        reflected.clear();
        #[cfg(test)]
        let cull = !self.water.test_no_reflection_cull;
        #[cfg(not(test))]
        let cull = true;
        let mut ranges = self.draw_bounds.iter().peekable();
        let mut culled = 0;
        let mut out = None;
        for i in 0..self.draws.len() as u32 {
            // The box of the entity whose range holds draw `i`, tested
            // once per entity.
            while ranges.next_if(|r| r.1 <= i).is_some() {
                out = None;
            }
            let hidden = match ranges.peek() {
                Some(&&(start, _, bounds)) if cull && start <= i => {
                    *out.get_or_insert_with(|| bounds.outside(clip))
                }
                _ => false,
            };
            if hidden {
                culled += 1;
            } else {
                reflected.push(i);
            }
        }
        self.water.stats.reflection_draws = reflected.len();
        self.water.stats.reflection_culled = culled;
    }
}
