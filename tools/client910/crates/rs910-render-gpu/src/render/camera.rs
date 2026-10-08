//! [`OrbitCamera`]: the orbit/`cam2`/legacy camera inputs the frame
//! converts to engine units (512 per tile).

/// The classic third-person orbit camera (view/projection in `camera.rs`).
/// State is kept in viewer terms (target in world units = tiles, y up) and
/// converted to engine units per frame; angles are the game's 14-bit units.
#[derive(Clone, Debug)]
pub struct OrbitCamera {
    /// Look-at point, world coords (tiles, y up).
    pub target: glam::Vec3,
    /// Orbit yaw (14-bit units; 0 = eye south of the target, facing
    /// north; increasing moves the eye east).
    pub yaw: f32,
    /// Orbit pitch (14-bit units, clamped to `[1077, 2787]`).
    pub pitch: f32,
    /// Viewer-only multiplier on the client's pitch-derived orbit distance
    /// (`1.0` = faithful); wheel zoom edits it.
    pub dist: f32,
    /// Viewport pixels, updated on resize.
    pub viewport: (u32, u32),
    /// Viewport FOV/zoom/clamp profile installed by CS2.
    pub viewport_profile: crate::camera::ViewportProfile,
    /// Effective scene rectangle set by the viewport command; the renderer
    /// still owns the full window, but projection must use this rect's size.
    pub scene_viewport: Option<[i32; 4]>,
    /// Viewer-only far-plane multiplier (`1.0` = the client's clip distances).
    pub far_scale: f32,
    pub map_size_x: i32,
    /// Camera state 3: the frame the retained UI's `cam2` owner produced
    /// this redraw; `None` keeps the orbit.
    pub cam2: Option<crate::camera::Cam2Frame>,
    /// Legacy camera states 1/5/6 pose and the camera zoom (camera.rs).
    pub legacy: Option<crate::camera::LegacyFrame>,
    pub zoom: Option<i32>,
}

impl OrbitCamera {
    /// Client defaults: yaw 0, `orbitCameraPitch = 1088`, faithful distance.
    pub fn new(target: glam::Vec3) -> Self {
        Self {
            target,
            yaw: 0.0,
            pitch: crate::camera::DEFAULT_ORBIT_PITCH,
            dist: 1.0,
            viewport: (1024, 768),
            viewport_profile: crate::camera::ViewportProfile::default(),
            scene_viewport: None,
            far_scale: 1.0,
            map_size_x: 104,
            cam2: None,
            legacy: None,
            zoom: None,
        }
    }

    /// The engine-unit camera for this state at `viewport`.
    pub fn scene_camera(&self, viewport: (u32, u32)) -> crate::camera::SceneCamera {
        let viewport = self.scene_viewport.map_or(viewport, |rect| {
            (rect[2].max(1) as u32, rect[3].max(1) as u32)
        });
        crate::camera::SceneCamera {
            target: [
                (self.target.x * 512.0) as i32,
                (-self.target.y * 512.0) as i32,
                (self.target.z * 512.0) as i32,
            ],
            pitch: self.pitch,
            yaw: self.yaw,
            distance_scale: self.dist,
            far_scale: self.far_scale,
            viewport: (viewport.0 as i32, viewport.1 as i32),
            viewport_profile: self.viewport_profile,
            map_size_x: self.map_size_x,
            cam2: self.cam2.clone(),
            legacy: self.legacy,
            zoom: self.zoom,
        }
    }

    /// Eye position (world units).
    pub fn position(&self) -> glam::Vec3 {
        self.scene_camera(self.viewport).eye_world()
    }

    /// Eye-to-target distance in tiles (pan speed reference).
    pub fn distance_tiles(&self) -> f32 {
        self.position().distance(self.target).max(1.0)
    }

    /// Drag-orbit: `dx`/`dy` in pixels (about 13 camera units per pixel).
    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw = (self.yaw - dx * 13.0).rem_euclid(16384.0);
        self.pitch = (self.pitch + dy * 13.0).clamp(
            crate::camera::ORBIT_PITCH_MIN,
            crate::camera::ORBIT_PITCH_MAX,
        );
    }

    /// Wheel zoom on the distance multiplier; `factor < 1` zooms in.
    pub fn zoom(&mut self, factor: f32) {
        self.dist = (self.dist * factor).clamp(0.25, 12.0);
    }

    /// Pan the target in the ground plane: `forward` along the view direction
    /// (yaw 0 faces north = +z), `strafe` along screen-right (east at yaw 0).
    pub fn pan(&mut self, forward: f32, strafe: f32) {
        let (sin_yaw, cos_yaw) = crate::trig::radians(self.yaw as i32).sin_cos();
        let fwd = glam::Vec3::new(-sin_yaw, 0.0, cos_yaw);
        let right = glam::Vec3::new(cos_yaw, 0.0, sin_yaw);
        self.target += fwd * forward + right * strafe;
    }

    /// Raise/lower the target (Q/E keys).
    pub fn nudge_vertical(&mut self, dy: f32) {
        self.target.y += dy;
    }
}
