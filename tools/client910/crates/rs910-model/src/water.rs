//! High-detail water inputs that the GL renderer generates on the CPU.
//!
//! - A seeded gradient noise (a permutation shuffled by the 48-bit LCG random, quintic fade).
//! - The octave sum of that noise into a signed byte volume.
//! - The 128x128x16 RGBA normal/height volume bound as the environment-mapped
//!   water's normal sampler.
//! - The uniform constants of the water shader and its time term.
//!
//! The volume is deterministic (seed 419684), so it is generated once and
//! uploaded like the other renderer textures.

/// The 48-bit LCG random (multiplier `0x5DEECE66D`), the subset the gradient noise uses.
struct Lcg48 {
    seed: i64,
}

impl Lcg48 {
    fn new(seed: i64) -> Self {
        Self {
            seed: (seed ^ 0x5_DEEC_E66D) & ((1 << 48) - 1),
        }
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(0x5_DEEC_E66D).wrapping_add(0xB)) & ((1 << 48) - 1);
        (self.seed >> (48 - bits)) as i32
    }

    fn next_int(&mut self) -> i32 {
        self.next(32)
    }
}

/// The seeded gradient noise behind the water volume.
pub struct GradientNoise {
    perm: [i32; 512],
}

/// The size of a noise volume.
#[derive(Clone, Copy, Debug)]
pub struct VolumeSize {
    pub width: i32,
    pub height: i32,
    pub depth: i32,
}

/// One octave of noise: its frequency per axis and its amplitude.
#[derive(Clone, Copy, Debug)]
pub struct Octave {
    pub frequency: [f32; 3],
    pub amplitude: f32,
}

impl GradientNoise {
    /// The permutation table (the golden test's `water_perm`,
    /// `client910/src/model_goldens.rs`).
    #[cfg(any(test, feature = "test-hooks"))]
    #[must_use]
    pub fn perm(&self) -> &[i32; 512] {
        &self.perm
    }
}

/// The eight cube-corner gradients.
const GRADIENTS: [[f32; 3]; 8] = [
    [-0.333_333, -0.333_333, -0.333_333],
    [0.333_333, -0.333_333, -0.333_333],
    [-0.333_333, 0.333_333, -0.333_333],
    [0.333_333, 0.333_333, -0.333_333],
    [-0.333_333, -0.333_333, 0.333_333],
    [0.333_333, -0.333_333, 0.333_333],
    [-0.333_333, 0.333_333, 0.333_333],
    [0.333_333, 0.333_333, 0.333_333],
];

/// `t^3 (t (6t - 15) + 10)`.
fn fade(t: f32) -> f32 {
    t * t * t * ((t * 6.0 - 15.0) * t + 10.0)
}

/// `(b - a) * t + a`.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    (b - a) * t + a
}

/// Dot with gradient `g`, summed in `z, x, y` order.
fn grad(g: i32, x: f32, y: f32, z: f32) -> f32 {
    let v = GRADIENTS[g as usize];
    v[2] * z + v[0] * x + v[1] * y
}

impl GradientNoise {
    /// The noise for `seed`.
    #[must_use]
    pub fn new(seed: i32) -> Self {
        let mut random = Lcg48::new(i64::from(seed));
        let mut perm = [0_i32; 512];
        for i in 0..256 {
            perm[i] = i as i32;
            perm[i + 256] = i as i32;
        }
        for i in 0..256 {
            let j = (random.next_int() & 0xFF) as usize;
            let v = perm[j];
            perm[j] = perm[i];
            perm[j + 256] = perm[i];
            perm[i] = v;
            perm[i + 256] = v;
        }
        Self { perm }
    }

    /// One `width x height` slice of octave noise at depth `z` of a volume of
    /// `size`.
    pub fn slice(&self, z: i32, size: VolumeSize, octave: Octave, out: &mut [f32]) {
        let VolumeSize {
            width: w,
            height: h,
            depth: d,
        } = size;
        let [fx, fy, fz] = octave.frequency;
        let amplitude = octave.amplitude;
        let p = &self.perm;
        let mx = ((w as f32 * fx - 1.0) as i32) & 0xFF;
        let my = ((h as f32 * fy - 1.0) as i32) & 0xFF;
        let mz = ((d as f32 * fz - 1.0) as i32) & 0xFF;
        let zf = z as f32 * fz;
        let z0 = zf as i32;
        let z1 = z0 + 1;
        let tz = zf - z0 as f32;
        let tz1 = 1.0 - tz;
        let fzz = fade(tz);
        let a = p[(z0 & mz) as usize];
        let b = p[(z1 & mz) as usize];
        let mut i = 0;
        for yy in 0..h {
            let yf = yy as f32 * fy;
            let y0 = yf as i32;
            let y1 = y0 + 1;
            let ty = yf - y0 as f32;
            let ty1 = 1.0 - ty;
            let fyy = fade(ty);
            let ya = y0 & my;
            let yb = y1 & my;
            let aa = p[(a + ya) as usize];
            let ab = p[(a + yb) as usize];
            let ba = p[(b + ya) as usize];
            let bb = p[(b + yb) as usize];
            for xx in 0..w {
                let xf = xx as f32 * fx;
                let x0 = xf as i32;
                let x1 = x0 + 1;
                let tx = xf - x0 as f32;
                let tx1 = 1.0 - tx;
                let fxx = fade(tx);
                let xa = x0 & mx;
                let xb = x1 & mx;
                let g = |base: i32, xi: i32| p[(base + xi) as usize] & 0x7;
                out[i] = amplitude
                    * lerp(
                        lerp(
                            lerp(
                                grad(g(aa, xa), tx1, ty1, tz1),
                                grad(g(aa, xb), tx, ty1, tz1),
                                fxx,
                            ),
                            lerp(
                                grad(g(ab, xa), tx1, ty, tz1),
                                grad(g(ab, xb), tx, ty, tz1),
                                fxx,
                            ),
                            fyy,
                        ),
                        lerp(
                            lerp(
                                grad(g(ba, xa), tx1, ty1, tz),
                                grad(g(ba, xb), tx, ty1, tz),
                                fxx,
                            ),
                            lerp(
                                grad(g(bb, xa), tx1, ty, tz),
                                grad(g(bb, xb), tx, ty, tz),
                                fxx,
                            ),
                            fyy,
                        ),
                        fzz,
                    );
                i += 1;
            }
        }
    }
}

/// Float to signed byte: truncate and saturate to `i32`, then narrow.
fn f2b(v: f32) -> i8 {
    (v as i32) as i8
}

/// The octave sum of `noise` into a signed byte volume: `octaves` octaves,
/// starting at `first` and doubling the frequency and scaling the amplitude by
/// `persistence` each time.
#[must_use]
pub fn octave_volume(
    size: VolumeSize,
    octaves: i32,
    noise: &GradientNoise,
    first: Octave,
    persistence: f32,
) -> Vec<i8> {
    let VolumeSize {
        width: w,
        height: h,
        depth: d,
    } = size;
    let [fx, fy, fz] = first.frequency;
    let amplitude = first.amplitude;
    let area = (w * h) as usize;
    let mut out = vec![0_i8; area * d as usize];
    let mut slice = vec![0.0_f32; area];
    for z in 0..d {
        let base = z as usize * area;
        let (mut fx, mut fy, mut fz, mut amp) = (fx, fy, fz, amplitude);
        for _ in 0..octaves {
            noise.slice(
                z,
                size,
                Octave {
                    frequency: [fx / w as f32, fy / h as f32, fz / d as f32],
                    amplitude: amp * 127.0,
                },
                &mut slice,
            );
            for i in 0..area {
                out[base + i] = f2b(f32::from(out[base + i]) + slice[i]);
            }
            fx *= 2.0;
            fy *= 2.0;
            fz *= 2.0;
            amp *= persistence;
        }
        for v in &mut out[base..base + area] {
            *v = v.wrapping_add(127);
        }
    }
    out
}

/// The RGBA8 normal sampler volume (128x128x16): `rgb` = the height field's normal remapped to
/// `[0, 255]`, `a` = the height (wave crest term).
#[must_use]
pub fn normal_volume() -> Vec<u8> {
    let heights = octave_volume(
        VolumeSize {
            width: 128,
            height: 128,
            depth: 16,
        },
        8,
        &GradientNoise::new(419_684),
        Octave {
            frequency: [4.0, 4.0, 16.0],
            amplitude: 0.5,
        },
        0.6,
    );
    let h = |i: usize| i32::from(heights[i] as u8);
    let mut out = Vec::with_capacity(heights.len() * 4);
    for slice in 0..16_usize {
        let base = slice * 16384;
        let mut src = base;
        for y in 0..128_usize {
            let row = y * 128 + base;
            let up = ((y as i32 - 1) & 0x7F) as usize * 128 + base;
            let down = ((y + 1) & 0x7F) * 128 + base;
            for x in 0..128_usize {
                let dz = (h(up + x) - h(down + x)) as f32;
                let dx =
                    (h(((x as i32 - 1) & 0x7F) as usize + row) - h(((x + 1) & 0x7F) + row)) as f32;
                let scale = (128.0 / f64::from(dz * dz + dx * dx + 16384.0).sqrt()) as f32;
                out.push(f2b(dx * scale + 127.0) as u8);
                out.push(f2b(scale * 128.0 + 127.0) as u8);
                out.push(f2b(dz * scale + 127.0) as u8);
                out.push(heights[src] as u8);
                src += 1;
            }
        }
    }
    out
}

/// The water shader's time term: `(millis * (1 << (argument & 3)) % 40000) /
/// 40000` in wrapping `int` arithmetic.
#[must_use]
pub fn time_fraction(millis: i32, argument: u8) -> f32 {
    let speed = 1_i32 << (argument & 3);
    (millis.wrapping_mul(speed) % 40000) as f32 / 40000.0
}

/// The underwater height fog of a lit floor batch: plane `(0, 1/(scale*0.15), 0, 256/(scale*0.15))`
/// and the batch water-fog colour.
#[must_use]
pub fn underwater_lit_fog(fog: &crate::floor::WaterFogData) -> ([f32; 4], [f32; 4]) {
    let k = 0.15_f32;
    let s = fog.scale as f32 * k;
    ([0.0, 1.0 / s, 0.0, 256.0 / s], rgb(fog.colour))
}

/// The underwater height fog of an unlit floor batch: plane
/// `(0, 1, 0, offset/255*scale + height_bias) * (1/scale)`.
#[must_use]
pub fn underwater_unlit_fog(
    fog: &crate::floor::WaterFogData,
    height_bias: i32,
) -> ([f32; 4], [f32; 4]) {
    let scale = fog.scale as f32;
    let w = fog.offset as f32 / 255.0 * scale + height_bias as f32;
    let inv = 1.0 / scale;
    ([0.0, inv, 0.0, w * inv], rgb(fog.colour))
}

fn rgb(colour: i32) -> [f32; 4] {
    [
        ((colour >> 16) & 0xFF) as f32 / 255.0,
        ((colour >> 8) & 0xFF) as f32 / 255.0,
        (colour & 0xFF) as f32 / 255.0,
        0.0,
    ]
}

/// The height bias of the underwater pass's initial fog (colour 1583160, scale
/// 40, offset 127, then 63, 0, 0, 0).
pub const UNDERWATER_HEIGHT_BIAS: i32 = -1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcg48_matches_reference_sequence() {
        // Seed 42, `next_int()` x3 (reference values of the 48-bit LCG).
        let mut r = Lcg48::new(42);
        assert_eq!(r.next_int(), -1_170_105_035);
        assert_eq!(r.next_int(), 234_785_527);
        assert_eq!(r.next_int(), -1_360_544_799);
    }

    #[test]
    fn time_fraction_wraps_like_i32() {
        assert_eq!(time_fraction(20000, 0), 0.5);
        assert_eq!(time_fraction(20000, 1), 0.0);
        assert_eq!(time_fraction(10000, 6), 0.0);
        // 32-bit overflow before the modulo.
        let big = i32::MAX;
        assert_eq!(
            time_fraction(big, 3),
            (big.wrapping_mul(8) % 40000) as f32 / 40000.0
        );
    }

    #[test]
    fn underwater_fog_planes_follow_the_batch_fog() {
        let fog = crate::floor::WaterFogData {
            colour: 0x102030,
            scale: 512,
            offset: 255,
            ..crate::floor::WaterFogData::default()
        };
        let (plane, colour) = underwater_lit_fog(&fog);
        assert!((plane[1] - 1.0 / 76.8).abs() < 1e-6);
        assert!((plane[3] - 256.0 / 76.8).abs() < 1e-4);
        assert_eq!(colour[0], 16.0 / 255.0);
        let (plane, _) = underwater_unlit_fog(&fog, UNDERWATER_HEIGHT_BIAS);
        assert_eq!(plane[1], 1.0 / 512.0);
        assert_eq!(plane[3], (512.0 - 1.0) / 512.0);
    }
}
