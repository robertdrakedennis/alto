//! Point, entity, orientation and spline focus owners.

use rs910_core::fault::Fault;

pub use rs910_core::vector_math::*;

use super::{
    CameraSplineKind, CameraSplinePath, Movement, Position, Scene, SplineTrack, TrackableRef,
    MODE_ENTITY, MODE_ORIENTATION, MODE_POINT,
};

// ---------------------------------------------------------------------------
// Focus owners.
// ---------------------------------------------------------------------------

/// A focus on a fixed point.
#[derive(Clone, Debug, PartialEq)]
pub struct PointFocus {
    /// Current point, target point and velocity.
    pub current: Vec3,
    pub target: Vec3,
    pub velocity: Vec3,
}

impl Default for PointFocus {
    fn default() -> Self {
        Self {
            current: Vec3::NAN,
            target: Vec3::NAN,
            velocity: Vec3::ZERO,
        }
    }
}

impl PointFocus {
    /// Aims at a coordinate; an uninitialised focus jumps there at once.
    pub fn set(&mut self, coord: [i32; 3]) {
        self.target = Vec3::new(coord[0] as f32, coord[1] as f32, coord[2] as f32);
        if self.current.x.is_nan() {
            self.current = self.target;
            self.velocity.reset();
        }
    }
}

/// A focus on an entity plus an offset.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityFocus {
    /// The followed entity.
    pub trackable: Option<TrackableRef>,
    /// Offset from the entity, and whether it turns with the entity's orientation.
    pub offset: Vec3,
    pub rotate_with_entity: bool,
    /// Current point, target point and velocity.
    pub current: Vec3,
    pub target: Vec3,
    pub velocity: Vec3,
}

impl Default for EntityFocus {
    fn default() -> Self {
        Self {
            trackable: None,
            offset: Vec3::ZERO,
            rotate_with_entity: false,
            current: Vec3::NAN,
            target: Vec3::NAN,
            velocity: Vec3::ZERO,
        }
    }
}

impl EntityFocus {
    /// Follows an entity with an offset.
    pub fn set(&mut self, trackable: TrackableRef, offset: Vec3, rotate_with_entity: bool) {
        self.trackable = Some(trackable);
        self.offset = offset;
        self.rotate_with_entity = rotate_with_entity;
    }
    /// The offset, rotated by the entity's orientation when it turns with the
    /// entity. A missing entity is a missing-value fault there.
    pub(super) fn rotated_offset(&self, scene: &Scene) -> Result<Vec3, String> {
        let mut rotated = self.offset;
        if self.rotate_with_entity {
            let t = self
                .trackable
                .and_then(|r| scene.trackable(r))
                .ok_or_else(|| Fault::MissingValue.message("look-at entity trackable"))?;
            rotated.rotate(&t.orientation());
        }
        Ok(rotated)
    }
}

/// A focus given as an orientation. It keeps the orientation quaternion
/// independently of a target point; the camera derives a forward point from
/// it for the renderer and projection helpers.
#[derive(Clone, Debug, PartialEq)]
pub struct OrientationFocus {
    /// The current orientation.
    pub current: Quat,
    /// The last orientation supplied through `set_orientation`.
    pub target: Quat,
    pub rotation_x: i32,
    pub rotation_y: i32,
    pub movement_x: i32,
    pub movement_z: i32,
    /// Clamp bounds (`-1` disables one): min x, max x, min y, max y, min z, max z.
    pub clamps: [f32; 6],
}

impl Default for OrientationFocus {
    fn default() -> Self {
        Self {
            current: Quat::IDENTITY,
            target: Quat::NAN,
            rotation_x: 0,
            rotation_y: 0,
            movement_x: 0,
            movement_z: 0,
            clamps: [-1.0; 6],
        }
    }
}

impl OrientationFocus {
    /// Copies the orientation into the target and makes it current immediately.
    pub(super) fn set_orientation(&mut self, q: Quat) {
        self.target = q;
        self.current = q;
    }

    /// Aims along a direction: builds the camera basis, converts the matrix to a
    /// quaternion and takes its opposite. The conversion keeps the component
    /// order of the original client's math so results stay bit-identical.
    pub(super) fn set_vector(&mut self, x: i32, y: i32, z: i32) {
        let fx = x as f32;
        let fy = -(y as f32);
        let fz = z as f32;
        let forward_len = f64::from(fx * fx + fy * fy + fz * fz).sqrt() as f32;
        if forward_len == 0.0 {
            return;
        }
        let forward = [fx / forward_len, fy / forward_len, fz / forward_len];
        // `up=(0,1,0)` crossed with forward.
        let right_len = f64::from(forward[0] * forward[0] + forward[2] * forward[2]).sqrt() as f32;
        if right_len == 0.0 {
            return;
        }
        let right = [forward[2] / right_len, 0.0, -forward[0] / right_len];
        let up = [
            forward[1] * right[2] - forward[2] * right[1],
            forward[2] * right[0] - forward[0] * right[2],
            forward[0] * right[1] - forward[1] * right[0],
        ];
        let m = [
            right[0], up[0], forward[0], right[1], up[1], forward[1], right[2], up[2], forward[2],
        ];
        let trace = f64::from(m[0] + 1.0 + m[4] + m[8]);
        let mut q = if trace > 1.0e-8 {
            let scale = trace.sqrt() as f32 * 2.0;
            Quat {
                w: (m[7] - m[5]) / scale,
                x: (m[2] - m[6]) / scale,
                y: (m[3] - m[1]) / scale,
                z: scale * 0.25,
            }
        } else if m[0] > m[4] && m[0] > m[8] {
            let scale = f64::from(m[0] + 1.0 - m[4] - m[8]).sqrt() as f32 * 2.0;
            Quat {
                w: scale * 0.25,
                x: (m[3] + m[1]) / scale,
                y: (m[2] + m[6]) / scale,
                z: (m[7] - m[5]) / scale,
            }
        } else if m[4] > m[8] {
            let scale = f64::from(m[4] + 1.0 - m[0] - m[8]).sqrt() as f32 * 2.0;
            Quat {
                w: (m[3] + m[1]) / scale,
                x: scale * 0.25,
                y: (m[7] + m[5]) / scale,
                z: (m[2] - m[6]) / scale,
            }
        } else {
            let scale = f64::from(m[8] + 1.0 - m[0] - m[4]).sqrt() as f32 * 2.0;
            Quat {
                w: (m[2] + m[6]) / scale,
                x: (m[7] + m[5]) / scale,
                y: scale * 0.25,
                z: (m[3] - m[1]) / scale,
            }
        };
        // The original client takes the opposite after the matrix conversion.
        q.opposite();
        self.set_orientation(q);
    }

    /// Applies queued rotations, then moves a point eye in the camera-local X/Z
    /// axes. The eye owner stays authoritative for entity and spline modes, so
    /// queued movement is dropped there.
    pub(super) fn update(&mut self, rotation: Quat, position: Option<&mut Position>) {
        let mut q = self.current;
        if self.rotation_x != 0 || self.rotation_y != 0 {
            let x = Quat::rotation(
                1.0,
                0.0,
                0.0,
                self.rotation_x as f32 * std::f32::consts::TAU / 16384.0,
            );
            q.multiply(&x);
            let mut axis = Vec3::new(0.0, 1.0, 0.0);
            axis.rotate(&q);
            axis.normalise();
            let y = Quat::rotation_axis(
                &axis,
                self.rotation_y as f32 * std::f32::consts::TAU / 16384.0,
            );
            q.multiply(&y);
            self.set_orientation(q);
            self.rotation_x = 0;
            self.rotation_y = 0;
        }
        if self.movement_x == 0 && self.movement_z == 0 {
            return;
        }
        let Some(Position::Point(point)) = position else {
            self.movement_x = 0;
            self.movement_z = 0;
            return;
        };
        let mut inverse = rotation;
        inverse.opposite();
        let mut forward = Vec3::new(0.0, 0.0, self.movement_z as f32);
        forward.rotate(&inverse);
        forward.y *= -1.0;
        point.target.add(&forward);
        let mut sideways = Vec3::new(self.movement_x as f32, 0.0, 0.0);
        sideways.rotate(&inverse);
        sideways.y *= -1.0;
        point.target.add(&sideways);
        self.clamp(&mut point.target);
        if point.current.x.is_nan() {
            point.current = point.target;
            point.velocity.reset();
        }
        self.movement_x = 0;
        self.movement_z = 0;
    }

    pub(super) fn clamp(&self, point: &mut Vec3) {
        if self.clamps[0] != -1.0 {
            point.x = point.x.max(self.clamps[0]);
        }
        if self.clamps[1] != -1.0 {
            point.x = point.x.min(self.clamps[1]);
        }
        if self.clamps[2] != -1.0 {
            point.y = point.y.max(self.clamps[2]);
        }
        if self.clamps[3] != -1.0 {
            point.y = point.y.min(self.clamps[3]);
        }
        if self.clamps[4] != -1.0 {
            point.z = point.z.max(self.clamps[4]);
        }
        if self.clamps[5] != -1.0 {
            point.z = point.z.min(self.clamps[5]);
        }
    }

    pub(super) fn point(&self, eye: Vec3) -> Vec3 {
        let mut point = Vec3::new(0.0, 0.0, 1000.0);
        let inverse = Quat::opposite_of(&self.current);
        point.rotate(&inverse);
        point.y *= -1.0;
        Vec3::sum(&eye, &point)
    }
}

/// The focus owner, by focus mode.
#[derive(Clone, Debug, PartialEq)]
pub enum Focus {
    Point(PointFocus),
    Entity(EntityFocus),
    Orientation(OrientationFocus),
    Spline(SplineTrack),
    CoupledSpline(CoupledSplineFocus),
}

/// A single look-at spline sampled at the eye spline's current parameter. It
/// does not advance itself; the eye owner supplies the clock for both camera
/// ends.
#[derive(Clone, Debug, PartialEq)]
pub struct CoupledSplineFocus {
    path: Option<CameraSplinePath>,
    pub(super) parameter: f32,
}

impl CoupledSplineFocus {
    pub(super) fn new() -> Self {
        Self {
            path: None,
            parameter: 0.0,
        }
    }
    pub(super) fn decode(
        &mut self,
        reader: &mut crate::ui_bytes::Cursor<'_>,
    ) -> anyhow::Result<()> {
        self.path = Some(CameraSplinePath::decode(reader)?);
        self.parameter = 0.0;
        Ok(())
    }
    pub(super) fn initialised(&self) -> bool {
        self.path.is_some()
    }
    pub(super) fn point(&self) -> Vec3 {
        self.path
            .as_ref()
            .map_or(Vec3::NAN, |path| path.sample_segment(self.parameter))
    }
}

impl Focus {
    pub(super) fn for_mode(mode: i32) -> Self {
        match mode {
            MODE_POINT => Self::Point(PointFocus::default()),
            MODE_ENTITY => Self::Entity(EntityFocus::default()),
            MODE_ORIENTATION => Self::Orientation(OrientationFocus::default()),
            2 => Self::Spline(SplineTrack::new(CameraSplineKind::Accelerated)),
            4 => Self::Spline(SplineTrack::new(CameraSplineKind::Timed)),
            5 => Self::CoupledSpline(CoupledSplineFocus::new()),
            6 => Self::Spline(SplineTrack::new(CameraSplineKind::Linear)),
            // Focus modes are 0-6; both the script setter and the camera update packet
            // filter other ids before creating an owner.
            m => unreachable!("focus mode {m} is filtered before owner creation"),
        }
    }
    /// Initialised once a target was seen.
    pub fn initialised(&self) -> bool {
        match self {
            Self::Point(l) => !l.current.x.is_nan(),
            Self::Entity(l) => !l.current.x.is_nan(),
            Self::Orientation(l) => l.current.is_finite(),
            Self::Spline(p) => p.initialised(),
            Self::CoupledSpline(l) => l.initialised(),
        }
    }
    /// The point looked at.
    pub fn point(&self, scene: &Scene) -> Result<Vec3, String> {
        match self {
            Self::Point(l) => Ok(l.current),
            Self::Entity(l) => Ok(Vec3::sum(&l.current, &l.rotated_offset(scene)?)),
            Self::Orientation(_) => Err("orientation lookat requires the camera eye".into()),
            Self::Spline(p) => p.point(),
            Self::CoupledSpline(l) => Ok(l.point()),
        }
    }
    /// Advances the focus one step.
    pub(super) fn update(
        &mut self,
        dt: f32,
        rotation: Quat,
        mv: &Movement,
        scene: &Scene,
    ) -> Result<(), String> {
        match self {
            Self::Point(l) => {
                mv.step(dt, &mut l.current, rotation, &l.target, &mut l.velocity);
                Ok(())
            }
            Self::Entity(l) => {
                if let Some(t) = l.trackable.and_then(|r| scene.trackable(r)) {
                    l.target = t.coord_vec();
                    mv.step(dt, &mut l.current, rotation, &l.target, &mut l.velocity);
                }
                Ok(())
            }
            Self::Orientation(_) => Ok(()),
            Self::Spline(p) => p.update(
                dt,
                [mv.max_speed.x, mv.max_speed.y, mv.max_speed.z],
                [mv.accel.x, mv.accel.y, mv.accel.z],
            ),
            Self::CoupledSpline(_) => Ok(()),
        }
    }
}
