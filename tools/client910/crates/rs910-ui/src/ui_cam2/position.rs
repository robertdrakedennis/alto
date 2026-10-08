//! Point and entity eye owners, including terrain collision and clearance.

pub use crate::cam2_scene::*;

use rs910_core::fault::Fault;

pub use rs910_core::vector_math::*;

use super::{pitch_of, yaw_of, Heightmap, Scene, TrackableRef};

// ---------------------------------------------------------------------------
// Position owners.
// ---------------------------------------------------------------------------

/// An eye at a fixed point.
#[derive(Clone, Debug, PartialEq)]
pub struct PointPosition {
    /// Level of the point.
    pub level: i32,
    /// Current point, target point and velocity.
    pub current: Vec3,
    pub target: Vec3,
    pub velocity: Vec3,
    /// Whether the target is lifted above the terrain.
    pub collision: bool,
}

impl Default for PointPosition {
    fn default() -> Self {
        Self {
            level: 0,
            current: Vec3::NAN,
            target: Vec3::NAN,
            velocity: Vec3::ZERO,
            collision: false,
        }
    }
}

impl PointPosition {
    /// Aims at a coordinate on a level; an uninitialised eye jumps there at once.
    pub fn set(&mut self, level: i32, coord: [i32; 3]) {
        self.target = Vec3::new(coord[0] as f32, coord[1] as f32, coord[2] as f32);
        if self.current.x.is_nan() {
            self.current = self.target;
            self.velocity.reset();
        }
        self.level = level;
    }
    /// Lifts the target above the terrain by 512.
    #[allow(
        clippy::overly_complex_bool_expr,
        reason = "the legacy condition is kept verbatim; it reduces to `!collision[1]`"
    )]
    pub(super) fn collide(&mut self, scene: &Scene, collision: [bool; 2]) -> Result<(), String> {
        if self.current.x.is_nan() || (!collision[0] && !collision[1]) || !collision[1] {
            return Ok(());
        }
        let Some(hm) = &scene.heightmap else {
            return Ok(());
        };
        let tile_x = (self.target.x as i32).wrapping_sub(scene.base[0]) >> 9;
        let tile_z = (self.target.z as i32).wrapping_sub(scene.base[1]) >> 9;
        // Grid dimensions in vertices: tiles + 1.
        let (len_x, len_z) = (hm.width as i32 + 1, hm.height as i32 + 1);
        if tile_x < 0 || tile_z < 0 || tile_x + 1 >= len_x || tile_z + 1 >= len_z {
            return Ok(());
        }
        let mut height_level = self.level;
        if hm.is_link_below(tile_x, tile_z) {
            height_level = self.level + 1;
        }
        let sampled = hm.bilinear(height_level, tile_x, tile_z, self.target.x, self.target.z)?;
        let ground = sampled - 512;
        let deficit = (-ground) as f32 - self.target.y;
        if deficit > 0.0 {
            self.target.y = (-ground) as f32;
        }
        Ok(())
    }
}

/// An eye that follows an entity with an offset and rotation.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityPosition {
    /// The followed entity.
    pub trackable: Option<TrackableRef>,
    /// Offset from the entity, rotation, whether to follow the entity's yaw, and
    /// the shortest travel vector that still counts as a heading.
    pub offset: Vec3,
    pub rotation: Quat,
    pub follow_yaw: bool,
    pub min_distance: i32,
    /// Level of the entity.
    pub level: i32,
    /// Current point, target point and velocity.
    pub current: Vec3,
    pub target: Vec3,
    pub velocity: Vec3,
    /// The angular-interpolated rotation actually applied to the offset.
    pub smoothed: Quat,
}

impl Default for EntityPosition {
    fn default() -> Self {
        Self {
            trackable: None,
            offset: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            follow_yaw: false,
            min_distance: 200,
            level: 0,
            current: Vec3::NAN,
            target: Vec3::NAN,
            velocity: Vec3::ZERO,
            smoothed: Quat::IDENTITY,
        }
    }
}

/// What a script or packet places on an entity-following eye.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityPlacement {
    pub trackable: TrackableRef,
    pub offset: Vec3,
    pub rotation: Quat,
    pub follow_yaw: bool,
    pub min_distance: i32,
}

impl EntityPosition {
    /// Places the eye on an entity and lifts it clear of the terrain.
    pub fn set(
        &mut self,
        placement: EntityPlacement,
        scene: &Scene,
        collision: [bool; 2],
    ) -> Result<(), String> {
        let EntityPlacement {
            trackable,
            offset,
            rotation,
            follow_yaw,
            min_distance,
        } = placement;
        self.trackable = Some(trackable);
        self.offset = offset;
        self.rotation = rotation;
        self.follow_yaw = follow_yaw;
        self.min_distance = min_distance;
        let t = scene
            .trackable(trackable)
            .ok_or_else(|| Fault::MissingValue.message("camera entity trackable"))?;
        self.level = t.level;
        self.collide(&t, scene, collision)
    }
    /// The rotation plus the entity's travel or facing yaw.
    pub(super) fn target_rotation(&self, t: &Trackable) -> Quat {
        let mut result = self.rotation;
        if self.follow_yaw {
            let mut heading = t.create_vector3();
            if heading.length() < self.min_distance as f32 {
                heading = Vec3::new(0.0, 0.0, 1.0);
                heading.rotate(&t.orientation());
            }
            heading.y = 0.0;
            let yaw = f64::from(heading.x).atan2(f64::from(heading.z)) as f32;
            let yaw_rotation = Quat::rotation(0.0, 1.0, 0.0, yaw);
            result.multiply(&yaw_rotation);
        }
        result
    }
    /// The target plus the offset rotated by `q`.
    pub(super) fn offset_point(&self, q: &Quat) -> Vec3 {
        let mut rotated = self.offset;
        rotated.rotate(q);
        Vec3::sum(&self.target, &rotated)
    }
    /// Height clearance of the offset point over the terrain (NaN outside the
    /// scene).
    pub(super) fn clearance(
        &self,
        t: &Trackable,
        scene: &Scene,
        hm: &Heightmap,
        level: i32,
        use_link: bool,
    ) -> Result<f32, String> {
        let rotation = self.target_rotation(t);
        let point = self.offset_point(&rotation);
        let tile_x = (point.x as i32).wrapping_sub(scene.base[0]) >> 9;
        let tile_z = (point.z as i32).wrapping_sub(scene.base[1]) >> 9;
        // Grid dimensions in vertices: tiles + 1.
        let (len_x, len_z) = (hm.width as i32 + 1, hm.height as i32 + 1);
        if tile_x < 0 || tile_z < 0 || tile_x + 1 >= len_x || tile_z + 1 >= len_z {
            return Ok(f32::NAN);
        }
        let mut sample_level = level;
        if use_link && hm.is_link_below(tile_x, tile_z) {
            sample_level = level + 1;
        }
        let sampled = hm.bilinear(sample_level, tile_x, tile_z, point.x, point.z)?;
        let floor_offset = sampled - 1024;
        Ok((-floor_offset) as f32 - point.y)
    }
    /// Pitches the offset up (ten bisection steps) until the camera point clears
    /// the terrain.
    pub(super) fn collide(
        &mut self,
        t: &Trackable,
        scene: &Scene,
        collision: [bool; 2],
    ) -> Result<(), String> {
        if self.current.x.is_nan() || (!collision[0] && !collision[1]) {
            return Ok(());
        }
        self.level = t.level;
        let mut sample_level = self.level;
        let mut use_link = true;
        if self.level == 3 {
            use_link = false;
        } else if let Some(hm) = &scene.heightmap {
            if hm.is_link_below(
                t.coord[0].wrapping_sub(scene.base[0]) >> 9,
                t.coord[2].wrapping_sub(scene.base[1]) >> 9,
            ) {
                sample_level = self.level + 1;
                use_link = false;
            }
        }
        if !collision[1] {
            return Ok(());
        }
        let Some(hm) = &scene.heightmap else {
            return Ok(());
        };
        let mut margin = self.clearance(t, scene, hm, sample_level, use_link)?;
        if margin.is_nan() || margin <= 0.0 {
            return Ok(());
        }
        // `3.1415927f32` is bit-identical to `f32::consts::PI`.
        let mut upper = std::f32::consts::PI;
        let mut forward = Vec3::new(0.0, 0.0, 1.0);
        forward.rotate(&self.rotation);
        let mut flat = Vec3::new(forward.x, 0.0, forward.z);
        flat.normalise();
        let mut lower = forward.dot(&flat);
        for _ in 0..10 {
            let half_gap = (upper + lower) / 2.0 - lower;
            let mut pitch_step = half_gap;
            if margin > 0.0 {
                pitch_step = -half_gap;
            }
            let mut right = Vec3::new(1.0, 0.0, 0.0);
            right.rotate(&self.rotation);
            let tilt = Quat::rotation_axis(&right, pitch_step);
            self.rotation.multiply(&tilt);
            self.rotation.inverse();
            margin = self.clearance(t, scene, hm, sample_level, use_link)?;
            if margin.is_nan() {
                return Ok(());
            }
            if margin > 0.0 {
                lower += half_gap;
            } else {
                upper -= half_gap;
            }
        }
        Ok(())
    }
    /// The current point plus the offset rotated by the smoothed rotation.
    pub fn point(&self) -> Vec3 {
        let mut rotated = self.offset;
        rotated.rotate(&self.smoothed);
        Vec3::sum(&self.current, &rotated)
    }
    /// Pitch and yaw of the target rotation.
    pub fn pitch(&self) -> f32 {
        pitch_of(&self.rotation)
    }
    pub fn yaw(&self) -> f32 {
        yaw_of(&self.rotation)
    }
}
