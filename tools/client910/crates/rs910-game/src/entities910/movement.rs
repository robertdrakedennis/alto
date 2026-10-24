//! CPU motion routines: route stepping, forced movement, facing and eased
//! angles. No animation playback or graphics types.
use super::Error;
use super::Player;
type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Motion {
    pub y: f32,
    pub speed: i32,
    pub yaw_velocity: i32,
    pub turn_ticks: i32,
    pub roll: [i32; 2],
    pub pitch: [i32; 2],
}
// Moved to rs910-config (Phase 2.6).
pub use rs910_config::types910::movement::*;
/// The animation node's sequence + delay and the sequences' movement priorities. Spot gates
/// are supplied only for actual spot IDs whose effect enables movement blocking.
pub struct Gate {
    pub delayed: bool,
    pub moving_priority: i32,
    pub stationary_priority: i32,
}
pub struct Context<'a> {
    pub bas: &'a Bas,
    pub turn_speed: i32,
    pub smoothing: bool,
    pub special: bool,
    pub main: Option<Gate>,
    pub spots: Vec<Gate>,
}
#[derive(Debug)]
pub struct Step {
    pub state: Player,
    pub speed: i8,
    pub direction: i32,
}
fn div(a: i32, b: i32) -> Result<i32> {
    if b == 0 {
        Err(Error::Invalid("integer division by zero"))
    } else {
        Ok(a.wrapping_div(b))
    }
}
fn turn(e: &mut Player, target: i32, c: &Context) {
    if c.bas.turn_accel == 0 && c.turn_speed == 0 {
        return;
    }
    e.angle &= 16383;
    let d = target.wrapping_sub(e.angle) & 16383;
    e.desired_angle = if d > 8192 {
        e.angle.wrapping_sub(16384 - d)
    } else {
        e.angle.wrapping_add(d)
    }
}
fn snap_turn(e: &mut Player, target: i32, c: &Context) {
    if c.bas.turn_accel != 0 || c.turn_speed != 0 {
        e.desired_angle = target & 16383;
        e.angle = e.desired_angle;
        e.motion.yaw_velocity = 0
    }
}
/// Route movement, including delayed main/spot blocking, smoothing,
/// i32-overflow acceleration and oldest-waypoint consumption. State is atomic.
pub fn advance_route(prior: &Player, c: &Context) -> Result<Step> {
    let mut e = prior.clone();
    if e.route_length > 9 {
        return Err(Error::Invalid("route length"));
    }
    let stationary = |state| {
        Ok(Step {
            state,
            speed: -1,
            direction: 0,
        })
    };
    if e.route_length == 0 {
        e.seq_trigger = 0;
        return stationary(e);
    }
    let blocks = |g: &Gate| {
        if e.steps_remaining > 0 {
            g.moving_priority == 0
        } else {
            g.stationary_priority == 0
        }
    };
    if c.main.as_ref().is_some_and(|g| !g.delayed && blocks(g))
        || c.spots.iter().any(|g| g.delayed && blocks(g))
    {
        e.seq_trigger = e.seq_trigger.wrapping_add(1);
        return stationary(e);
    }
    let x = e.fine_x as i32;
    let z = e.fine_z as i32;
    let tx = e.x[e.route_length - 1]
        .wrapping_mul(512)
        .wrapping_add(e.size.wrapping_mul(256));
    let tz = e.z[e.route_length - 1]
        .wrapping_mul(512)
        .wrapping_add(e.size.wrapping_mul(256));
    let target = if x < tx {
        if z < tz {
            Some(10240)
        } else if z > tz {
            Some(14336)
        } else {
            Some(12288)
        }
    } else if x > tx {
        if z < tz {
            Some(6144)
        } else if z > tz {
            Some(2048)
        } else {
            Some(4096)
        }
    } else if z < tz {
        Some(8192)
    } else if z > tz {
        Some(0)
    } else {
        None
    };
    if let Some(t) = target {
        turn(&mut e, t, c)
    }
    let route_speed = e.speeds[e.route_length - 1];
    let dx = tx.wrapping_sub(x);
    let dz = tz.wrapping_sub(z);
    if !c.special && (!(-1024..=1024).contains(&dx) || !(-1024..=1024).contains(&dz)) {
        e.fine_x = tx as f32;
        e.fine_z = tz as f32;
        let target = e.desired_angle;
        snap_turn(&mut e, target, c);
        e.route_length -= 1;
        if e.steps_remaining > 0 {
            e.steps_remaining -= 1
        }
        return stationary(e);
    }
    let mut speed = 16i32;
    if c.smoothing {
        if e.desired_angle.wrapping_sub(e.angle) != 0 && e.target == -1 && c.turn_speed != 0 {
            speed = 8
        }
        if !c.special && e.route_length > 2 {
            speed = 24
        }
        if !c.special && e.route_length > 3 {
            speed = 32
        }
    } else {
        if !c.special && e.route_length > 1 {
            speed = 24
        }
        if !c.special && e.route_length > 2 {
            speed = 32
        }
    }
    if e.seq_trigger > 0 && e.route_length > 1 {
        speed = 32;
        e.seq_trigger -= 1
    }
    if route_speed == 2 {
        speed <<= 1
    } else if route_speed == 0 {
        speed >>= 1
    }
    let accel = c.bas.walk_speed;
    if accel != -1 {
        let max = speed << 9;
        if e.route_length == 1 {
            let square = e.motion.speed.wrapping_mul(e.motion.speed);
            let ax = if x > tx {
                x.wrapping_sub(tx)
            } else {
                tx.wrapping_sub(x)
            }
            .wrapping_shl(9);
            let az = if z > tz {
                z.wrapping_sub(tz)
            } else {
                tz.wrapping_sub(z)
            }
            .wrapping_shl(9);
            let distance = ax.max(az);
            let stopping = accel.wrapping_mul(2).wrapping_mul(distance);
            if square > stopping {
                e.motion.speed /= 2
            } else if square / 2 > distance {
                e.motion.speed = e.motion.speed.wrapping_sub(accel).max(0)
            } else if e.motion.speed < max {
                e.motion.speed = e.motion.speed.wrapping_add(accel).min(max)
            }
        } else if e.motion.speed < max {
            e.motion.speed = e.motion.speed.wrapping_add(accel).min(max)
        } else if e.motion.speed > 0 {
            e.motion.speed = e.motion.speed.wrapping_sub(accel).max(0)
        }
        speed = (e.motion.speed >> 9).max(1);
    }
    let mut direction = 0;
    let current;
    if x == tx && z == tz {
        current = -1
    } else {
        if x < tx {
            e.fine_x = (e.fine_x + speed as f32).min(tx as f32);
            direction |= 4
        } else if x > tx {
            e.fine_x = (e.fine_x - speed as f32).max(tx as f32);
            direction |= 8
        }
        if z < tz {
            e.fine_z = (e.fine_z + speed as f32).min(tz as f32);
            direction |= 1
        } else if z > tz {
            e.fine_z = (e.fine_z - speed as f32).max(tz as f32);
            direction |= 2
        }
        current = if speed >= 32 { 2 } else { route_speed };
    }
    if e.fine_x as i32 == tx && e.fine_z as i32 == tz {
        e.route_length -= 1;
        if e.steps_remaining > 0 {
            e.steps_remaining -= 1
        }
    }
    Ok(Step {
        state: e,
        speed: current,
        direction,
    })
}
/// Eased angle step, wrapping i32 arithmetic and arithmetic right shifts.
fn eased(angle: &mut i32, velocity: &mut i32, target: i32, accel: i32, max: i32) -> Result<bool> {
    let old = *velocity;
    if *angle == target && old == 0 {
        return Ok(false);
    }
    let accelerating;
    if old == 0 {
        if target > *angle && target <= angle.wrapping_add(accel)
            || target < *angle && target >= angle.wrapping_sub(accel)
        {
            *angle = target;
            return Ok(false);
        }
        accelerating = true;
    } else if old > 0 && target > *angle {
        let distance = div(old.wrapping_mul(old), accel.wrapping_mul(2))?;
        let end = angle.wrapping_add(distance);
        accelerating = end < target && end >= *angle;
    } else if old < 0 && target < *angle {
        let distance = div(old.wrapping_mul(old), accel.wrapping_mul(2))?;
        let end = angle.wrapping_sub(distance);
        accelerating = end > target && end <= *angle;
    } else {
        accelerating = false;
    }
    if accelerating {
        if target > *angle {
            *velocity = velocity.wrapping_add(accel);
            if max != 0 && *velocity > max {
                *velocity = max
            }
        } else {
            *velocity = velocity.wrapping_sub(accel);
            if max != 0 && *velocity < max.wrapping_neg() {
                *velocity = max.wrapping_neg()
            }
        }
        if *velocity != old {
            let distance = div(velocity.wrapping_mul(*velocity), accel.wrapping_mul(2))?;
            if target > *angle && angle.wrapping_add(distance) > target
                || target < *angle && angle.wrapping_sub(distance) < target
            {
                *velocity = old;
            }
        }
    } else if old > 0 {
        *velocity = velocity.wrapping_sub(accel);
        if *velocity < 0 {
            *velocity = 0
        }
    } else {
        *velocity = velocity.wrapping_add(accel);
        if *velocity > 0 {
            *velocity = 0
        }
    }
    *angle = angle.wrapping_add(velocity.wrapping_add(old) >> 1);
    Ok(accelerating)
}
/// Facing and facing steps. Target deltas are the result of an f32
/// subtraction and cast, supplied explicitly; NPC tile conversion is
/// left to the caller. Returns whether an NPC face tile was consumed.
pub fn advance_facing(
    prior: &Player,
    c: &Context,
    target_delta: Option<(i32, i32)>,
    npc_tile_delta: Option<(i32, i32)>,
    is_npc: bool,
) -> Result<(Player, i32, bool)> {
    let mut e = prior.clone();
    if c.turn_speed == 0 {
        return Ok((e, 0, false));
    }
    if let Some((x, z)) = target_delta {
        if x != 0 || z != 0 {
            turn(
                &mut e,
                ((x as f64).atan2(z as f64) * 2607.5945876176133) as i32 & 16383,
                c,
            )
        }
    }
    let mut consumed = false;
    if !is_npc {
        if e.face_override != -1 && (e.route_length == 0 || e.seq_trigger > 0) {
            let a = e.face_override;
            turn(&mut e, a, c);
            e.face_override = -1;
        }
    } else if (e.route_length == 0 || e.seq_trigger > 0) && npc_tile_delta.is_some() {
        let (x, z) = npc_tile_delta.unwrap();
        if x != 0 || z != 0 {
            turn(
                &mut e,
                ((x as f64).atan2(z as f64) * 2607.5945876176133) as i32 & 16383,
                c,
            )
        }
        consumed = true;
    }
    let old = e.angle;
    let b = c.bas;
    let accel = if b.turn_accel == 0 {
        c.turn_speed
    } else {
        b.turn_accel
    };
    let max = if b.turn_accel == 0 {
        c.turn_speed
    } else {
        b.turn_max
    };
    let accelerating = eased(
        &mut e.angle,
        &mut e.motion.yaw_velocity,
        e.desired_angle,
        accel,
        max,
    )?;
    let delta = e.angle.wrapping_sub(old);
    if delta == 0 {
        e.motion.turn_ticks = 0;
        e.angle = e.desired_angle;
        e.motion.yaw_velocity = 0
    } else {
        e.motion.turn_ticks = e.motion.turn_ticks.wrapping_add(1)
    }
    if accelerating {
        if b.roll_accel != 0 {
            let [ref mut angle, ref mut velocity] = e.motion.roll;
            eased(
                angle,
                velocity,
                if delta > 0 {
                    b.roll_target
                } else {
                    b.roll_target.wrapping_neg()
                },
                b.roll_accel,
                b.roll_max,
            )?;
        }
        if b.pitch_accel != 0 {
            let [ref mut angle, ref mut velocity] = e.motion.pitch;
            eased(angle, velocity, b.pitch_target, b.pitch_accel, b.pitch_max)?;
        }
    } else {
        for (a, accel, max) in [
            (&mut e.motion.roll, b.roll_accel, b.roll_max),
            (&mut e.motion.pitch, b.pitch_accel, b.pitch_max),
        ] {
            if accel == 0 {
                *a = [0; 2]
            } else {
                let [ref mut angle, ref mut velocity] = *a;
                eased(angle, velocity, 0, accel, max)?;
            }
        }
    }
    Ok((e, delta, consumed))
}
/// Forced movement. The height callback must match the heightmap lookup
/// (bridge and out-of-map behavior belong to the caller).
pub fn advance_forced(
    prior: &Player,
    c: &Context,
    cycle: i32,
    can_advance_main: bool,
    height: impl Fn(i32, i32, i32) -> i32,
) -> Result<Player> {
    let mut e = prior.clone();
    let f = e.forced;
    let sx = f[0]
        .wrapping_mul(512)
        .wrapping_add(e.size.wrapping_mul(256));
    let sz = f[1]
        .wrapping_mul(512)
        .wrapping_add(e.size.wrapping_mul(256));
    if f[6] > cycle {
        let n = f[6].wrapping_sub(cycle);
        let y = height(sx, sz, f[4]);
        e.fine_x = div(sx.wrapping_sub(e.fine_x as i32), n)?.wrapping_add(e.fine_x as i32) as f32;
        e.fine_z = div(sz.wrapping_sub(e.fine_z as i32), n)?.wrapping_add(e.fine_z as i32) as f32;
        e.motion.y =
            div(y.wrapping_sub(e.motion.y as i32), n)?.wrapping_add(e.motion.y as i32) as f32;
        e.seq_trigger = 0;
        turn(&mut e, f[8], c);
    } else if f[7] >= cycle {
        if f[7] == cycle || c.main.is_none() || can_advance_main {
            let n = f[7].wrapping_sub(f[6]);
            let t = cycle.wrapping_sub(f[6]);
            let ex = f[2]
                .wrapping_mul(512)
                .wrapping_add(e.size.wrapping_mul(256));
            let ez = f[3]
                .wrapping_mul(512)
                .wrapping_add(e.size.wrapping_mul(256));
            let mix = |a: i32, b: i32| {
                div(
                    n.wrapping_sub(t)
                        .wrapping_mul(a)
                        .wrapping_add(t.wrapping_mul(b)),
                    n,
                )
            };
            let x = mix(sx, ex)?;
            let z = mix(sz, ez)?;
            e.motion.y = mix(height(x, z, f[4]), height(ex, ez, f[5]))? as f32;
            e.fine_x = x as f32;
            e.fine_z = z as f32;
        }
        e.seq_trigger = 0;
        snap_turn(&mut e, f[8], c);
    } else {
        return Err(Error::Invalid("forced movement not active"));
    }
    Ok(e)
}
