//! The classic orbit camera and the view/projection chain.
//!
//! Scene space is **x east, y down (heights are negative upward),
//! z north** and the projection is a row-vector `w = z_view` projection
//! whose y column is negated before it reaches the GPU path
//! ([`glx_flip_y`]). That single negation is what turns
//! the y-down world into a y-up picture *without* mirroring east/west: facing
//! north, east is on the right. A viewer that only flips y in a right-handed
//! y-up camera is a reflection and shows the map mirrored, which is the bug
//! this module replaces.
//!
//! Everything here works in scene units (512 per tile, y negative upward) with
//! a fixed single-precision arithmetic order so the matrices are reproducible
//! bit for bit; [`SceneCamera::view_proj`] finally re-expresses the result over the
//! viewer's world units (scene units / 512, y negated) and wgpu's `[0, 1]` depth.

use crate::trig;
pub use rs910_core::animation_matrix::multiply;

/// Default maximum field-of-view zoom (used at small viewport heights).
pub const VIEWPORT_FOV_MAX: i32 = 256;
/// Default minimum field-of-view zoom (used at tall viewport heights).
pub const VIEWPORT_FOV_MIN: i32 = 205;
/// Default minimum orbit-distance zoom scale.
pub const VIEWPORT_ZOOM_MIN: i32 = 256;
/// Default maximum orbit-distance zoom scale.
pub const VIEWPORT_ZOOM_MAX: i32 = 320;

/// Mutable viewport lens limits, changed by the `viewport_set*` CS2 commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportProfile {
    pub fov_max: i32,
    pub fov_min: i32,
    pub zoom_min: i32,
    pub zoom_max: i32,
    pub min_fov: i32,
    pub max_fov: i32,
    pub min_height: i32,
    pub max_height: i32,
}

impl Default for ViewportProfile {
    fn default() -> Self {
        Self {
            fov_max: VIEWPORT_FOV_MAX,
            fov_min: VIEWPORT_FOV_MIN,
            zoom_min: VIEWPORT_ZOOM_MIN,
            zoom_max: VIEWPORT_ZOOM_MAX,
            min_fov: 1,
            max_fov: 32767,
            min_height: 1,
            max_height: 32767,
        }
    }
}
/// Initial orbit pitch.
pub const DEFAULT_ORBIT_PITCH: f32 = 1088.0;
/// Orbit pitch bounds.
pub const ORBIT_PITCH_MIN: f32 = 1077.0;
pub const ORBIT_PITCH_MAX: f32 = 2787.0;

/// Clip planes `(near, far)` in scene units for a `map_size_x`-tile window,
/// with the extended draw distance the GPU path always uses.
#[must_use]
pub fn clip_distances(map_size_x: i32) -> (i32, i32) {
    let near = 200;
    let mut far = if map_size_x == 0 {
        430
    } else {
        (f64::from(map_size_x) * 34.46) as i32
    };
    far <<= 2;
    far += 512;
    (near, far)
}

/// Camera zoom with the default fov and height limits (1/32767 each), i.e.
/// no letterboxing: the fov lerps from the maximum to the minimum
/// as the viewport height goes 334 → 434 and `zoom = h * fov / 334`.
#[must_use]
pub fn camera_zoom(width: i32, height: i32) -> i32 {
    camera_zoom_profile(width, height, ViewportProfile::default())
}

/// The camera zoom using the active mutable profile.
#[must_use]
pub fn camera_zoom_profile(_width: i32, height: i32, profile: ViewportProfile) -> i32 {
    let height = height.max(1);
    let height_blend = (height - 334).clamp(0, 100);
    let blended_fov = (profile.fov_min - profile.fov_max) * height_blend / 100 + profile.fov_max;
    height * blended_fov / 334
}

/// Where an orbit camera sits: the point it circles, the angles (14-bit) and
/// the distance from the point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Orbit {
    pub target: [i32; 3],
    pub pitch: i32,
    pub yaw: i32,
    pub distance: i32,
}

/// The eye of an orbit camera, scaled by the viewport-height zoom blend. The
/// camera's pitch and yaw are the orbit's own, roll is 0.
#[must_use]
pub fn orbit_camera(orbit: Orbit, viewport_height: i32) -> [i32; 3] {
    orbit_camera_with_profile(orbit, viewport_height, ViewportProfile::default())
}

/// [`orbit_camera`] with the active mutable zoom profile.
#[must_use]
pub fn orbit_camera_with_profile(
    orbit: Orbit,
    viewport_height: i32,
    profile: ViewportProfile,
) -> [i32; 3] {
    let Orbit {
        target,
        pitch,
        yaw,
        distance,
    } = orbit;
    let height_blend = (viewport_height - 334).clamp(0, 100);
    let zoom_scale = (profile.zoom_max - profile.zoom_min) * height_blend / 100 + profile.zoom_min;
    let scaled_distance = (distance * zoom_scale) >> 8;
    let pitch_angle = (16384 - pitch) & 0x3FFF;
    let yaw_angle = (16384 - yaw) & 0x3FFF;
    let mut offset_x = 0;
    let mut offset_y = 0;
    let mut offset_z = scaled_distance;
    if pitch_angle != 0 {
        offset_y = (trig::sin(pitch_angle) * -scaled_distance) >> 14;
        offset_z = (trig::cos(pitch_angle) * scaled_distance) >> 14;
    }
    if yaw_angle != 0 {
        offset_x = (trig::sin(yaw_angle) * offset_z) >> 14;
        offset_z = (trig::cos(yaw_angle) * offset_z) >> 14;
    }
    [
        target[0] - offset_x,
        target[1] - offset_y,
        target[2] - offset_z,
    ]
}

/// Orbit distance of the classic camera for a pitch:
/// `(pitch >> 3) * 3 + 600 << 2`.
#[must_use]
pub fn orbit_distance(pitch: i32) -> i32 {
    ((pitch >> 3) * 3 + 600) << 2
}

/// Row-vector affine 4x3 matrix, rows
/// `entry00..entry02` (x axis), `entry10..` (y), `entry20..` (z),
/// `entry30..entry32` (translation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix4x3 {
    pub e: [f32; 12],
}

impl Matrix4x3 {
    /// A pure translation.
    #[must_use]
    pub fn translation(x: f32, y: f32, z: f32) -> Self {
        Self {
            e: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, x, y, z],
        }
    }

    /// Post-multiply by the axis/angle rotation, single-precision order preserved.
    pub fn rotate_around_axis(&mut self, axis_x: f32, axis_y: f32, axis_z: f32, angle: f32) {
        let cos_angle = (f64::from(angle)).cos() as f32;
        let sin_angle = (f64::from(angle)).sin() as f32;
        let rot00 = axis_x * axis_x * (1.0 - cos_angle) + cos_angle;
        let rot01 = axis_x * axis_y * (1.0 - cos_angle) + axis_z * sin_angle;
        let rot02 = axis_x * axis_z * (1.0 - cos_angle) + -axis_y * sin_angle;
        let rot10 = axis_x * axis_y * (1.0 - cos_angle) + -axis_z * sin_angle;
        let rot11 = axis_y * axis_y * (1.0 - cos_angle) + cos_angle;
        let rot12 = axis_y * axis_z * (1.0 - cos_angle) + axis_x * sin_angle;
        let rot20 = axis_x * axis_z * (1.0 - cos_angle) + axis_y * sin_angle;
        let rot21 = axis_y * axis_z * (1.0 - cos_angle) + -axis_x * sin_angle;
        let rot22 = axis_z * axis_z * (1.0 - cos_angle) + cos_angle;
        let [e00, e01, e02, e10, e11, e12, e20, e21, e22, e30, e31, e32] = self.e;
        self.e = [
            e02 * rot20 + rot00 * e00 + rot10 * e01,
            e02 * rot21 + rot01 * e00 + rot11 * e01,
            e02 * rot22 + rot02 * e00 + rot12 * e01,
            e12 * rot20 + rot00 * e10 + rot10 * e11,
            e12 * rot21 + rot01 * e10 + rot11 * e11,
            e12 * rot22 + rot02 * e10 + rot12 * e11,
            e22 * rot20 + rot00 * e20 + rot10 * e21,
            e22 * rot21 + rot01 * e20 + rot11 * e21,
            e22 * rot22 + rot02 * e20 + rot12 * e21,
            e32 * rot20 + rot00 * e30 + rot10 * e31,
            e32 * rot21 + rot01 * e30 + rot11 * e31,
            e32 * rot22 + rot02 * e30 + rot12 * e31,
        ];
    }

    /// A look-at view matrix from `eye` to `target` with the given up vector.
    /// The eye and target are doubles and the products floats, in this order,
    /// so the matrix matches the legacy client bit for bit.
    pub fn set_look_at(&mut self, eye: [f64; 3], target: [f64; 3], up: [f32; 3]) {
        let [eye_x, eye_y, eye_z] = eye;
        let [target_x, target_y, target_z] = target;
        let [up_x, up_y, up_z] = up;
        let forward_x = (target_x - eye_x) as f32;
        let forward_y = (target_y - eye_y) as f32;
        let forward_z = (target_z - eye_z) as f32;
        let side_x = up_y * forward_z - up_z * forward_y;
        let side_y = up_z * forward_x - up_x * forward_z;
        let side_z = up_x * forward_y - up_y * forward_x;
        let side_inv_length =
            (1.0 / f64::from(side_z * side_z + side_x * side_x + side_y * side_y).sqrt()) as f32;
        let forward_inv_length = (1.0
            / f64::from(forward_z * forward_z + forward_x * forward_x + forward_y * forward_y)
                .sqrt()) as f32;
        let e00 = side_x * side_inv_length;
        let e10 = side_y * side_inv_length;
        let e20 = side_z * side_inv_length;
        let e02 = forward_x * forward_inv_length;
        let e12 = forward_y * forward_inv_length;
        let e22 = forward_z * forward_inv_length;
        let e01 = e12 * e20 - e22 * e10;
        let e11 = e22 * e00 - e02 * e20;
        let e21 = e10 * e02 - e12 * e00;
        let e30 =
            -((f64::from(e20) * eye_z + f64::from(e10) * eye_y + f64::from(e00) * eye_x) as f32);
        let e31 =
            -((f64::from(e21) * eye_z + f64::from(e11) * eye_y + f64::from(e01) * eye_x) as f32);
        let e32 =
            -((f64::from(e22) * eye_z + f64::from(e12) * eye_y + f64::from(e02) * eye_x) as f32);
        self.e = [e00, e01, e02, e10, e11, e12, e20, e21, e22, e30, e31, e32];
    }

    /// Add to the translation row.
    pub fn translate(&mut self, x: f32, y: f32, z: f32) {
        self.e[9] += x;
        self.e[10] += y;
        self.e[11] += z;
    }

    /// Row-major 4x4 entries with the translation in `[12..15]`.
    #[must_use]
    pub fn to_entries(self) -> [f32; 16] {
        let e = &self.e;
        [
            e[0], e[1], e[2], 0.0, e[3], e[4], e[5], 0.0, e[6], e[7], e[8], 0.0, e[9], e[10],
            e[11], 1.0,
        ]
    }
}

/// A pinhole lens over a pixel viewport: the optical centre and focal
/// lengths in pixels, the near and far planes and the viewport size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelLens {
    pub centre: [f32; 2],
    pub focal: [f32; 2],
    pub near: f32,
    pub far: f32,
    pub size: [f32; 2],
}

/// The viewport-pixel perspective projection of `lens`: row-major entries,
/// `w = z`.
#[must_use]
pub fn perspective_pixels(lens: PixelLens) -> [f32; 16] {
    let PixelLens {
        centre: [centre_x, centre_y],
        focal: [focal_x, focal_y],
        near,
        far,
        size: [width, height],
    } = lens;
    let mut m = [0.0_f32; 16];
    m[0] = focal_x * 2.0 / width;
    m[5] = focal_y * 2.0 / height;
    m[8] = centre_x * 2.0 / width - 1.0;
    m[9] = centre_y * 2.0 / height - 1.0;
    m[10] = (near + far) / (far - near);
    m[11] = 1.0;
    m[14] = far * 2.0 * near / (near - far);
    m
}

/// The perspective projection for `(near, far, fovX, fovY)` built through
/// the frustum form: row-major entries, `w = z`.
#[must_use]
pub fn perspective_fov(near_distance: f32, far_distance: f32, fov_x: f32, fov_y: f32) -> [f32; 16] {
    let half_width = (f64::from(fov_x / 2.0).tan() * f64::from(near_distance)) as f32;
    let half_height = (f64::from(fov_y / 2.0).tan() * f64::from(near_distance)) as f32;
    let (l, r, b, t, near, far) = (
        -half_width,
        half_width,
        -half_height,
        half_height,
        near_distance,
        far_distance,
    );
    let mut m = [0.0_f32; 16];
    m[0] = near * 2.0 / (r - l);
    m[5] = near * 2.0 / (t - b);
    m[8] = (l + r) / (r - l);
    m[9] = (b + t) / (t - b);
    m[10] = (near + far) / (far - near);
    m[11] = 1.0;
    m[14] = -(far * 2.0 * near) / (far - near);
    m
}

/// The orthographic projection of a box, row-major entries, `w = 1`.
#[must_use]
pub fn orthographic(
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    near: f32,
    far: f32,
) -> [f32; 16] {
    let mut m = [0.0_f32; 16];
    m[0] = 2.0 / (right - left);
    m[5] = 2.0 / (top - bottom);
    m[10] = 2.0 / (far - near);
    m[12] = -(left + right) / (right - left);
    m[13] = -(bottom + top) / (top - bottom);
    m[14] = -(near + far) / (far - near);
    m[15] = 1.0;
    m
}

/// A square orthographic volume of half-extent `10000 / scale`.
#[must_use]
pub fn orthographic_scaled(near: f32, far: f32, scale: f32) -> [f32; 16] {
    orthographic(
        -10000.0 / scale,
        10000.0 / scale,
        -10000.0 / scale,
        10000.0 / scale,
        near,
        far,
    )
}

/// One camera effect applied for the frame: `Shake`
/// (`offset = magnitude * sin(phase)`, modes 0-2 translate x/y/z, 3-5 rotate
/// about x/y/z) or `Tilt`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameEffect {
    Shake { mode: i32, offset: f32 },
    Tilt(f32),
}

/// What the scripted camera (camera state 3) contributes to a frame:
/// the eye and look-at points in scene units (y down) relative to
/// [`SceneCamera::target`], the renderer's frame origin, the camera angles the
/// draw plan culls with, the depth planes, the field of view and the effects.
#[derive(Clone, Debug, PartialEq)]
pub struct Cam2Frame {
    pub eye: [f32; 3],
    pub lookat: [f32; 3],
    /// Camera pitch and yaw in radians.
    pub pitch: f32,
    pub yaw: f32,
    pub near: f32,
    pub far: f32,
    pub fov: [f32; 2],
    /// Projection mode id (0 = perspective) and the orthographic scale
    /// (default 5).
    pub projection_mode: i32,
    pub projection_scale: i32,
    pub effects: Vec<FrameEffect>,
}
impl Cam2Frame {
    /// The unflipped projection of the scripted camera: mode 0 is the
    /// perspective frustum; any other mode (including an unknown id) takes
    /// the orthographic volume.
    #[must_use]
    pub fn projection(&self) -> [f32; 16] {
        if self.projection_mode == 0 {
            perspective_fov(self.near, self.far, self.fov[0], self.fov[1])
        } else {
            orthographic_scaled(self.near, self.far, self.projection_scale as f32)
        }
    }
    /// Express the points relative to `anchor`.
    pub fn rebase(&mut self, anchor: [i32; 3]) {
        for (i, &a) in anchor.iter().enumerate() {
            self.eye[i] -= a as f32;
            self.lookat[i] -= a as f32;
        }
    }
    /// Look-at view matrix from the eye to the look-at point with up
    /// `(0, 1, 0)`, then every effect applied to the matrix.
    /// `origin` is the frame origin the relative points are added to.
    #[must_use]
    pub fn view_matrix(&self, origin: [i32; 3]) -> Matrix4x3 {
        let mut view = Matrix4x3::translation(0.0, 0.0, 0.0);
        let [ex, ey, ez] = [
            self.eye[0] + origin[0] as f32,
            self.eye[1] + origin[1] as f32,
            self.eye[2] + origin[2] as f32,
        ];
        let [lx, ly, lz] = [
            self.lookat[0] + origin[0] as f32,
            self.lookat[1] + origin[1] as f32,
            self.lookat[2] + origin[2] as f32,
        ];
        view.set_look_at(
            [f64::from(ex), f64::from(ey), f64::from(ez)],
            [f64::from(lx), f64::from(ly), f64::from(lz)],
            [0.0, 1.0, 0.0],
        );
        for effect in &self.effects {
            match *effect {
                FrameEffect::Shake { mode: 0, offset } => view.translate(offset, 0.0, 0.0),
                FrameEffect::Shake { mode: 1, offset } => view.translate(0.0, offset, 0.0),
                FrameEffect::Shake { mode: 2, offset } => view.translate(0.0, 0.0, offset),
                FrameEffect::Shake { mode: 3, offset } => {
                    view.rotate_around_axis(1.0, 0.0, 0.0, offset)
                }
                FrameEffect::Shake { mode: 4, offset } => {
                    view.rotate_around_axis(0.0, 1.0, 0.0, offset)
                }
                FrameEffect::Shake { mode: 5, offset } => {
                    view.rotate_around_axis(0.0, 0.0, 1.0, offset)
                }
                FrameEffect::Shake { .. } => {}
                FrameEffect::Tilt(angle) => view.rotate_around_axis(0.0, 0.0, 1.0, angle),
            }
        }
        view
    }
    /// The eye the scene draw is culled from: the eye plus the translating
    /// shakes, truncated to integers.
    #[must_use]
    pub fn eye(&self, origin: [i32; 3]) -> [i32; 3] {
        let mut eye = [
            (self.eye[0] + origin[0] as f32) as i32,
            (self.eye[1] + origin[1] as f32) as i32,
            (self.eye[2] + origin[2] as f32) as i32,
        ];
        for effect in &self.effects {
            match *effect {
                FrameEffect::Shake { mode: 0, offset } => eye[0] = (eye[0] as f32 + offset) as i32,
                FrameEffect::Shake { mode: 1, offset } => eye[1] = (eye[1] as f32 + offset) as i32,
                FrameEffect::Shake { mode: 2, offset } => eye[2] = (eye[2] as f32 + offset) as i32,
                _ => {}
            }
        }
        eye
    }
}

/// Negate the projection's y column (entries 1, 5, 9, 13). Applied to every
/// projection the GPU path receives.
pub fn glx_flip_y(m: &mut [f32; 16]) {
    m[1] = -m[1];
    m[5] = -m[5];
    m[9] = -m[9];
    m[13] = -m[13];
}

/// CPU projection. Unlike the GPU
/// matrix this retains y-down clip space and GL's [-w,w] depth range.
#[derive(Clone, Debug)]
pub struct CpuProjection {
    pub view: [f32; 16],
    pub projection: [f32; 16],
    pub combined: [f32; 16],
    pub viewport: [i32; 4],
}

impl CpuProjection {
    pub fn new(view: [f32; 16], projection: [f32; 16], viewport: [i32; 4]) -> Self {
        Self {
            view,
            projection,
            combined: multiply(&view, &projection),
            viewport,
        }
    }

    /// No clipping, z is view depth, not normalized depth.
    pub fn project(&self, [x, y, z]: [f32; 3]) -> [f32; 3] {
        let m = &self.combined;
        let w = m[11] * z + m[7] * y + m[3] * x + m[15];
        let cx = m[8] * z + m[4] * y + m[0] * x + m[12];
        let cy = m[9] * z + m[5] * y + m[1] * x + m[13];
        let v = &self.view;
        let depth = v[10] * z + v[6] * y + v[2] * x + v[14];
        let half_w = self.viewport[2] as f32 / 2.0;
        let half_h = self.viewport[3] as f32 / 2.0;
        [
            half_w * cx / w + (self.viewport[0] as f32 + half_w),
            half_h * cy / w + (self.viewport[1] as f32 + half_h),
            depth,
        ]
    }

    /// The clipped projection used for 2D entity elements. A point outside
    /// the clip volume yields NaN in every component.
    pub fn project_clipped(&self, [x, y, z]: [f32; 3]) -> [f32; 3] {
        let m = &self.combined;
        let w = m[11] * z + m[7] * y + m[3] * x + m[15];
        let cz = m[10] * z + m[6] * y + m[2] * x + m[14];
        let cx = m[8] * z + m[4] * y + m[0] * x + m[12];
        let cy = m[9] * z + m[5] * y + m[1] * x + m[13];
        if cz < -w || cz > w || cx < -w || cx > w || cy < -w || cy > w {
            return [f32::NAN; 3];
        }
        self.project([x, y, z])
    }

    /// Common out-code of a segment's endpoints.
    pub fn segment_code(&self, a: [i32; 3], b: [i32; 3]) -> i32 {
        let m = &self.combined;
        let component = |p: [i32; 3], c: usize| {
            m[8 + c] * p[2] as f32 + m[4 + c] * p[1] as f32 + m[c] * p[0] as f32 + m[12 + c]
        };
        let (z0, z1, w0, w1) = (
            component(a, 2),
            component(b, 2),
            component(a, 3),
            component(b, 3),
        );
        let mut code = 0;
        if z0 < -w0 && z1 < -w1 {
            code |= 16;
        } else if z0 > w0 && z1 > w1 {
            code |= 32;
        }
        let (x0, x1) = (component(a, 0), component(b, 0));
        if x0 < -w0 && x1 < -w1 {
            code |= 1;
        }
        if x0 > w0 && x1 > w1 {
            code |= 2;
        }
        let (y0, y1) = (component(a, 1), component(b, 1));
        if y0 < -w0 && y1 < -w1 {
            code |= 4;
        }
        if y0 > w0 && y1 > w1 {
            code |= 8;
        }
        code
    }
}

/// Row-major entries → `glam::Mat4` acting on column vectors
/// (the row-vector product `v * M` equals `M_glam * v`).
#[must_use]
pub fn to_glam(m: &[f32; 16]) -> glam::Mat4 {
    glam::Mat4::from_cols_array(m)
}

/// GL clip depth `[-w, w]` → wgpu `[0, w]`: `z' = 0.5 z + 0.5 w`.
#[must_use]
pub fn gl_to_wgpu_depth() -> glam::Mat4 {
    glam::Mat4::from_cols(
        glam::Vec4::new(1.0, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, 1.0, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, 0.5, 0.0),
        glam::Vec4::new(0.0, 0.0, 0.5, 1.0),
    )
}

/// Camera position and roll/pitch/yaw as drawn by the non-scripted legacy
/// states (1 transition, 5 cutscene move/look, 6 spline move-along), with
/// 14-bit angles. Like [`Cam2Frame`] the
/// eye is relative to [`SceneCamera::target`], which the caller places on the
/// view axis (the renderer derives its distance cut from target - eye).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyFrame {
    pub eye: [i32; 3],
    pub pitch: i32,
    pub yaw: i32,
    pub roll: i32,
}

/// The classic third-person camera state (any camera state but the scripted
/// one): orbit target in scene units, orbit pitch/yaw,
/// viewport size, and the window size the clip planes derive from.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneCamera {
    /// Orbit target x, follow height (terrain height under the player minus
    /// the follow-height offset) and target z, in scene units, y negative
    /// upward.
    pub target: [i32; 3],
    /// Orbit pitch (float, clamped to `[1077, 2787]`).
    pub pitch: f32,
    /// Orbit yaw (float, wrapped to `[0, 16384)`).
    pub yaw: f32,
    /// Viewer-only zoom knob on the orbit distance (`1.0` = the client's
    /// pitch-derived distance). CS2 `viewportZoom*` is applied separately by
    /// [`Self::viewport_profile`].
    pub distance_scale: f32,
    /// Viewer-only far-plane multiplier (`1.0` = the default clip distance).
    pub far_scale: f32,
    /// Viewport `(width, height)` in pixels.
    pub viewport: (i32, i32),
    /// Mutable viewport lens/clamp profile.
    pub viewport_profile: ViewportProfile,
    /// Map width in tiles, for the clip distances.
    pub map_size_x: i32,
    /// Scripted camera (state 3): the frame comes from this instead of the
    /// orbit fields above.
    pub cam2: Option<Cam2Frame>,
    /// Legacy states 1/5/6: the explicit pose replaces the orbit.
    pub legacy: Option<LegacyFrame>,
    /// Camera zoom set by the viewport or the state 1
    /// transition; `None` derives it from the viewport profile.
    pub zoom: Option<i32>,
}

impl SceneCamera {
    /// Defaults at a 104-tile window.
    #[must_use]
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "defaults constructor; exercised by tests only")
    )]
    pub fn new(target: [i32; 3]) -> Self {
        Self {
            target,
            pitch: DEFAULT_ORBIT_PITCH,
            yaw: 0.0,
            distance_scale: 1.0,
            far_scale: 1.0,
            viewport: (1024, 768),
            viewport_profile: ViewportProfile::default(),
            map_size_x: 104,
            cam2: None,
            legacy: None,
            zoom: None,
        }
    }

    /// The orbit pitch truncated to an integer, clamped to the pitch bounds.
    #[must_use]
    pub fn pitch_int(&self) -> i32 {
        if let Some(l) = &self.legacy {
            return l.pitch;
        }
        if let Some(c) = &self.cam2 {
            // Radians to 14-bit angle units (16384 / 2π = 2607.59...).
            return (f64::from(c.pitch) * 2607.5945876176133) as i32 & 0x3FFF;
        }
        self.pitch.clamp(ORBIT_PITCH_MIN, ORBIT_PITCH_MAX) as i32
    }

    /// The orbit yaw truncated to an integer and wrapped to 14 bits.
    #[must_use]
    pub fn yaw_int(&self) -> i32 {
        if let Some(l) = &self.legacy {
            return l.yaw & 0x3FFF;
        }
        if let Some(c) = &self.cam2 {
            // Radians to 14-bit angle units.
            return (f64::from(c.yaw) * 2607.5945876176133) as i32 & 0x3FFF;
        }
        (self.yaw as i32) & 0x3FFF
    }

    /// The orbit distance for the current pitch, with the viewer knob.
    #[must_use]
    pub fn distance(&self) -> i32 {
        (orbit_distance(self.pitch_int()) as f32 * self.distance_scale) as i32
    }

    /// The camera eye in scene units.
    #[must_use]
    pub fn eye(&self) -> [i32; 3] {
        if let Some(l) = &self.legacy {
            return [
                self.target[0] + l.eye[0],
                self.target[1] + l.eye[1],
                self.target[2] + l.eye[2],
            ];
        }
        if let Some(c) = &self.cam2 {
            return c.eye(self.target);
        }
        orbit_camera_with_profile(
            Orbit {
                target: self.target,
                pitch: self.pitch_int(),
                yaw: self.yaw_int(),
                distance: self.distance(),
            },
            self.viewport.1,
            self.viewport_profile,
        )
    }

    /// The view matrix for the classic camera.
    #[must_use]
    pub fn view_matrix(&self) -> Matrix4x3 {
        if let Some(c) = &self.cam2 {
            return c.view_matrix(self.target);
        }
        let [cx, cy, cz] = self.eye();
        let roll = self.legacy.map_or(0, |l| l.roll);
        let mut view = Matrix4x3::translation(-(cx as f32), -(cy as f32), -(cz as f32));
        view.rotate_around_axis(0.0, -1.0, 0.0, trig::radians(-self.yaw_int() & 0x3FFF));
        view.rotate_around_axis(-1.0, 0.0, 0.0, trig::radians(-self.pitch_int() & 0x3FFF));
        view.rotate_around_axis(0.0, 0.0, -1.0, trig::radians(-roll & 0x3FFF));
        view
    }

    /// The projection for the classic camera followed by the y flip:
    /// row-major entries.
    #[must_use]
    pub fn projection(&self) -> [f32; 16] {
        if let Some(c) = &self.cam2 {
            // Scripted camera: perspective or orthographic by mode.
            let mut m = c.projection();
            glx_flip_y(&mut m);
            return m;
        }
        let (w, h) = (self.viewport.0.max(1), self.viewport.1.max(1));
        let zoom = self
            .zoom
            .unwrap_or_else(|| camera_zoom_profile(w, h, self.viewport_profile))
            << 1;
        let (near, far) = clip_distances(self.map_size_x);
        let far = (far as f32 * self.far_scale) as i32;
        let mut m = perspective_pixels(PixelLens {
            centre: [(w / 2) as f32, (h / 2) as f32],
            focal: [zoom as f32, zoom as f32],
            near: near as f32,
            far: far as f32,
            size: [w as f32, h as f32],
        });
        glx_flip_y(&mut m);
        m
    }

    /// `(far, near_min)` recovered from the projection:
    /// `far = (m14 - m15) / (m11 - m10)`, `nearMin = -m14 / m10`.
    #[must_use]
    pub fn fog_reference(&self) -> (f32, f32) {
        let m = self.projection();
        ((m[14] - m[15]) / (m[11] - m[10]), -m[14] / m[10])
    }

    /// The view matrix as row-major 4x4 entries.
    #[must_use]
    pub fn view_entries(&self) -> [f32; 16] {
        self.view_matrix().to_entries()
    }

    /// `view * projection` over scene units, as a column-vector `glam::Mat4`
    /// with wgpu depth.
    #[must_use]
    pub fn view_proj_scene_units(&self) -> glam::Mat4 {
        let wvp = multiply(&self.view_matrix().to_entries(), &self.projection());
        gl_to_wgpu_depth() * to_glam(&wvp)
    }

    /// The same matrix over the viewer's world units (scene units / 512, y
    /// negated), i.e. what every existing mesh path multiplies by.
    #[must_use]
    pub fn view_proj(&self) -> glam::Mat4 {
        self.view_proj_scene_units() * glam::Mat4::from_scale(glam::Vec3::new(512.0, -512.0, 512.0))
    }

    /// Eye position in viewer world units.
    #[must_use]
    pub fn eye_world(&self) -> glam::Vec3 {
        let [x, y, z] = self.eye();
        glam::Vec3::new(x as f32 / 512.0, -(y as f32) / 512.0, z as f32 / 512.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fog_reference_recovers_far_plane() {
        let cam = SceneCamera::new([0, 0, 0]);
        let (far, near_min) = cam.fog_reference();
        assert!((far - 14844.0).abs() < 0.5, "{far}");
        // -m14/m10 = 2fn/(n+f)
        assert!(
            (near_min - 2.0 * 14844.0 * 200.0 / (200.0 + 14844.0)).abs() < 0.5,
            "{near_min}"
        );
    }

    #[test]
    fn clip_distances_match_world() {
        // 104 tiles: (int)(104 * 34.46) = 3583, << 2 = 14332, + 512.
        assert_eq!(clip_distances(104), (200, 14844));
        assert_eq!(clip_distances(0), (200, 430 * 4 + 512));
    }

    #[test]
    fn camera_zoom_lerps_fov_over_viewport_height() {
        assert_eq!(camera_zoom(800, 334), 256);
        assert_eq!(camera_zoom(800, 434), 434 * 205 / 334);
        assert_eq!(camera_zoom(800, 768), 768 * 205 / 334);
    }

    #[test]
    fn viewport_profile_reaches_zoom_and_orbit() {
        let profile = ViewportProfile {
            fov_max: 300,
            fov_min: 300,
            zoom_min: 512,
            zoom_max: 512,
            ..ViewportProfile::default()
        };
        assert_eq!(camera_zoom_profile(800, 334, profile), 300);
        assert_eq!(
            orbit_camera_with_profile(
                Orbit {
                    target: [0, 0, 0],
                    pitch: 0,
                    yaw: 0,
                    distance: 4096
                },
                334,
                profile
            ),
            [0, 0, -8192]
        );
        let mut cam = SceneCamera::new([0, 0, 0]);
        cam.viewport_profile = profile;
        assert_ne!(cam.projection(), SceneCamera::new([0, 0, 0]).projection());
    }

    #[test]
    fn orbit_camera_yaw_zero_sits_south_of_target() {
        // pitch 0 (flat), yaw 0: eye is `dist` due south (-z), same height.
        let orbit = |pitch, yaw| Orbit {
            target: [1000, -100, 2000],
            pitch,
            yaw,
            distance: 4096,
        };
        let eye = orbit_camera(orbit(0, 0), 334);
        assert_eq!(eye, [1000, -100, 2000 - 4096]);
        // pitch 4096 (straight up): eye is `dist` above (scene y negative).
        let eye = orbit_camera(orbit(4096, 0), 334);
        assert_eq!(eye, [1000, -100 - 4096, 2000]);
        // yaw 4096: eye east of the target.
        let eye = orbit_camera(orbit(0, 4096), 334);
        assert_eq!(eye, [1000 + 4096, -100, 2000]);
    }

    #[test]
    fn facing_north_puts_east_on_the_right_and_up_up() {
        let cam = SceneCamera {
            target: [0, 0, 0],
            viewport: (800, 600),
            ..SceneCamera::new([0, 0, 0])
        };
        let vp = cam.view_proj_scene_units();
        let [ex, ey, ez] = cam.eye();
        assert!(ez < 0, "yaw 0 eye is south of the target");
        assert!(ey < 0, "eye is above the target (scene y negative)");
        assert_eq!(ex, 0);
        let project = |x: f32, y: f32, z: f32| {
            let c = vp * glam::Vec4::new(x, y, z, 1.0);
            (c.x / c.w, c.y / c.w, c.z / c.w)
        };
        // A point east of the target projects right of it.
        let (cx, _, _) = project(0.0, 0.0, 0.0);
        let (east_x, _, _) = project(512.0, 0.0, 0.0);
        assert!(east_x > cx, "east must be on the right: {east_x} vs {cx}");
        // A point above the target (scene y negative) projects higher.
        let (_, cy, _) = project(0.0, 0.0, 0.0);
        let (_, up_y, _) = project(0.0, -512.0, 0.0);
        assert!(up_y > cy, "up must be up: {up_y} vs {cy}");
        // A point further north is further away: larger wgpu depth.
        let (_, _, near_z) = project(0.0, 0.0, 0.0);
        let (_, _, far_z) = project(0.0, 0.0, 2048.0);
        assert!(far_z > near_z && (0.0..=1.0).contains(&far_z));
    }

    #[test]
    fn world_matrix_agrees_with_scene_matrix() {
        let cam = SceneCamera::new([3222 * 512, -600, 3222 * 512]);
        let scene_units = cam.view_proj_scene_units()
            * glam::Vec4::new(3230.0 * 512.0, -1000.0, 3210.0 * 512.0, 1.0);
        let world = cam.view_proj() * glam::Vec4::new(3230.0, 1000.0 / 512.0, 3210.0, 1.0);
        assert!(
            (scene_units - world).abs().max_element() < 1e-3,
            "{scene_units} vs {world}"
        );
    }

    #[test]
    fn cam2_frame_looks_from_eye_to_target() {
        // Eye south of the target and above it (scene y negative), looking north.
        let frame = Cam2Frame {
            eye: [0.0, -1000.0, -2000.0],
            lookat: [0.0, 0.0, 0.0],
            pitch: 0.4636476,
            yaw: 0.0,
            near: 50.0,
            far: 14847.0,
            fov: [1.5707964, 1.2],
            projection_mode: 0,
            projection_scale: 5,
            effects: vec![],
        };
        let cam = SceneCamera {
            cam2: Some(frame),
            viewport: (800, 600),
            ..SceneCamera::new([0, 0, 0])
        };
        assert_eq!(cam.eye(), [0, -1000, -2000]);
        assert_eq!(
            (cam.pitch_int(), cam.yaw_int()),
            [(0.4636476_f64 * 2607.5945876176133) as i32, 0].into()
        );
        // The frame is relative to the target: moving the origin moves the eye.
        let moved = SceneCamera {
            target: [1000, 0, 0],
            ..cam.clone()
        };
        assert_eq!(moved.eye(), [1000, -1000, -2000]);
        assert_ne!(moved.view_matrix(), cam.view_matrix());
        let vp = cam.view_proj_scene_units();
        let project = |x: f32, y: f32, z: f32| {
            let c = vp * glam::Vec4::new(x, y, z, 1.0);
            (c.x / c.w, c.y / c.w, c.z / c.w)
        };
        // The look-at point is the screen centre; east is right, up is up.
        let (cx, cy, _) = project(0.0, 0.0, 0.0);
        assert!(cx.abs() < 1e-4 && cy.abs() < 1e-4, "{cx} {cy}");
        let (east_x, _, _) = project(512.0, 0.0, 0.0);
        assert!(east_x > cx);
        let (_, up_y, _) = project(0.0, -512.0, 0.0);
        assert!(up_y > cy);
        let (_, _, near_z) = project(0.0, 0.0, 0.0);
        let (_, _, far_z) = project(0.0, 0.0, 2048.0);
        assert!(far_z > near_z && (0.0..=1.0).contains(&far_z));
        // A translating shake moves the culling eye too.
        let shaken = SceneCamera {
            cam2: Some(Cam2Frame {
                effects: vec![FrameEffect::Shake {
                    mode: 0,
                    offset: 12.5,
                }],
                ..cam.cam2.clone().unwrap()
            }),
            ..cam.clone()
        };
        assert_eq!(shaken.eye(), [12, -1000, -2000]);
        let mut rebased = cam.cam2.clone().unwrap();
        rebased.rebase([0, -1000, -2000]);
        assert_eq!(
            (rebased.eye, rebased.lookat),
            ([0.0; 3], [0.0, 1000.0, 2000.0])
        );
        assert_ne!(shaken.view_matrix(), cam.view_matrix());
    }

    /// Non-zero projection modes (including an unknown id) use the
    /// orthographic volume `+-10000 / projection_scale`, which flows through
    /// `SceneCamera::projection` (draw, culling and the picking unprojection
    /// all read it).
    #[test]
    fn cam2_orthographic_projection_mode() {
        let frame = Cam2Frame {
            eye: [0.0, 0.0, -2000.0],
            lookat: [0.0, 0.0, 0.0],
            pitch: 0.0,
            yaw: 0.0,
            near: 50.0,
            far: 4050.0,
            fov: [1.5707964, 1.2],
            projection_mode: 1,
            projection_scale: 5,
            effects: vec![],
        };
        let m = frame.projection();
        // Orthographic box (-2000, 2000, -2000, 2000, 50, 4050).
        assert_eq!(m[0], 2.0 / 4000.0);
        assert_eq!(m[5], 2.0 / 4000.0);
        assert_eq!(m[10], 2.0 / 4000.0);
        assert_eq!(m[11], 0.0);
        assert_eq!((m[12], m[13]), (0.0, 0.0));
        assert_eq!(m[14], -4100.0 / 4000.0);
        assert_eq!(m[15], 1.0);
        for mode in [2, 7] {
            let other = Cam2Frame {
                projection_mode: mode,
                ..frame.clone()
            };
            assert_eq!(other.projection(), m);
        }
        let perspective = Cam2Frame {
            projection_mode: 0,
            ..frame.clone()
        };
        assert_eq!(
            perspective.projection(),
            perspective_fov(50.0, 4050.0, 1.5707964, 1.2)
        );
        // Through the camera: a point's screen x no longer depends on depth.
        let cam = SceneCamera {
            cam2: Some(frame),
            viewport: (800, 600),
            ..SceneCamera::new([0, 0, 0])
        };
        let mut unflipped = cam.projection();
        glx_flip_y(&mut unflipped);
        assert_eq!(unflipped, m);
        let cpu = CpuProjection::new(cam.view_entries(), unflipped, [0, 0, 800, 600]);
        let near = cpu.project([1000.0, 0.0, -1000.0]);
        let far = cpu.project([1000.0, 0.0, 1500.0]);
        assert!((near[0] - far[0]).abs() < 1e-3, "{near:?} {far:?}");
        assert!((near[0] - (400.0 + 400.0 * 1000.0 / 2000.0)).abs() < 1e-2);
    }
}
