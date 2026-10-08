//! Positional sound: how a sound's distance and bearing from the listener
//! set its per-channel gains.

/// What the adjusters read: the listener (the local player's position with
/// y = 0) and the camera state and yaw.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AdjustContext {
    pub listener: Option<[f32; 3]>,
    pub camera_state: i32,
    pub cam2_yaw: f32,
    pub camera_yaw: i32,
}

/// How a sound is positioned, selecting its adjuster.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundShape {
    /// Unpositioned.
    None,
    /// A spatialised sound. Nothing in the client builds one; it plays
    /// silence.
    Spatializer,
    /// A position-only sound. Nothing in the client builds one; it plays
    /// silence.
    Position,
    /// Distance attenuation plus stereo panning by bearing.
    Stereo,
    /// Distance attenuation only.
    Volume,
}

/// A sound's adjuster: its shape bound to the sound's position, full-volume
/// distance and range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Adjuster {
    pub shape: SoundShape,
    pub position: [f32; 3],
    pub size: f32,
    pub range: f32,
}

pub(crate) fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// The camera-relative pan of a sound, 0 (hard left) to 16384 (hard right),
/// 8192 when the sound is directly above or below the listener. Sounds
/// closer than 4096 units pan less.
fn pan(delta: [f32; 3], distance: f32, ctx: &AdjustContext) -> i32 {
    let mut pan = 8192;
    if delta[0] != 0.0 || delta[2] != 0.0 {
        let angle = (f64::from(delta[0]).atan2(f64::from(delta[2])) * 2607.5945876176133) as i32;
        let mut relative = if ctx.camera_state == 3 {
            ((f64::from(-ctx.cam2_yaw) * 2607.5945876176133) as i32 - angle - 4096) & 0x3FFF
        } else {
            (-ctx.camera_yaw - angle - 4096) & 0x3FFF
        };
        if relative > 8192 {
            relative = 16384 - relative;
        }
        let spread = if distance <= 0.0 {
            8192
        } else if distance >= 4096.0 {
            16384
        } else {
            (distance * 8192.0 / 4096.0 + 8192.0) as i32
        };
        pan = ((16384 - spread) >> 1) + relative * spread / 8192;
    }
    pan
}

/// Distance attenuation: full volume up to `size`, falling linearly to
/// silence at `range`.
pub fn attenuation(distance: f32, size: f32, range: f32) -> f32 {
    if distance <= size {
        1.0
    } else {
        let mut value = 1.0 - 1.0 / (range - size) * (distance - size);
        if f64::from(value) < 0.0 || f64::from(value) > 1.0 {
            value = value.clamp(0.0, 1.0);
        }
        value
    }
}

impl Adjuster {
    /// Set the channel gains for the sound's position relative to the
    /// listener.
    pub fn apply(&self, gains: &mut [f32; 2], ctx: &AdjustContext) {
        let Some(listener) = ctx.listener else {
            // A positioned sound cannot be created without a listener.
            return;
        };
        let delta = sub(self.position, listener);
        let distance = length(delta);
        match self.shape {
            SoundShape::None => {}
            SoundShape::Stereo => {
                // Equal-power panning scaled by the distance attenuation.
                let volume = attenuation(distance, self.size, self.range);
                let pan = pan(delta, distance, ctx);
                let right = pan as f32 * 6.1035156E-5;
                gains[0] = if pan < 0 {
                    volume
                } else {
                    (f64::from(volume) * f64::from((1.0 - right) * 2.0).sqrt()) as f32
                };
                gains[1] = if pan < 0 {
                    -volume
                } else {
                    (f64::from(volume) * f64::from(right * 2.0).sqrt()) as f32
                };
            }
            SoundShape::Volume => {
                // The same gain on both channels.
                let volume = attenuation(distance, self.size, self.range);
                gains[0] = volume;
                gains[1] = volume;
            }
            SoundShape::Spatializer | SoundShape::Position => {
                // These shapes need extents that no caller provides: they
                // play silence.
                gains[0] = 0.0;
                gains[1] = 0.0;
                if distance < self.range {
                    log::info!(
                        "[client910] audio adjuster {:?} has no extents; playing silence",
                        self.shape
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_adjuster_pans_and_attenuates() {
        let ctx = AdjustContext {
            listener: Some([0.0, 0.0, 0.0]),
            camera_state: 2,
            cam2_yaw: 0.0,
            camera_yaw: 0,
        };
        let adjuster = |position| Adjuster {
            shape: SoundShape::Stereo,
            position,
            size: 0.0,
            range: 8192.0,
        };
        // East of the listener with camera yaw 0: fully right (the pan
        // reaches 16384 at 4096 units or more).
        let mut gains = [1.0, 1.0];
        adjuster([4096.0, 0.0, 0.0]).apply(&mut gains, &ctx);
        assert!(gains[0].abs() < 1e-6, "{gains:?}");
        assert!((gains[1] - 0.5 * 2.0_f32.sqrt()).abs() < 1e-5, "{gains:?}");
        // Straight ahead: centred, equal power sqrt(2 * 0.5) per side.
        let mut gains = [1.0, 1.0];
        adjuster([0.0, 0.0, 2048.0]).apply(&mut gains, &ctx);
        assert!(
            (gains[0] - gains[1]).abs() < 1e-6 && (gains[0] - 0.75).abs() < 1e-5,
            "{gains:?}"
        );
        // Beyond the range: silent.
        let mut gains = [1.0, 1.0];
        adjuster([0.0, 0.0, 9000.0]).apply(&mut gains, &ctx);
        assert_eq!(gains, [0.0, 0.0]);
    }
}
