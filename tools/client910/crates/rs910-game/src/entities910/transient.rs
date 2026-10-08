//! Projectile and spot-animation state.
//! Effect pose playback is advanced by the game tick and applied by the app's
//! temporary-scene model owner.
use super::animation_state::Node;
#[derive(Clone, Debug, PartialEq)]
pub struct Projectile {
    pub effect: i32,
    pub level: i32,
    pub offset_start: i32,
    pub offset_end: i32,
    pub start: i32,
    pub end: i32,
    pub pitch: i32,
    pub arc: i32,
    pub source: i32,
    pub target: i32,
    pub slot: i32,
    pub targeted: i32,
    pub follow_ground: bool,
    pub mobile: bool,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub vx: f64,
    pub vz: f64,
    pub speed: f64,
    pub vy: f64,
    pub ay: f64,
    pub animation: Node,
}
impl Projectile {
    pub fn velocity(
        &mut self,
        x: i32,
        z: i32,
        y: i32,
        cycle: i32,
        height: &dyn Fn(i32, i32, i32) -> i32,
    ) {
        let p = &mut self.position;
        if !self.mobile {
            let dx = x as f32 - p[0];
            let dz = z as f32 - p[2];
            let d = ((dx * dx + dz * dz) as f64).sqrt() as f32;
            if d != 0. {
                p[0] += self.arc as f32 * dx / d;
                p[2] += self.arc as f32 * dz / d;
            }
            if self.follow_ground {
                p[1] = height(p[0] as i32, p[2] as i32, self.level).wrapping_sub(self.offset_start)
                    as f32;
            }
        }
        let n = self.end.wrapping_add(1).wrapping_sub(cycle) as f64;
        self.vx = (x as f32 - p[0]) as f64 / n;
        self.vz = (z as f32 - p[2]) as f64 / n;
        self.speed = (self.vx * self.vx + self.vz * self.vz).sqrt();
        if self.pitch == -1 {
            self.vy = (y as f32 - p[1]) as f64 / n
        } else {
            if !self.mobile {
                self.vy = -self.speed * (self.pitch as f64 * 0.02454369).tan();
            }
            self.ay = ((y as f32 - p[1]) as f64 - self.vy * n) * 2. / (n * n);
        }
    }
    /// Physics/rotation only. Frame playback is not advanced by this API.
    pub fn advance_physics(&mut self, ticks: i32, height: &dyn Fn(i32, i32, i32) -> i32) {
        self.mobile = true;
        let t = ticks as f64;
        self.position[0] = (self.position[0] as f64 + t * self.vx) as f32;
        self.position[2] = (self.position[2] as f64 + t * self.vz) as f32;
        self.position[1] = if self.follow_ground {
            height(self.position[0] as i32, self.position[2] as i32, self.level)
                .wrapping_sub(self.offset_start) as f32
        } else if self.pitch == -1 {
            (self.position[1] as f64 + t * self.vy) as f32
        } else {
            let y = (self.position[1] as f64 + self.ay * 0.5 * t * t + t * self.vy) as f32;
            self.vy += t * self.ay;
            y
        };
        let pitch = self.vy.atan2(self.speed) as f32;
        let yaw = self.vx.atan2(self.vz) as f32 - std::f32::consts::PI;
        let ps = ((pitch * 0.5) as f64).sin() as f32;
        let pc = ((pitch * 0.5) as f64).cos() as f32;
        let ys = ((yaw * 0.5) as f64).sin() as f32;
        let yc = ((yaw * 0.5) as f64).cos() as f32;
        // The quaternion multiplication order is kept exactly, with zero products retained.
        let a = [ps, 0. * ps, 0. * ps, pc];
        let b = [0. * ys, ys, 0. * ys, yc];
        self.rotation = [
            a[2] * b[1] + a[0] * b[3] + a[3] * b[0] - a[1] * b[2],
            a[0] * b[2] + a[3] * b[1] + (a[1] * b[3] - a[2] * b[0]),
            a[3] * b[2] + (a[2] * b[3] + a[1] * b[0] - a[0] * b[1]),
            a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
        ];
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    pub key: i64,
    pub effect: i32,
    pub level: i32,
    pub occlude: i32,
    pub position: [f32; 3],
    pub orientation: i32,
    pub targeted: i32,
    pub animation: Option<Node>,
}
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Transients {
    pub projectiles: Vec<Projectile>,
    pub spots: Vec<Spot>,
}
