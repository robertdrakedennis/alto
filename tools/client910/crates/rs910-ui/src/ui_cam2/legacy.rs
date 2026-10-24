//! Classic camera poses, cutscene movement, shake and reset transitions.

/// Legacy camera packets still drive the ordinary scene during server
/// cutscenes. They share the retained camera owner with CAM2 so the packet
/// path reaches the renderer instead of being dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyMove {
    pub x: i32,
    pub z: i32,
    pub source_height: i32,
    pub acceleration: i32,
    pub speed: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyLook {
    pub x: i32,
    pub z: i32,
    pub height: i32,
    pub acceleration: i32,
    pub speed: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyModifier {
    pub enabled: bool,
    pub jitter: i32,
    pub wobble_scale: i32,
    pub cycle: i32,
    pub wobble_speed: i32,
}

/// The classic camera pose in scene-local fine units (y down). Written by the
/// cutscene step (state 5), the move-along step (6), the copy from the
/// scripted camera, forced angles, the smooth-reset transition (1) and by the
/// orbit camera on every state 2/4 draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyPose {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub pitch: i32,
    pub yaw: i32,
    pub roll: i32,
}

/// The smooth-reset transition: the start loop cycle, the start pose and the
/// start zoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyTransition {
    pub start: i32,
    pub from: LegacyPose,
    pub from_zoom: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LegacyCamera {
    pub force_angles: Option<(i32, i32)>,
    pub move_to: Option<LegacyMove>,
    pub look_at: Option<LegacyLook>,
    pub modifiers: [LegacyModifier; 5],
    /// Fine scene-local coordinate orbited by camera state 4 (`cam_followcoord`).
    pub follow_coord: Option<[i32; 2]>,
    /// Position and target spline selection, keyframes and progress driven by
    /// camera state 6 (`cam_movealong`). A camera reset or smooth reset clears the
    /// spline selection.
    pub move_along: Option<LegacyMoveAlong>,
    /// The pose last written by the move-along step, scene-local fine units.
    pub move_along_view: Option<LegacyView>,
    /// The classic camera pose (never cleared by a camera reset).
    pub pose: LegacyPose,
    /// Zoom as last drawn.
    pub zoom: i32,
    /// `cameraState == 1` smooth-reset transition.
    pub transition: Option<LegacyTransition>,
    /// Loop cycle of the current logic update.
    pub loop_cycle: i32,
}

/// Cutscene spline selection copied by `cam_movealong`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyMoveAlong {
    pub pos_spline: usize,
    pub pos_keyframe: usize,
    pub target_spline: usize,
    pub target_keyframe: usize,
    pub progress: i32,
    pub speed_min: i32,
    pub speed_max: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyView {
    pub position: [i32; 3],
    /// The target-spline point the camera looks at (same space).
    pub target: [i32; 3],
    pub pitch: i32,
    pub yaw: i32,
    pub roll: i32,
}

/// A uniform random value in `[0, 1)` for the camera modifier jitter: a
/// process-wide 48-bit linear congruential generator seeded from the clock.
pub fn random_unit() -> f64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0);
    let mut seed = SEED.load(Ordering::Relaxed);
    if seed == 0 {
        seed = (crate::logic_clock::system_time()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64)
            ^ 0x5DEECE66D)
            & ((1 << 48) - 1);
    }
    let mut next = |bits: u32| {
        seed = seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB) & ((1 << 48) - 1);
        (seed >> (48 - bits)) as i64
    };
    let value = ((next(26) << 27) + next(27)) as f64 * (1.0 / (1u64 << 53) as f64);
    SEED.store(seed.max(1), Ordering::Relaxed);
    value
}

/// Angle units (16384 per turn) per radian, for the classic camera angles.
pub(super) const ANGLE_UNITS_PER_RADIAN: f64 = 2607.5945876176133;

impl LegacyCamera {
    /// The shared part of a camera reset and a smooth reset: modifiers off, spline
    /// selection cleared and the look-at rotate speed and acceleration zeroed. The
    /// pose, move/look targets and follow coordinate survive.
    pub fn clear(&mut self) {
        for m in &mut self.modifiers {
            m.enabled = false;
        }
        self.move_along = None;
        self.move_along_view = None;
        self.force_angles = None;
        if let Some(look) = self.look_at.as_mut() {
            look.acceleration = 0;
            look.speed = 0;
        }
        self.transition = None;
    }
    pub fn force_angle(&mut self, pitch: i32, yaw: i32) {
        self.force_angles = Some((pitch, yaw));
    }
    pub fn move_to(&mut self, value: LegacyMove) {
        self.move_to = Some(value);
    }
    pub fn look_at(&mut self, value: LegacyLook) {
        self.look_at = Some(value);
    }
    pub fn shake(&mut self, channel: usize, value: LegacyModifier) {
        if let Some(slot) = self.modifiers.get_mut(channel) {
            *slot = value;
        }
    }
    /// Cutscene step, once per logic update while the camera state is 5.
    /// `height(x, z)` is the terrain height at the current player level.
    pub fn apply_cutscene(&mut self, height: &dyn Fn(i32, i32) -> i32) {
        let m = self.move_to.unwrap_or_default();
        let p = &mut self.pose;
        let dest_x = m.x * 512 + 256;
        let dest_z = m.z * 512 + 256;
        let dest_y = height(dest_x, dest_z) - m.source_height;
        if m.speed >= 100 {
            p.x = m.x * 512 + 256;
            p.z = m.z * 512 + 256;
            p.y = height(p.x, p.z) - m.source_height;
        } else {
            let step = |cur: &mut i32, target: i32| {
                if *cur < target {
                    *cur += m.speed.wrapping_mul(target - *cur) / 1000 + m.acceleration;
                    if *cur > target {
                        *cur = target;
                    }
                }
                if *cur > target {
                    *cur -= m.speed.wrapping_mul(*cur - target) / 1000 + m.acceleration;
                    if *cur < target {
                        *cur = target;
                    }
                }
            };
            step(&mut p.x, dest_x);
            step(&mut p.y, dest_y);
            step(&mut p.z, dest_z);
        }
        let l = self.look_at.unwrap_or_default();
        let look_x = l.x * 512 + 256;
        let look_z = l.z * 512 + 256;
        let look_y = height(look_x, look_z) - l.height;
        let dx = look_x - p.x;
        let dy = look_y - p.y;
        let dz = look_z - p.z;
        let horizontal =
            f64::from(dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz))).sqrt() as i32;
        let mut target_pitch =
            ((f64::from(dy).atan2(f64::from(horizontal)) * ANGLE_UNITS_PER_RADIAN) as i32) & 0x3FFF;
        let target_yaw =
            ((f64::from(dx).atan2(f64::from(dz)) * -ANGLE_UNITS_PER_RADIAN) as i32) & 0x3FFF;
        target_pitch = target_pitch.clamp(1024, 3072);
        if p.pitch < target_pitch {
            p.pitch += (((target_pitch - p.pitch) >> 3) * l.speed / 1000 + l.acceleration) << 3;
            if p.pitch > target_pitch {
                p.pitch = target_pitch;
            }
        }
        if p.pitch > target_pitch {
            p.pitch -= (((p.pitch - target_pitch) >> 3) * l.speed / 1000 + l.acceleration) << 3;
            if p.pitch < target_pitch {
                p.pitch = target_pitch;
            }
        }
        let mut yaw_delta = target_yaw - p.yaw;
        if yaw_delta > 8192 {
            yaw_delta -= 16384;
        }
        if yaw_delta < -8192 {
            yaw_delta += 16384;
        }
        let yaw_steps = yaw_delta >> 3;
        if yaw_steps > 0 {
            p.yaw += (l.speed * yaw_steps / 1000 + l.acceleration) << 3;
            p.yaw &= 0x3FFF;
        }
        if yaw_steps < 0 {
            p.yaw -= (l.speed * -yaw_steps / 1000 + l.acceleration) << 3;
            p.yaw &= 0x3FFF;
        }
        let mut remaining_delta = target_yaw - p.yaw;
        if remaining_delta > 8192 {
            remaining_delta -= 16384;
        }
        if remaining_delta < -8192 {
            remaining_delta += 16384;
        }
        if remaining_delta < 0 && yaw_steps > 0 || remaining_delta > 0 && yaw_steps < 0 {
            p.yaw = target_yaw;
        }
        p.roll = 0;
    }
    /// The instant branch of the look-at command (speed 100 or more).
    pub fn look_at_instant(&mut self, height: &dyn Fn(i32, i32) -> i32) {
        let l = self.look_at.unwrap_or_default();
        if l.speed < 100 {
            return;
        }
        let p = &mut self.pose;
        let look_x = l.x * 512 + 256;
        let look_z = l.z * 512 + 256;
        let look_y = height(look_x, look_z) - l.height;
        let dx = look_x - p.x;
        let dy = look_y - p.y;
        let dz = look_z - p.z;
        let horizontal =
            f64::from(dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz))).sqrt() as i32;
        p.pitch =
            ((f64::from(dy).atan2(f64::from(horizontal)) * ANGLE_UNITS_PER_RADIAN) as i32) & 0x3FFF;
        p.yaw = ((f64::from(dx).atan2(f64::from(dz)) * -ANGLE_UNITS_PER_RADIAN) as i32) & 0x3FFF;
        p.roll = 0;
        p.pitch = p.pitch.clamp(1024, 3072);
    }
    /// The modifier jitter/wobble and the scene clamp applied to the drawn copy of
    /// the pose; the stored pose is left untouched. `random` yields values in
    /// `[0, 1)`.
    pub fn drawn(&self, max_tile: [i32; 2], random: &mut dyn FnMut() -> f64) -> LegacyPose {
        let mut p = self.pose;
        for (i, m) in self.modifiers.iter().enumerate() {
            if !m.enabled {
                continue;
            }
            let noise = (random() * f64::from(m.jitter * 2 + 1) - f64::from(m.jitter)
                + (f64::from(m.cycle) / 100.0 * f64::from(m.wobble_speed)).sin()
                    * f64::from(m.wobble_scale)) as i32;
            match i {
                0 => p.x += noise << 2,
                1 => p.y += noise << 2,
                2 => p.z += noise << 2,
                3 => p.yaw = (p.yaw + noise) & 0x3FFF,
                _ => {
                    p.pitch += noise;
                    p.pitch = p.pitch.clamp(1024, 3072);
                }
            }
        }
        p.x = p.x.clamp(0, (max_tile[0] << 9) - 1);
        p.z = p.z.clamp(0, (max_tile[1] << 9) - 1);
        p
    }
}
