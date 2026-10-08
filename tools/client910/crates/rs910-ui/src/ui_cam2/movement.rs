//! Camera acceleration and spring laws, applied along world or camera axes.

pub use rs910_core::vector_math::*;

// ---------------------------------------------------------------------------
// Linear movement laws: how the eye or the focus point chases its target.
// ---------------------------------------------------------------------------

/// Where one axis stands relative to its target, as the acceleration law
/// sees it.
#[derive(Clone, Copy)]
pub(super) struct AxisInput {
    /// Distance needed to brake to a stop from the current speed.
    stopping: f32,
    /// Remaining distance to the target on this axis.
    remaining: f32,
    /// Signed offset to the target on this axis.
    delta: f32,
    /// Current speed on this axis (absolute value).
    speed: f32,
}

/// Acceleration and speed limit of one axis.
#[derive(Clone, Copy)]
pub(super) struct AxisLimits {
    accel: f32,
    max: f32,
}

/// The inputs of one acceleration step on all three axes, one vector per
/// quantity.
pub(super) struct AxisVectors {
    stopping: Vec3,
    remaining: Vec3,
    delta: Vec3,
    speed: Vec3,
    accel: Vec3,
    max: Vec3,
}

/// Tuning of the two acceleration-based laws.
#[derive(Clone, Copy)]
pub(super) struct AccelerationLaw {
    accel: Vec3,
    max: Vec3,
    /// When positive, the acceleration scales with `distance / trail * trail_scale`.
    trail: f32,
    trail_scale: f32,
    /// Farther than this the point jumps straight onto the target.
    snap_far: f32,
    /// Closer than this the point lands on the target.
    snap_near: f32,
}

/// Tuning of the damped spring law.
#[derive(Clone, Copy)]
pub(super) struct SpringLaw {
    spring: Vec3,
    damping: f32,
    snap_far: f32,
    snap_near: f32,
}

/// One axis of the acceleration law shared by the world-axis and camera-axis
/// laws: brake when the stopping distance exceeds the remaining distance,
/// else accelerate up to the speed limit.
pub(super) fn accelerate_axis(velocity: &mut f32, input: AxisInput, limits: AxisLimits, dt: f32) {
    let AxisInput {
        stopping,
        remaining,
        delta,
        speed,
    } = input;
    let AxisLimits { accel, max } = limits;
    if stopping > remaining {
        if delta < 0.0 {
            *velocity += accel * dt;
            if *velocity > 0.0 {
                *velocity = 0.0;
            }
        } else {
            *velocity -= accel * dt;
            if *velocity < 0.0 {
                *velocity = 0.0;
            }
        }
    } else if speed < max {
        if delta < 0.0 {
            *velocity -= accel * dt;
            if *velocity < -max {
                *velocity = -max;
            }
        } else {
            *velocity += accel * dt;
            if *velocity > max {
                *velocity = max;
            }
        }
    }
}

/// Runs [`accelerate_axis`] on x, then y, then z.
pub(super) fn accelerate_each_axis(velocity: &mut Vec3, axes: &AxisVectors, dt: f32) {
    let input = |pick: fn(&Vec3) -> f32| AxisInput {
        stopping: pick(&axes.stopping),
        remaining: pick(&axes.remaining),
        delta: pick(&axes.delta),
        speed: pick(&axes.speed),
    };
    let limits = |pick: fn(&Vec3) -> f32| AxisLimits {
        accel: pick(&axes.accel),
        max: pick(&axes.max),
    };
    accelerate_axis(&mut velocity.x, input(|v| v.x), limits(|v| v.x), dt);
    accelerate_axis(&mut velocity.y, input(|v| v.y), limits(|v| v.y), dt);
    accelerate_axis(&mut velocity.z, input(|v| v.z), limits(|v| v.z), dt);
}

/// Accelerates on the three world axes independently.
pub(super) fn accelerate_along_world_axes(
    dt: f32,
    current: &mut Vec3,
    target: &Vec3,
    velocity: &mut Vec3,
    law: &AccelerationLaw,
) {
    let AccelerationLaw {
        mut accel,
        max,
        trail,
        trail_scale,
        snap_far,
        snap_near,
    } = *law;
    if target.is_equal_to(current) {
        return;
    }
    let to_target = Vec3::diff(target, current);
    let distance = to_target.length();
    if trail > 0.0 {
        accel.scale(distance / trail * trail_scale);
    }
    if accel.x == f32::INFINITY || current.x.is_nan() || distance > snap_far {
        *current = *target;
        velocity.reset();
        return;
    }
    let mut abs_velocity = *velocity;
    abs_velocity.abs();
    let stop_time = Vec3::quotient(&abs_velocity, &accel);
    let mut stopping = Vec3::product(&abs_velocity, &stop_time);
    stopping.scale(0.5);
    let mut next_velocity = *velocity;
    let mut abs_delta = to_target;
    abs_delta.abs();
    let axes = AxisVectors {
        stopping,
        remaining: abs_delta,
        delta: to_target,
        speed: abs_velocity,
        accel,
        max,
    };
    accelerate_each_axis(&mut next_velocity, &axes, dt);
    velocity.blend(&next_velocity, 0.8);
    if distance < snap_near && velocity.length() < snap_near {
        *current = *target;
        velocity.reset();
    } else {
        current.add(&Vec3::scaled(velocity, dt));
    }
}

/// Accelerates on the camera's own axes (the default law).
pub(super) fn accelerate_along_camera_axes(
    dt: f32,
    current: &mut Vec3,
    mut rot: Quat,
    target: &Vec3,
    velocity: &mut Vec3,
    law: &AccelerationLaw,
) {
    let AccelerationLaw {
        mut accel,
        max,
        trail,
        trail_scale,
        snap_far,
        snap_near,
    } = *law;
    if target.is_equal_to(current) {
        return;
    }
    let origin = Vec3::ZERO;
    let mut local_delta = Vec3::diff(target, current);
    local_delta.rotate(&rot);
    let offset = Vec3::diff(&local_delta, &origin);
    let distance = offset.length();
    if trail > 0.0 {
        accel.scale(distance / trail * trail_scale);
    }
    if accel.x == f32::INFINITY || current.x.is_nan() || distance > snap_far || distance < snap_near
    {
        *current = *target;
        velocity.reset();
        return;
    }
    rot.opposite();
    let mut axis_x = Vec3::new(1.0, 0.0, 0.0);
    let mut axis_y = Vec3::new(0.0, 1.0, 0.0);
    let mut axis_z = Vec3::new(0.0, 0.0, 1.0);
    axis_x.rotate(&rot);
    axis_y.rotate(&rot);
    axis_z.rotate(&rot);
    let local_velocity = Vec3::new(
        axis_x.dot(velocity),
        axis_y.dot(velocity),
        axis_z.dot(velocity),
    );
    let mut abs_velocity = local_velocity;
    abs_velocity.abs();
    let stopping = Vec3::quotient(
        &Vec3::product(&abs_velocity, &abs_velocity),
        &Vec3::scaled(&accel, 2.0),
    );
    let mut abs_offset = offset;
    abs_offset.abs();
    let mut next_local = local_velocity;
    let axes = AxisVectors {
        stopping,
        remaining: abs_offset,
        delta: offset,
        speed: abs_velocity,
        accel,
        max,
    };
    accelerate_each_axis(&mut next_local, &axes, dt);
    let mut next_velocity = Vec3::scaled(&axis_x, next_local.x);
    next_velocity.add_scaled(&axis_y, next_local.y);
    next_velocity.add_scaled(&axis_z, next_local.z);
    velocity.blend(&next_velocity, 0.8);
    current.add(&Vec3::scaled(velocity, dt));
}

/// Damped spring on the camera's own axes.
pub(super) fn spring_along_camera_axes(
    dt: f32,
    current: &mut Vec3,
    mut rot: Quat,
    target: &Vec3,
    velocity: &mut Vec3,
    law: &SpringLaw,
) {
    let SpringLaw {
        spring,
        damping,
        snap_far,
        snap_near,
    } = *law;
    if target.is_equal_to(current) {
        return;
    }
    let origin = Vec3::ZERO;
    let mut local_delta = Vec3::diff(target, current);
    local_delta.rotate(&rot);
    let offset = Vec3::diff(&local_delta, &origin);
    let distance = offset.length();
    if spring.x == f32::INFINITY
        || current.x.is_nan()
        || distance > snap_far
        || distance < snap_near
    {
        *current = *target;
        velocity.reset();
        return;
    }
    rot.opposite();
    let mut axis_x = Vec3::new(1.0, 0.0, 0.0);
    let mut axis_y = Vec3::new(0.0, 1.0, 0.0);
    let mut axis_z = Vec3::new(0.0, 0.0, 1.0);
    axis_x.rotate(&rot);
    axis_y.rotate(&rot);
    axis_z.rotate(&rot);
    let local_velocity = Vec3::new(
        axis_x.dot(velocity),
        axis_y.dot(velocity),
        axis_z.dot(velocity),
    );
    let mut next_local = local_velocity;
    let spring_force = Vec3::new(
        spring.x * offset.x,
        spring.y * offset.y,
        spring.z * offset.z,
    );
    next_local.add_scaled(&spring_force, dt);
    next_local.divide_by(damping);
    let mut world_velocity = Vec3::scaled(&axis_x, next_local.x);
    world_velocity.add_scaled(&axis_y, next_local.y);
    world_velocity.add_scaled(&axis_z, next_local.z);
    *velocity = world_velocity;
    current.add(&Vec3::scaled(velocity, dt));
}

/// The camera settings that steer one target kind (the eye or the focus).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Movement {
    pub mode: i32,
    pub accel: Vec3,
    pub max_speed: Vec3,
    pub trail: (i32, f32),
    pub spring: Vec3,
    pub damping: f32,
    pub snap: [f32; 3],
}

impl Movement {
    pub(super) fn acceleration_law(&self) -> AccelerationLaw {
        AccelerationLaw {
            accel: self.accel,
            max: self.max_speed,
            trail: self.trail.0 as f32,
            trail_scale: self.trail.1,
            snap_far: self.snap[0],
            snap_near: self.snap[1],
        }
    }
    /// The spring law snaps at the first and the third snap distance.
    pub(super) fn spring_law(&self) -> SpringLaw {
        SpringLaw {
            spring: self.spring,
            damping: self.damping,
            snap_far: self.snap[0],
            snap_near: self.snap[2],
        }
    }
    /// Moves `current` one step toward `target` by the linear movement mode:
    /// 0 accelerates on world axes, 1 on camera axes, 2 is the damped spring.
    pub fn step(
        &self,
        dt: f32,
        current: &mut Vec3,
        rotation: Quat,
        target: &Vec3,
        velocity: &mut Vec3,
    ) {
        match self.mode {
            0 => {
                accelerate_along_world_axes(dt, current, target, velocity, &self.acceleration_law())
            }
            1 => accelerate_along_camera_axes(
                dt,
                current,
                rotation,
                target,
                velocity,
                &self.acceleration_law(),
            ),
            2 => spring_along_camera_axes(
                dt,
                current,
                rotation,
                target,
                velocity,
                &self.spring_law(),
            ),
            _ => {}
        }
    }
}
