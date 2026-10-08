//! Native wheel events to the client's integer wheel-rotation snapshot.
//! Keep fractional trackpad motion until it reaches a wheel step. The client's
//! positive direction is down; winit's is up. Pixel input uses logical pixels.
#[derive(Default)]
pub struct Wheel {
    remainder: f64,
}
impl Wheel {
    pub fn rotation(&mut self, delta: winit::event::MouseScrollDelta, scale: f64) -> i32 {
        let lines = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => f64::from(y),
            winit::event::MouseScrollDelta::PixelDelta(p) => p.y / scale.max(1.) / 100.,
        };
        self.remainder -= lines;
        let steps = self.remainder as i32;
        self.remainder -= f64::from(steps);
        steps
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use winit::{dpi::PhysicalPosition, event::MouseScrollDelta::*};
    #[test]
    fn fractional_motion_and_retina_pixels_preserve_wheel_steps() {
        let mut w = Wheel::default();
        for _ in 0..3 {
            assert_eq!(w.rotation(LineDelta(0., 0.25), 1.), 0);
        }
        assert_eq!(w.rotation(LineDelta(0., 0.25), 1.), -1);
        assert_eq!(w.rotation(LineDelta(0., -2.), 1.), 2);
        for scale in [1., 2.] {
            let mut w = Wheel::default();
            for _ in 0..3 {
                assert_eq!(
                    w.rotation(PixelDelta(PhysicalPosition::new(0., 25. * scale)), scale),
                    0
                );
            }
            assert_eq!(
                w.rotation(PixelDelta(PhysicalPosition::new(0., 25. * scale)), scale),
                -1
            );
        }
    }
}
