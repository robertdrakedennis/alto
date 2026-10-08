//! The scripted camera: an eye (the position owner) and a focus (the
//! look-at owner) chasing targets under linear movement laws or splines,
//! plus shake and tilt effects and the classic pre-script camera poses.
//!
//! State comes from three places: the `cam2_*` / `cam_*` script commands
//! (`Cam2::dispatch`), the camera update packet (`Cam2::decode`) and the
//! per-cycle scene snapshot (`Cam2::sync_scene`). The staff free-fly camera
//! ([`FreeCamera`]) is a client-owned camera with a point eye and an
//! orientation focus. Entity trackables come from the local player and the
//! retained NPC feed. Orientation mode, position and look-at splines and the
//! point owners' coordinate-object lane are live.

#[cfg(test)]
use crate::protocol910::terrain::Terrain;
#[cfg(test)]
use native910::vm::{Value, VmResult};

mod movement;
pub use movement::Movement;
mod scene;
pub use scene::{Heightmap, Scene, TrackableRef};
mod focus;
pub use focus::{CoupledSplineFocus, EntityFocus, Focus, OrientationFocus, PointFocus};
mod position;
pub use position::{EntityPlacement, EntityPosition, PointPosition};
mod spline;
use spline::CameraSplineKind;
use spline::CameraSplinePath;
pub use spline::SplineTrack;
mod legacy;
use legacy::ANGLE_UNITS_PER_RADIAN;
pub use legacy::{
    random_unit, LegacyCamera, LegacyLook, LegacyModifier, LegacyMove, LegacyMoveAlong, LegacyPose,
    LegacyTransition, LegacyView,
};
mod free_camera;
pub use free_camera::FreeCamera;
mod commands;

pub use crate::cam2_scene::*;

pub use rs910_core::vector_math::*;

#[path = "ui_cam2_packet.rs"]
mod packet;

/// Control mode: 0 is server-controlled, 1 is client-controlled.
pub const CLIENT: i32 = 1;

/// Logic updates per second (one every 20 ms).
pub const LOGIC_RATE: i32 = 50;

/// Owner mode ids shared by the focus and the eye: point 0, entity 1.
pub const MODE_POINT: i32 = 0;

pub const MODE_ENTITY: i32 = 1;

/// Focus mode 3: an orientation instead of a target point.
pub const MODE_ORIENTATION: i32 = 3;

/// Angle units (16384 per turn) per radian.
const RADIANS_TO_UNITS: f64 = 2607.5945876176133;

/// Private object-stack representation of coordinate values.
///
/// The native VM currently exposes one UTF-8 object lane to engine hosts,
/// while the original object stack is heterogeneous. Keeping the value tagged
/// here lets the ordinary host/consumer path preserve the object-stack
/// ordering without teaching string operations to understand a camera
/// coordinate object.
const COORD_FINE_TAG: &str = "\u{1}client910.coordfine:";

pub(crate) fn encode_coord_fine(level: i32, coord: [i32; 3]) -> String {
    format!(
        "{COORD_FINE_TAG}{level},{},{},{}",
        coord[0], coord[1], coord[2]
    )
}

pub(crate) fn decode_coord_fine(value: &str) -> Option<(i32, [i32; 3])> {
    let fields = value
        .strip_prefix(COORD_FINE_TAG)?
        .split(',')
        .collect::<Vec<_>>();
    (fields.len() == 4).then_some(())?;
    Some((
        fields[0].parse().ok()?,
        [
            fields[1].parse().ok()?,
            fields[2].parse().ok()?,
            fields[3].parse().ok()?,
        ],
    ))
}

/// Pitch of the rotated forward vector.
pub fn pitch_of(q: &Quat) -> f32 {
    let mut forward = Vec3::new(0.0, 0.0, 1.0);
    forward.rotate(q);
    (std::f64::consts::FRAC_PI_2 - f64::from(forward.y).acos()) as f32
}

/// Yaw of the rotated forward vector in `[0, 2π)`.
pub fn yaw_of(q: &Quat) -> f32 {
    let mut forward = Vec3::new(0.0, 0.0, 1.0);
    forward.rotate(q);
    let mut yaw = f64::from(forward.x).atan2(f64::from(forward.z));
    if yaw < 0.0 {
        yaw = yaw + std::f64::consts::PI + std::f64::consts::PI;
    }
    yaw as f32
}

/// `atan2(a, b)` normalised to `[0, 2π)`.
pub fn atan2_positive(a: f32, b: f32) -> f32 {
    let mut angle = f64::from(a).atan2(f64::from(b));
    if angle < 0.0 {
        angle = angle + std::f64::consts::PI + std::f64::consts::PI;
    }
    angle as f32
}

/// Intersections of a ray with a sphere: none, one (tangent) or two points.
pub fn sphere_intersections(
    origin: &Vec3,
    dir: &Vec3,
    centre: &Vec3,
    radius: f32,
) -> [Option<Vec3>; 2] {
    let offset = Vec3::diff(origin, centre);
    let c_term = offset.dot(&offset) - radius * radius;
    let b_term = dir.dot(&offset);
    let discriminant = b_term * b_term - c_term;
    if discriminant < 0.0 {
        [None, None]
    } else if discriminant >= 9.765625E-4 {
        let root = f64::from(discriminant).sqrt() as f32;
        let mut a = *origin;
        a.add(&Vec3::scaled(dir, -b_term - root));
        let mut b = *origin;
        b.add(&Vec3::scaled(dir, -b_term + root));
        [Some(a), Some(b)]
    } else {
        let mut a = *origin;
        a.add(&Vec3::scaled(dir, -b_term));
        [Some(a), None]
    }
}

/// The eye owner, by position mode.
#[derive(Clone, Debug, PartialEq)]
pub enum Position {
    Point(PointPosition),
    Entity(EntityPosition),
    Spline(SplineTrack),
}

impl Position {
    fn for_mode(mode: i32) -> Self {
        match mode {
            MODE_POINT => Self::Point(PointPosition::default()),
            MODE_ENTITY => Self::Entity(EntityPosition::default()),
            2 => Self::Spline(SplineTrack::new(CameraSplineKind::Accelerated)),
            3 => Self::Spline(SplineTrack::new(CameraSplineKind::Timed)),
            4 => Self::Spline(SplineTrack::new(CameraSplineKind::Linear)),
            // Position modes are 0-4; both callers filter first.
            m => unreachable!("position mode {m} is filtered before owner creation"),
        }
    }
    /// Initialised once a target was seen.
    pub fn initialised(&self) -> bool {
        match self {
            Self::Point(p) => !p.current.x.is_nan(),
            Self::Entity(p) => !p.current.x.is_nan(),
            Self::Spline(p) => p.initialised(),
        }
    }
    /// The camera eye point.
    pub fn point(&self) -> Result<Vec3, String> {
        match self {
            Self::Point(p) => Ok(p.current),
            Self::Entity(p) => Ok(p.point()),
            Self::Spline(p) => p.point(),
        }
    }
    /// Level and fine coordinate `(level, [x, y, z])`, truncated to integers.
    pub fn coord(&self) -> Result<(i32, [i32; 3]), String> {
        let level = match self {
            Self::Point(p) => p.level,
            Self::Entity(p) => p.level,
            Self::Spline(_) => 0,
        };
        let v = self.point()?;
        Ok((level, [v.x as i32, v.y as i32, v.z as i32]))
    }
    /// Advances the eye one step.
    fn update(
        &mut self,
        dt: f32,
        rotation: Quat,
        mv: &Movement,
        interpolation: f32,
        collision: [bool; 2],
        scene: &Scene,
    ) -> Result<(), String> {
        match self {
            Self::Point(p) => {
                if p.collision {
                    p.collide(scene, collision)?;
                }
                mv.step(dt, &mut p.current, rotation, &p.target, &mut p.velocity);
                Ok(())
            }
            Self::Entity(p) => {
                // The original client keeps the entity object across cycles and re-resolves it
                // only on refresh; the snapshot resolves every cycle, so a vanished entity
                // skips like a null one.
                let Some(t) = p.trackable.and_then(|r| scene.trackable(r)) else {
                    return Ok(());
                };
                p.collide(&t, scene, collision)?;
                let next_rotation = p.target_rotation(&t);
                p.smoothed.blend(&next_rotation, interpolation);
                if p.smoothed.w.is_nan() {
                    p.smoothed = next_rotation;
                }
                p.target = t.coord_vec();
                mv.step(dt, &mut p.current, rotation, &p.target, &mut p.velocity);
                Ok(())
            }
            Self::Spline(p) => p.update(
                dt,
                [mv.max_speed.x, mv.max_speed.y, mv.max_speed.z],
                [mv.accel.x, mv.accel.y, mv.accel.z],
            ),
        }
    }
}

/// What the frame reads from a ready camera.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(test), allow(dead_code, reason = "exercised by tests only"))]
pub struct View {
    /// Level and fine world coordinate of the eye (y up).
    pub level: i32,
    pub coord: [i32; 3],
    /// Pitch and yaw in radians.
    pub pitch: f32,
    pub yaw: f32,
    /// Horizontal field of view in radians.
    pub fov: f32,
}

// ---------------------------------------------------------------------------
// Camera state.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// A shake; modes 3-5 take radians. `phase` advances by `duration * dt` on
    /// every update.
    Shake {
        mode: i32,
        magnitude: f32,
        duration: f32,
        phase: f32,
    },
    /// A roll tilt in radians.
    Tilt(f32),
}

/// The scripted camera: mode owners, movement settings, effects, the scene
/// snapshot and the classic pose state.
#[derive(Clone, Debug, PartialEq)]
pub struct Cam2 {
    pub control_mode: i32,
    pub projection_mode: i32,
    pub projection_scale: i32,
    pub lookat_mode: Option<i32>,
    pub position_mode: Option<i32>,
    /// The focus and eye owners, created by the mode setters.
    pub lookat: Option<Focus>,
    pub position: Option<Position>,
    /// Linear movement mode; 0 and 1 are acceleration modes, 2 is the spring mode.
    pub linear_movement_mode: i32,
    pub position_angular_interpolation: f32,
    pub lookat_max_speed: [f32; 3],
    pub position_max_speed: [f32; 3],
    pub lookat_acceleration: [f32; 3],
    pub position_acceleration: [f32; 3],
    /// Snap distances: far (all laws), near (acceleration laws), near (spring law).
    pub snap: [f32; 3],
    pub lookat_spring: [f32; 3],
    pub position_spring: [f32; 3],
    /// Spring damping of the focus and the eye.
    pub lookat_spring_damping: f32,
    pub position_spring_damping: f32,
    /// Near and far depth planes.
    pub depth_planes: [f32; 2],
    /// Horizontal and vertical field of view in radians.
    pub fov: [f32; 2],
    /// The two collision flags set by the collision-mode command.
    pub collision: [bool; 2],
    /// Trail distance and trail scale.
    pub trail: (i32, f32),
    pub effects: Vec<(i32, Effect)>,
    pub effect_id_counter: i32,
    /// Set when a script changed the camera; true by default.
    pub changed: bool,
    /// Camera state: 1 smooth reset, 2 follow player, 3 scripted camera, 4 follow
    /// coordinate, 5 cutscene, 6 move-along.
    pub camera_state: i32,
    /// Time of the last scripted step, milliseconds.
    pub last_camera_update: i64,
    /// The per-cycle scene snapshot (`Cam2::sync_scene`).
    pub scene: Scene,
    pub legacy: LegacyCamera,
    /// The default camera state: 3 with the scripted-camera default, else 2.
    pub default_state: i32,
}

impl Cam2 {
    /// A client-owned camera at its defaults, in the default camera state.
    pub fn new(cam2_default: bool) -> Self {
        Self {
            control_mode: CLIENT,
            projection_mode: 0,
            projection_scale: 5,
            lookat_mode: None,
            position_mode: None,
            lookat: None,
            position: None,
            linear_movement_mode: 1,
            position_angular_interpolation: 0.05,
            lookat_max_speed: [100.0; 3],
            position_max_speed: [100.0; 3],
            lookat_acceleration: [f32::INFINITY; 3],
            position_acceleration: [f32::INFINITY; 3],
            snap: [5120.0, 10.0, 1.0],
            lookat_spring: [1.0; 3],
            position_spring: [1.0; 3],
            lookat_spring_damping: 1.1,
            position_spring_damping: 1.1,
            depth_planes: [50.0, 10000.0],
            fov: [1.5707964, 1.5707964],
            collision: [true, true],
            trail: (0, 1.0),
            effects: Vec::new(),
            effect_id_counter: 0,
            changed: true,
            camera_state: if cam2_default { 3 } else { 2 },
            last_camera_update: 0,
            scene: Scene::default(),
            legacy: LegacyCamera::default(),
            default_state: if cam2_default { 3 } else { 2 },
        }
    }
    /// Returns the camera to its client-owned defaults, as the login screen does:
    /// projection, focus and eye modes, speeds, springs, planes, field of view,
    /// collision, trail and effects. The camera state, snap distances, effect id
    /// counter, scene snapshot and classic pose are kept.
    pub fn restore_client_defaults(&mut self) {
        let fresh = Self::new(self.default_state == 3);
        *self = Self {
            snap: self.snap,
            effect_id_counter: self.effect_id_counter,
            camera_state: self.camera_state,
            last_camera_update: self.last_camera_update,
            scene: std::mem::take(&mut self.scene),
            legacy: std::mem::take(&mut self.legacy),
            default_state: self.default_state,
            ..fresh
        };
    }
    /// Whether the client (not the server) controls the camera.
    pub(super) fn client_owned(&self) -> bool {
        self.control_mode == CLIENT
    }
    /// Whether the linear movement mode is one of the acceleration modes.
    pub(super) fn acceleration_mode(&self) -> bool {
        self.linear_movement_mode != 2
    }
    /// Resets the camera state (the roof fields belong to the scene camera owner).
    pub fn camera_reset(&mut self, state: i32) {
        self.legacy.clear();
        self.camera_state = state;
        if self.camera_state != 3 {
            self.last_camera_update = 0;
        }
    }
    /// Starts the 100-cycle smooth-reset transition (camera state 1) from the
    /// current pose. With the scripted-camera default the camera is stepped once
    /// first.
    pub fn camera_smooth_reset(&mut self) {
        self.legacy.clear();
        self.camera_state = 1;
        if self.default_state == 3 {
            let dt = (1000 / LOGIC_RATE) as f32 / 1000.0;
            if let Err(reason) = self.update(dt) {
                crate::logging::warn_repeated!("[client910] cam2 smooth reset update: {reason}");
            }
        }
        self.legacy.transition = Some(LegacyTransition {
            start: self.legacy.loop_cycle,
            from: self.legacy.pose,
            from_zoom: self.legacy.zoom,
        });
    }
    /// Copies the scripted eye and angles into the classic pose before a classic
    /// camera command.
    pub fn copy_to_legacy(&mut self) {
        let Some(eye) = self.eye() else {
            // A missing eye is a missing-value fault.
            crate::logging::warn_repeated!(
                "[client910] copy_to_legacy: missing value (cam2 position not ready)"
            );
            return;
        };
        let p = &mut self.legacy.pose;
        p.x = eye.x as i32 - self.scene.base[0];
        p.y = -(eye.y as i32);
        p.z = eye.z as i32 - self.scene.base[1];
        let pitch = self.pitch();
        let yaw = self.yaw();
        let p = &mut self.legacy.pose;
        p.pitch = (f64::from(pitch) * ANGLE_UNITS_PER_RADIAN) as i32 & 0x3FFF;
        p.yaw = (f64::from(yaw) * ANGLE_UNITS_PER_RADIAN) as i32 & 0x3FFF;
        p.roll = 0;
    }
    /// The smooth-reset transition (camera state 1) at draw time. `orbit` is the
    /// pose the orbit camera produced for the default orbit this frame;
    /// `viewport_width` and `zoom` are the current viewport width and camera
    /// zoom. Returns `false` once the transition completed and the default state
    /// was restored.
    pub fn update_transition(&mut self, orbit: LegacyPose, viewport_width: i32, zoom: i32) -> bool {
        let Some(tr) = self.legacy.transition else {
            self.camera_state = self.default_state;
            return false;
        };
        let elapsed = self.legacy.loop_cycle.wrapping_sub(tr.start);
        if elapsed >= 100 {
            self.camera_state = self.default_state;
            self.legacy.transition = None;
            return false;
        }
        let eased =
            1.0 - ((100 - elapsed) * (100 - elapsed) * (100 - elapsed)) as f32 / 1_000_000.0;
        let f = tr.from;
        let mut z = zoom;
        let yaw_delta;
        if self.default_state == 3 {
            let coord = self
                .position
                .as_ref()
                .and_then(|p| p.coord().ok())
                .map_or([0; 3], |(_, c)| c);
            let pitch = self.pitch();
            let yaw = self.yaw();
            let fov = self.fov[0];
            let base = self.scene.base;
            let p = &mut self.legacy.pose;
            p.pitch = (f64::from(pitch) * ANGLE_UNITS_PER_RADIAN) as i32 & 0x3FFF;
            p.yaw = (f64::from(yaw) * -ANGLE_UNITS_PER_RADIAN) as i32 & 0x3FFF;
            p.roll = 0;
            z = (tr.from_zoom as f32
                + ((f64::from(viewport_width) / ((f64::from(fov / 2.0)).tan() * 4.0)) as i32
                    - tr.from_zoom) as f32
                    * eased) as i32;
            p.x = ((coord[0] - base[0] - f.x) as f32 * eased + f.x as f32) as i32;
            p.y = ((-coord[1] - f.y) as f32 * eased + f.y as f32) as i32;
            p.z = ((coord[2] - base[1] - f.z) as f32 * eased + f.z as f32) as i32;
            let mut v = (-p.yaw - f.yaw) & 0x3FFF;
            if v > 8192 {
                v -= 16384;
            } else if v < -8192 {
                v += 16384;
            }
            yaw_delta = v;
        } else {
            let p = &mut self.legacy.pose;
            p.x = ((orbit.x - f.x) as f32 * eased + f.x as f32) as i32;
            p.y = ((orbit.y - f.y) as f32 * eased + f.y as f32) as i32;
            p.z = ((orbit.z - f.z) as f32 * eased + f.z as f32) as i32;
            p.pitch = ((orbit.pitch - f.pitch) as f32 * eased + f.pitch as f32) as i32;
            p.yaw = orbit.yaw;
            p.roll = orbit.roll;
            let mut v = p.yaw - f.yaw;
            if v > 8192 {
                v -= 16384;
            } else if v < -8192 {
                v += 16384;
            }
            yaw_delta = v;
        }
        let p = &mut self.legacy.pose;
        p.yaw = (yaw_delta as f32 * eased + f.yaw as f32) as i32;
        p.yaw &= 0x3FFF;
        self.legacy.zoom = ((z - tr.from_zoom) as f32 * eased + tr.from_zoom as f32) as i32;
        true
    }
    /// The movement settings for the focus or the eye.
    pub(super) fn movement(&self, is_position: bool) -> Movement {
        Movement {
            mode: self.linear_movement_mode,
            accel: Vec3::from_array(if is_position {
                self.position_acceleration
            } else {
                self.lookat_acceleration
            }),
            max_speed: Vec3::from_array(if is_position {
                self.position_max_speed
            } else {
                self.lookat_max_speed
            }),
            trail: self.trail,
            spring: Vec3::from_array(if is_position {
                self.position_spring
            } else {
                self.lookat_spring
            }),
            damping: if is_position {
                self.position_spring_damping
            } else {
                self.lookat_spring_damping
            },
            snap: self.snap,
        }
    }
    /// The eye point once the position is initialised.
    pub fn eye(&self) -> Option<Vec3> {
        self.position
            .as_ref()
            .filter(|p| p.initialised())
            .and_then(|p| p.point().ok())
    }
    /// The looked-at point once the focus is initialised.
    pub fn lookat_point(&self) -> Option<Vec3> {
        let eye = self.eye()?;
        self.lookat
            .as_ref()
            .filter(|l| l.initialised())
            .and_then(|l| match l {
                Focus::Orientation(o) => Some(o.point(eye)),
                _ => l.point(&self.scene).ok(),
            })
    }
    /// Pitch in radians (0 until both ends exist).
    pub fn pitch(&self) -> f32 {
        let (Some(eye_point), Some(focus_point)) = (self.eye(), self.lookat_point()) else {
            return 0.0;
        };
        let delta = Vec3::diff(&focus_point, &eye_point);
        let horizontal = f64::from(delta.z * delta.z + delta.x * delta.x).sqrt() as f32;
        f64::from(-delta.y).atan2(f64::from(horizontal)) as f32
    }
    /// Yaw in radians: `π - atan2(dx, dz)` of eye minus focus.
    pub fn yaw(&self) -> f32 {
        let mut heading = 0.0_f32;
        if let (Some(eye_point), Some(focus_point)) = (self.eye(), self.lookat_point()) {
            let mut delta = Vec3::diff(&eye_point, &focus_point);
            delta.y = 0.0;
            heading = f64::from(delta.x).atan2(f64::from(delta.z)) as f32;
        }
        (std::f64::consts::PI - f64::from(heading)) as f32
    }
    /// The camera rotation from yaw, pitch and zero roll (the orientation itself
    /// for an orientation focus).
    pub fn rotation(&self) -> Quat {
        if let Some(Focus::Orientation(o)) = self.lookat.as_ref() {
            return o.current;
        }
        let mut quat = Quat::IDENTITY;
        quat.set_to_rotation_ypr(self.yaw(), self.pitch(), 0.0);
        quat
    }
    /// Both owners exist and are initialised.
    pub fn ready(&self) -> bool {
        match (&self.lookat, &self.position) {
            (Some(Focus::CoupledSpline(l)), Some(Position::Spline(p))) => {
                l.initialised() && p.initialised()
            }
            (Some(l), Some(p)) => l.initialised() && p.initialised(),
            _ => false,
        }
    }
    /// The frame view; only when the camera is ready.
    #[cfg_attr(not(test), allow(dead_code, reason = "exercised by tests only"))]
    pub fn view(&self) -> Option<View> {
        if !self.ready() {
            return None;
        }
        let (level, coord) = self.position.as_ref()?.coord().ok()?;
        Some(View {
            level,
            coord,
            pitch: self.pitch(),
            yaw: self.yaw(),
            fov: self.fov[0],
        })
    }
    /// The frame for the renderer: `None` while the camera is not ready (the scene
    /// then fills black). Points are absolute world units with `y` negated; the
    /// caller rebases them onto the renderer's frame origin (minus the scene
    /// base).
    pub fn frame(&self) -> Option<crate::camera::Cam2Frame> {
        if !self.ready() {
            return None;
        }
        let eye = self.eye()?;
        let lookat = self.lookat_point()?;
        let effects = self
            .effects
            .iter()
            .map(|(_, e)| match *e {
                // Shake offset: magnitude * sin(phase).
                Effect::Shake {
                    mode,
                    magnitude,
                    phase,
                    ..
                } => crate::camera::FrameEffect::Shake {
                    mode,
                    offset: magnitude * f64::from(phase).sin() as f32,
                },
                Effect::Tilt(angle) => crate::camera::FrameEffect::Tilt(angle),
            })
            .collect();
        Some(crate::camera::Cam2Frame {
            eye: [eye.x, -eye.y, eye.z],
            lookat: [lookat.x, -lookat.y, lookat.z],
            pitch: self.pitch(),
            yaw: self.yaw(),
            near: self.depth_planes[0],
            far: self.depth_planes[1],
            fov: self.fov,
            projection_mode: self.projection_mode,
            projection_scale: self.projection_scale,
            effects,
        })
    }
    /// Copy this cycle's scene inputs; the heightmap is copied only when the
    /// installed terrain changed (the original client reads the grid in place).
    pub fn sync_scene(&mut self, input: &SceneInput<'_>) {
        self.scene.base = input.base;
        self.scene.local_player = input.local_player;
        self.scene.npcs.clear();
        if let Some(npcs) = input.npcs {
            self.scene
                .npcs
                .extend(npcs.entities.iter().map(|(&index, npc)| {
                    (
                        index as i32,
                        Trackable {
                            kind: TRACKABLE_NPC,
                            index: index as i32,
                            level: npc.path.level,
                            coord: [
                                (npc.path.fine_x as i32).wrapping_add(input.base[0]),
                                -(npc.path.motion.y as i32),
                                (npc.path.fine_z as i32).wrapping_add(input.base[1]),
                            ],
                            yaw: npc.path.angle,
                        },
                    )
                }));
        }
        self.scene.local_size = input.local_size;
        match input.terrain {
            None => self.scene.heightmap = None,
            Some(t) => {
                if self
                    .scene
                    .heightmap
                    .as_ref()
                    .is_none_or(|h| h.generation != input.terrain_generation)
                {
                    self.scene.heightmap =
                        Some(Heightmap::from_terrain(t, input.terrain_generation));
                }
            }
        }
    }
    pub(super) fn sync_coupled_lookat(&mut self) {
        let Some(parameter) = self.position.as_ref().and_then(|position| match position {
            Position::Spline(spline) => Some(spline.parameter()),
            _ => None,
        }) else {
            return;
        };
        if let Some(Focus::CoupledSpline(lookat)) = self.lookat.as_mut() {
            lookat.parameter = parameter;
        }
    }
    /// Advances the focus, the eye and the effects by `dt` seconds.
    pub fn update(&mut self, dt: f32) -> Result<(), String> {
        if matches!(self.lookat.as_ref(), Some(Focus::Orientation(_))) {
            let rotation = self.rotation();
            if let Some(Focus::Orientation(orientation)) = self.lookat.as_mut() {
                orientation.update(rotation, self.position.as_mut());
            }
        } else if self.lookat.is_some() {
            let rotation = self.rotation();
            let mv = self.movement(false);
            if let Some(l) = self.lookat.as_mut() {
                l.update(dt, rotation, &mv, &self.scene)?;
            }
        }
        if self.position.is_some() {
            let rotation = self.rotation();
            let mv = self.movement(true);
            let (interpolation, collision) = (self.position_angular_interpolation, self.collision);
            if let Some(p) = self.position.as_mut() {
                p.update(dt, rotation, &mv, interpolation, collision, &self.scene)?;
            }
            self.sync_coupled_lookat();
        }
        for (_, effect) in &mut self.effects {
            // A shake advances its phase; a tilt has no per-update state.
            if let Effect::Shake {
                duration, phase, ..
            } = effect
            {
                *phase += *duration * dt;
            }
        }
        Ok(())
    }
    /// Once per logic cycle while the camera state is 3 (and no staff free camera
    /// is active), steps the camera in chunks of `1000 / logic rate * 1.25` ms.
    /// `now` is the monotonic clock, sampled exactly where the original client
    /// samples it.
    pub fn step(&mut self, now: &mut dyn FnMut() -> i64) -> Result<(), String> {
        if self.camera_state != 3 {
            return Ok(());
        }
        if crate::ui_debug_flags::flags().ui_cam2_trace {
            static STEPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = STEPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n.is_multiple_of(25) || (n < 200 && self.ready()) {
                let lookat = match &self.lookat {
                    Some(Focus::Entity(l)) => Some((l.trackable, l.current)),
                    _ => None,
                };
                log::info!("[trace] cam2 step#{n} ready={} player={:?} lookat={lookat:?} heightmap={} base={:?}", self.ready(), self.scene.local_player.map(|p| (p.index, p.coord)), self.scene.heightmap.is_some(), self.scene.base);
            }
        }
        if self.last_camera_update <= 0 {
            self.last_camera_update = now();
        }
        let mut remaining_ms = (now() - self.last_camera_update) as f32;
        let tick_ms = 1000 / LOGIC_RATE;
        let chunk_ms = (f64::from(tick_ms) * 1.25) as i32;
        while remaining_ms > 0.0 {
            let chunk = remaining_ms.min(chunk_ms as f32);
            self.update(chunk / 1000.0)?;
            remaining_ms -= chunk_ms as f32;
        }
        self.last_camera_update = now();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
