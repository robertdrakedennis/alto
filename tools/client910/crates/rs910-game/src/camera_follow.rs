//! Classic player-follow camera. Inputs are scene-local.
use crate::protocol910::{
    self,
    terrain::{self, Terrain},
};
#[derive(Clone, Debug, PartialEq)]
pub struct Follow {
    pub x: i32,
    pub z: i32,
    pub pitch: f32,
    pub yaw: f32,
    pub pitch_velocity: f32,
    pub yaw_velocity: f32,
    pub pitch_touched: bool,
    pub yaw_touched: bool,
    pub changed: bool,
    pub pitch_clamp: i32,
    pub height: i32,
    pub offset_x: i32,
    pub offset_z: i32,
    pub offset_yaw: i32,
}
impl Default for Follow {
    fn default() -> Self {
        Self {
            x: 0,
            z: 0,
            pitch: 1088.,
            yaw: 0.,
            pitch_velocity: 0.,
            yaw_velocity: 0.,
            pitch_touched: false,
            yaw_touched: false,
            changed: true,
            pitch_clamp: 0,
            height: 235,
            offset_x: 0,
            offset_z: 0,
            offset_yaw: 0,
        }
    }
}
pub struct World<'a> {
    pub terrain: &'a Terrain,
    pub offsets: Option<&'a [Option<Vec<i8>>; 4]>,
    pub level: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub target: [i32; 3],
    pub pitch: i32,
    pub yaw: i32,
}
impl Follow {
    /// Runs in the game update, before interface camera bindings and redraw.
    pub fn logic(&mut self) {
        if self.pitch_touched {
            self.pitch_touched = false;
        } else {
            self.pitch_velocity /= 2.;
        }
        if self.yaw_touched {
            self.yaw_touched = false;
        } else {
            self.yaw_velocity /= 2.;
        }
    }
    /// A camera increment/decrement request. Repeated requests in one logic step coalesce.
    pub fn input(&mut self, pitch: bool, positive: bool) {
        let (v, touched, limit) = if pitch {
            (&mut self.pitch_velocity, &mut self.pitch_touched, 12.)
        } else {
            (&mut self.yaw_velocity, &mut self.yaw_touched, 24.)
        };
        if !*touched {
            *v += ((if positive { limit } else { -limit }) - *v) / 2.;
            *touched = true;
            self.changed = true;
        }
    }
    /// Normal game rebuild rebase, applied once for each base change.
    pub fn rebase(&mut self, dx: i32, dz: i32) {
        self.x = self.x.wrapping_sub(dx.wrapping_mul(512));
        self.z = self.z.wrapping_sub(dz.wrapping_mul(512));
    }
    pub fn frame(
        &mut self,
        millis: i64,
        player: [f32; 2],
        w: &World,
    ) -> Result<(), protocol910::Error> {
        let tx = self.offset_x.wrapping_add(player[0] as i32);
        let tz = self.offset_z.wrapping_add(player[1] as i32);
        if self.x.wrapping_sub(tx) < -2000
            || self.x.wrapping_sub(tx) > 2000
            || self.z.wrapping_sub(tz) < -2000
            || self.z.wrapping_sub(tz) > 2000
        {
            self.x = tx;
            self.z = tz;
        }
        fn smooth(current: &mut i32, target: i32, millis: i64) {
            if *current == target {
                return;
            }
            let d = target.wrapping_sub(*current);
            let mut step = ((d as i64).wrapping_mul(millis) / 320) as i32;
            if d > 0 {
                if step == 0 {
                    step = 1;
                } else if step > d {
                    step = d;
                }
            } else if step == 0 {
                step = -1;
            } else if step < d {
                step = d;
            }
            *current = current.wrapping_add(step);
        }
        smooth(&mut self.x, tx, millis);
        smooth(&mut self.z, tz, millis);
        self.yaw += millis as f32 * self.yaw_velocity / 6.;
        self.pitch += millis as f32 * self.pitch_velocity / 6.;
        self.clamp(w)
    }
    /// The `cameraState == 4` orbit around the followed coordinate
    /// (`cam_followcoord`). Unlike the player follow there is no anticheat
    /// offset and no far-distance snap.
    pub fn frame_coord(
        &mut self,
        millis: i64,
        coord: [i32; 2],
        w: &World,
    ) -> Result<(), protocol910::Error> {
        fn smooth(current: &mut i32, target: i32, millis: i64) {
            if *current == target {
                return;
            }
            let d = target.wrapping_sub(*current);
            let mut step = ((d as i64).wrapping_mul(millis) / 320) as i32;
            if d > 0 {
                if step == 0 {
                    step = 1;
                } else if step > d {
                    step = d;
                }
            } else if step == 0 {
                step = -1;
            } else if step < d {
                step = d;
            }
            *current = current.wrapping_add(step);
        }
        smooth(&mut self.x, coord[0], millis);
        smooth(&mut self.z, coord[1], millis);
        self.yaw += millis as f32 * self.yaw_velocity / 40. * 8.;
        self.pitch += millis as f32 * self.pitch_velocity / 40. * 8.;
        self.clamp(w)
    }
    /// Scene draw: state 4 reads the height at the followed
    /// coordinate and omits `cameraAnticheatAngle`.
    pub fn view_coord(
        &self,
        coord: [i32; 2],
        w: &World,
        pitch_modifier: Option<i32>,
    ) -> Result<View, protocol910::Error> {
        let mut pitch = (self.pitch as i32).max(self.pitch_clamp >> 8);
        if let Some(wobble) = pitch_modifier {
            pitch = pitch.max(wobble.wrapping_add(128));
        }
        Ok(View {
            target: [
                self.x,
                terrain::height(Some(w.terrain), coord[0], coord[1], w.level)?
                    .wrapping_sub(self.height),
                self.z,
            ],
            pitch,
            yaw: self.yaw as i32 & 16383,
        })
    }
    pub fn clamp(&mut self, w: &World) -> Result<(), protocol910::Error> {
        self.pitch = self.pitch.clamp(1077., 2787.);
        while self.yaw >= 16384. {
            self.yaw -= 16384.;
        }
        while self.yaw < 0. {
            self.yaw += 16384.;
        }
        let (x, z) = (self.x >> 9, self.z >> 9);
        let terrain = w.terrain;
        let height = terrain::height(Some(terrain), self.x, self.z, w.level)?;
        let mut delta = 0;
        if x > 3 && z > 3 && x < (terrain.width as i32) - 4 && z < (terrain.height as i32) - 4 {
            for tx in x - 4..=x + 4 {
                for tz in z - 4..=z + 4 {
                    let (tx, tz) = (tx as usize, tz as usize);
                    let mut level = w.level as usize;
                    if level < 3 && terrain.tiles[terrain.tile(1, tx, tz)].flags & 2 != 0 {
                        level += 1;
                    }
                    let offset = w
                        .offsets
                        .and_then(|v| v[level].as_ref())
                        .map_or(0, |v| (v[tx * (terrain.height + 1) + tz] as u8 as i32) * 32);
                    delta = delta.max(height.wrapping_sub(
                        terrain.heights[terrain.point(level, tx, tz)].wrapping_sub(offset),
                    ));
                }
            }
        }
        let target = (delta >> 2).wrapping_mul(1536).clamp(262144, 786432);
        if target > self.pitch_clamp {
            self.pitch_clamp = self
                .pitch_clamp
                .wrapping_add(target.wrapping_sub(self.pitch_clamp) / 24);
        } else if target < self.pitch_clamp {
            self.pitch_clamp = self
                .pitch_clamp
                .wrapping_add(target.wrapping_sub(self.pitch_clamp) / 80);
        }
        Ok(())
    }
    /// drawScene uses player height, rather than the smoothed target's height.
    pub fn view(
        &self,
        player: [f32; 2],
        w: &World,
        pitch_modifier: Option<i32>,
    ) -> Result<View, protocol910::Error> {
        let mut pitch = (self.pitch as i32).max(self.pitch_clamp >> 8);
        if let Some(wobble) = pitch_modifier {
            pitch = pitch.max(wobble.wrapping_add(128));
        }
        Ok(View {
            target: [
                self.x,
                terrain::height(Some(w.terrain), player[0] as i32, player[1] as i32, w.level)?
                    .wrapping_sub(self.height),
                self.z,
            ],
            pitch,
            yaw: self.offset_yaw.wrapping_add(self.yaw as i32) & 16383,
        })
    }
}
