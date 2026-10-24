//! Client-owned free camera creation and orbit input.

use rs910_core::fault::Fault;

pub use rs910_core::vector_math::*;

use super::{Cam2, Focus, Position, Scene, CLIENT, MODE_ORIENTATION, MODE_POINT};

/// The staff free-fly camera: a client-owned camera with a point eye and an
/// orientation focus, steered by the middle mouse button and the arrow keys.
#[derive(Clone, Debug, PartialEq)]
pub struct FreeCamera {
    pub camera: Cam2,
    /// Mouse position at the last orbit step.
    pub last_orbit: [i32; 2],
}

impl FreeCamera {
    /// A free camera at a coordinate.
    pub fn create(level: i32, coord: [i32; 3], mouse: [i32; 2], scene: &Scene) -> Self {
        let mut camera = Cam2::new(false);
        camera.control_mode = CLIENT;
        camera.position_mode = Some(MODE_POINT);
        camera.position = Some(Position::for_mode(MODE_POINT));
        camera.lookat_mode = Some(MODE_ORIENTATION);
        camera.lookat = Some(Focus::for_mode(MODE_ORIENTATION));
        if let Some(Position::Point(p)) = camera.position.as_mut() {
            p.set(level, coord);
        }
        if let Some(Focus::Orientation(o)) = camera.lookat.as_mut() {
            // The zero yaw/pitch/roll rotation.
            let mut q = Quat::IDENTITY;
            q.set_to_rotation_ypr(0.0, 0.0, 0.0);
            o.set_orientation(q);
        }
        camera.position_max_speed = [99999.0; 3];
        camera.position_acceleration = [f32::INFINITY; 3];
        camera.lookat_max_speed = [99999.0; 3];
        camera.lookat_acceleration = [f32::INFINITY; 3];
        camera.scene = scene.clone();
        Self {
            camera,
            last_orbit: mouse,
        }
    }
    /// Orbit input, once per logic update instead of the scripted camera step.
    /// `viewport` is the scene viewport component size, `held` reports whether a
    /// key is down.
    pub fn handle_orbit_input(
        &mut self,
        viewport: Option<[i32; 2]>,
        mouse: [i32; 2],
        middle_held: bool,
        held: &dyn Fn(i32) -> bool,
        scene: &Scene,
    ) -> Result<(), String> {
        let c = &mut self.camera;
        let mut eye = match c.position.as_ref() {
            Some(Position::Point(p)) => p.current,
            _ => return Err(Fault::WrongValueType.message("free camera position owner")),
        };
        let mut orientation = match c.lookat.as_ref() {
            Some(Focus::Orientation(o)) => o.current,
            _ => return Err(Fault::WrongValueType.message("free camera focus owner")),
        };
        if let Some([w, h]) = viewport {
            let focal_length = 1000.0_f32;
            let fov_x = (f64::from(w as f32 / 2.0 / focal_length).atan() * 2.0) as f32;
            let fov_y = (f64::from(h as f32 / 2.0 / focal_length).atan() * 2.0) as f32;
            c.fov = [fov_x, fov_y];
        }
        if middle_held {
            let pitch_delta = Quat::rotation(
                1.0,
                0.0,
                0.0,
                (mouse[1] - self.last_orbit[1]) as f32 / 200.0,
            );
            orientation.multiply(&pitch_delta);
            let mut up = Vec3::new(0.0, 1.0, 0.0);
            up.rotate(&orientation);
            let yaw_delta =
                Quat::rotation_axis(&up, (self.last_orbit[0] - mouse[0]) as f32 / 200.0);
            orientation.multiply(&yaw_delta);
            if let Some(Focus::Orientation(o)) = c.lookat.as_mut() {
                o.set_orientation(orientation);
            }
        }
        self.last_orbit = mouse;
        orientation.opposite();
        for (key, step) in [
            (98, [0.0, 0.0, 25.0]),
            (99, [0.0, 0.0, -25.0]),
            (96, [-25.0, 0.0, 0.0]),
            (97, [25.0, 0.0, 0.0]),
        ] {
            if held(key) {
                let mut v = Vec3::new(step[0], step[1], step[2]);
                v.rotate(&orientation);
                v.y *= -1.0;
                eye.add(&v);
            }
        }
        if let Some(Position::Point(p)) = c.position.as_mut() {
            p.set(0, [eye.x as i32, eye.y as i32, eye.z as i32]);
        }
        c.scene = scene.clone();
        c.update(0.02)
    }
}
