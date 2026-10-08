//! The waterfall billow volume: the `128 x 128 x 16` two-channel noise
//! volume the waterfall material samples for its foam scroll.
//!
//! The volume is generated, not shipped: a seeded, tileable, fixed-point
//! 3D gradient noise summed over five octaves. Every step is integer
//! arithmetic on Q12 fractions, so the volume is bit-identical on every
//! machine (`tests::volume_digest_is_stable` pins it).
//!
//! Recipe:
//! - a 256-entry permutation table (doubled to 512) shuffled by a 48-bit
//!   linear-congruential generator seeded with 12;
//! - per octave `o` the lattice has `(16, 2, 2) << o` cells per axis and
//!   wraps at that period; corner gradients are the classic 12-direction
//!   set chosen by the low four hash bits; corners blend with a Q12
//!   quintic fade;
//! - octave amplitudes are `0.45^o` in Q12 (truncated), summed in Q12;
//! - a texel is `127 + (min(|sum|, 4095) >> 5)` in both channels.

use std::sync::OnceLock;

/// Volume width in texels.
pub const WIDTH: usize = 128;
/// Volume height in texels.
pub const HEIGHT: usize = 128;
/// Volume depth in texels.
pub const DEPTH: usize = 16;
/// Bytes per texel (two identical channels).
pub const CHANNELS: usize = 2;

const SEED: i64 = 12;
const OCTAVES: usize = 5;
/// Lattice cells per axis at octave 0.
const LATTICE: [i32; 3] = [16, 2, 2];
/// Amplitude ratio between octaves.
const PERSISTENCE: f32 = 0.45;

/// The Q12 quintic fade `6t^5 - 15t^4 + 10t^3` for `t = i / 4096`, in the
/// integer evaluation order the volume was authored with.
const FADE: [i32; 4096] = fade_table();

const fn fade_table() -> [i32; 4096] {
    let mut table = [0; 4096];
    let mut i = 0;
    while i < 4096 {
        let x = i as i32;
        let cube = (((x * x) >> 12) * x) >> 12;
        let ramp = x * 6 - 61440;
        let blend = ((x * ramp) >> 12) + 40960;
        table[i] = (cube * blend) >> 12;
        i += 1;
    }
    table
}

/// The 48-bit linear-congruential generator (the standard multiplier
/// `0x5DEECE66D`), its raw 32-bit output.
struct Lcg48(u64);

impl Lcg48 {
    fn new(seed: i64) -> Self {
        Self((seed as u64 ^ 0x5_DEEC_E66D) & ((1 << 48) - 1))
    }

    fn next_u32(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(0x5_DEEC_E66D).wrapping_add(11) & ((1 << 48) - 1);
        (self.0 >> 16) as i32
    }

    /// A value in `0..bound` (`bound > 0`): a scaled 32-bit draw for a power
    /// of two, otherwise rejection sampling below the largest multiple of
    /// the bound, then the floored remainder.
    fn below(&mut self, bound: i32) -> usize {
        if bound & -bound == bound {
            return ((u64::from(self.next_u32() as u32) * bound as u64) >> 32) as usize;
        }
        let limit = i32::MIN.wrapping_sub((4_294_967_296_u64 % bound as u64) as i32);
        loop {
            let draw = self.next_u32();
            if draw < limit {
                return draw.rem_euclid(bound) as usize;
            }
        }
    }
}

/// The doubled 256-entry hash table. Slot 255 of the initial identity run
/// stays 0 (the shuffle starts from the identity of `0..255`), a quirk the
/// volume depends on.
fn permutation() -> [u8; 512] {
    let mut table = [0_u8; 512];
    for (i, slot) in table.iter_mut().take(255).enumerate() {
        *slot = i as u8;
    }
    let mut random = Lcg48::new(SEED);
    for step in 0..255 {
        let last = 255 - step;
        let picked = random.below(last as i32);
        table.swap(picked, last);
        table[last + 256] = table[last];
    }
    table
}

/// The dot product of the corner offset `(x, y, z)` (Q12) with the gradient
/// the low four bits of `hash` select.
fn corner_gradient(hash: u8, x: i32, y: i32, z: i32) -> i32 {
    let h = hash & 0xF;
    let u = if h < 8 { x } else { y };
    let v = if h < 4 {
        y
    } else if h == 12 || h == 14 {
        x
    } else {
        z
    };
    (if h & 1 == 0 { u } else { -u }) + (if h & 2 == 0 { v } else { -v })
}

/// Linear blend in Q12: `a + (b - a) * t`.
fn lerp(a: i32, b: i32, t: i32) -> i32 {
    (((b - a) * t) >> 12) + a
}

/// One octave's noise at the Q12 lattice position `(x, y, z)`; each axis
/// wraps at its own `period` (in cells).
fn octave_noise(table: &[u8; 512], position: [i32; 3], period: [i32; 3]) -> i32 {
    let cell = position.map(|p| p >> 12);
    let low = cell.map(|c| (c & 0xFF) as usize);
    let high: [usize; 3] = std::array::from_fn(|axis| {
        let next = cell[axis] + 1;
        if next >= period[axis] {
            0
        } else {
            (next & 0xFF) as usize
        }
    });
    let frac = position.map(|p| p & 0xFFF);
    let weight = frac.map(|f| FADE[f as usize]);
    let back = frac.map(|f| f - 4096);
    let [x_lo, y_lo, z_lo] = low;
    let [x_hi, y_hi, z_hi] = high;
    let [fx, fy, fz] = frac;
    let [bx, by, bz] = back;
    let hash = |i: usize| usize::from(table[i]);
    let z_lo_hash = hash(z_lo);
    let z_hi_hash = hash(z_hi);
    let row_lo_lo = hash(y_lo + z_lo_hash);
    let row_hi_lo = hash(y_hi + z_lo_hash);
    let row_lo_hi = hash(y_lo + z_hi_hash);
    let row_hi_hi = hash(y_hi + z_hi_hash);
    // The lower z plane, then the upper.
    let plane = |row_lo: usize, row_hi: usize, dz: i32| {
        let near = lerp(
            corner_gradient(table[x_lo + row_lo], fx, fy, dz),
            corner_gradient(table[x_hi + row_lo], bx, fy, dz),
            weight[0],
        );
        let far = lerp(
            corner_gradient(table[x_lo + row_hi], fx, by, dz),
            corner_gradient(table[x_hi + row_hi], bx, by, dz),
            weight[0],
        );
        lerp(near, far, weight[1])
    };
    let lower = plane(row_lo_lo, row_hi_lo, fz);
    let upper = plane(row_lo_hi, row_hi_hi, bz);
    lerp(lower, upper, weight[2])
}

/// The octave amplitudes in Q12: `0.45^o`, truncated.
fn amplitudes() -> [i32; OCTAVES] {
    let ratio = f64::from(PERSISTENCE);
    std::array::from_fn(|octave| (ratio.powi(octave as i32) * 4096.0) as i16 as i32)
}

/// Generate the volume: `WIDTH * HEIGHT * DEPTH` texels of [`CHANNELS`]
/// bytes each, `x` fastest, then `y`, then `z`.
#[must_use]
pub fn generate() -> Vec<u8> {
    let table = permutation();
    let amplitude = amplitudes();
    let axis = |texels: usize| -> Vec<i32> {
        (0..texels as i32)
            .map(|i| (i << 12) / texels as i32)
            .collect()
    };
    let (xs, ys, zs) = (axis(WIDTH), axis(HEIGHT), axis(DEPTH));
    let mut out = Vec::with_capacity(WIDTH * HEIGHT * DEPTH * CHANNELS);
    for &z in &zs {
        for &y in &ys {
            for &x in &xs {
                let mut sum = 0_i32;
                for (octave, &weight) in amplitude.iter().enumerate() {
                    let frequency = 1 << octave;
                    let period = LATTICE.map(|cells| cells * frequency);
                    let position: [i32; 3] =
                        std::array::from_fn(|axis| [x, y, z][axis] * frequency * LATTICE[axis]);
                    let noise = octave_noise(&table, position, period);
                    sum += (weight * noise) >> 12;
                }
                let magnitude = sum.abs().min(4095);
                let texel = (127 + (magnitude >> 5)) as u8;
                out.extend_from_slice(&[texel; CHANNELS]);
            }
        }
    }
    out
}

/// The volume, generated once per process.
#[must_use]
pub fn volume() -> &'static [u8] {
    static VOLUME: OnceLock<Vec<u8>> = OnceLock::new();
    VOLUME.get_or_init(generate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FNV-1a over the volume: any change to the generator moves the digest.
    fn digest(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xCBF2_9CE4_8422_2325, |hash, &b| {
            (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
        })
    }

    #[test]
    fn volume_digest_is_stable() {
        let volume = generate();
        assert_eq!(volume.len(), WIDTH * HEIGHT * DEPTH * CHANNELS);
        assert_eq!(digest(&volume), 5_298_155_083_610_615_027);
    }
}
