//! The inverse MDCT that turns a decoded spectrum into time-domain samples,
//! and the power-complementary window that blends neighbouring blocks.
//!
//! The transform is the split-radix style algorithm of the reference Vorbis
//! decoder, working in place on a buffer of one block (`n` samples in, the
//! windowed-ready half-overlapped samples out).

use super::bits::{bit_length, reverse_bits};
use std::f64::consts::{FRAC_PI_2, PI};

/// The trigonometric and bit-reversal tables of one block size.
#[derive(Clone, Debug, Default)]
pub(super) struct MdctTables {
    /// Twiddles for the butterfly stages: `cos, -sin` of `k * 4 * pi / n`.
    stage: Vec<f32>,
    /// Twiddles for the final rotation: `cos, sin` of `(2k + 1) * pi / 2n`.
    rotation: Vec<f32>,
    /// Twiddles for the middle pass: `cos, -sin` of `(4k + 2) * pi / n`.
    middle: Vec<f32>,
    /// Bit-reversed indices of the `n / 8` butterflies.
    bit_reversed: Vec<i32>,
}

impl MdctTables {
    /// Build the tables for blocks of `n` samples.
    pub(super) fn new(n: i32) -> Self {
        let half = n >> 1;
        let quarter = n >> 2;
        let eighth = n >> 3;
        let mut stage = vec![0.0_f32; half as usize];
        for k in 0..quarter as usize {
            let angle = f64::from(k as i32 * 4) * PI / f64::from(n);
            stage[k * 2] = angle.cos() as f32;
            stage[k * 2 + 1] = -(angle.sin() as f32);
        }
        let mut rotation = vec![0.0_f32; half as usize];
        for k in 0..quarter as usize {
            let angle = f64::from(k as i32 * 2 + 1) * PI / f64::from(n * 2);
            rotation[k * 2] = angle.cos() as f32;
            rotation[k * 2 + 1] = angle.sin() as f32;
        }
        let mut middle = vec![0.0_f32; quarter as usize];
        for k in 0..eighth as usize {
            let angle = f64::from(k as i32 * 4 + 2) * PI / f64::from(n);
            middle[k * 2] = angle.cos() as f32;
            middle[k * 2 + 1] = -(angle.sin() as f32);
        }
        let index_bits = bit_length(eighth - 1);
        let bit_reversed = (0..eighth).map(|k| reverse_bits(k, index_bits)).collect();
        Self {
            stage,
            rotation,
            middle,
            bit_reversed,
        }
    }
}

/// Multiply one slope of the window: `sin(pi/2 * sin^2(x))` for the
/// position `x` (already scaled into the slope's quarter period).
fn slope(x: f64) -> f32 {
    let s = x.sin() as f32;
    (f64::from(s) * FRAC_PI_2 * f64::from(s)).sin() as f32
}

/// Apply the rising window slope over `w[start..end]`; the slope is
/// `slope_len` samples long.
pub(super) fn window_rise(w: &mut [f32], start: i32, end: i32, slope_len: i32) {
    for i in start..end {
        let x = (f64::from(i - start) + 0.5) / f64::from(slope_len) * 0.5 * PI;
        w[i as usize] *= slope(x);
    }
}

/// Apply the falling window slope over `w[start..end]`.
pub(super) fn window_fall(w: &mut [f32], start: i32, end: i32, slope_len: i32) {
    for i in start..end {
        let x = (f64::from(i - start) + 0.5) / f64::from(slope_len) * 0.5 * PI + FRAC_PI_2;
        w[i as usize] *= slope(x);
    }
}

/// The in-place inverse MDCT of one block of `n` samples: the first half of
/// `w` holds the spectrum on entry, and the whole buffer holds the time
/// samples (before windowing) on exit.
pub(super) fn imdct(w: &mut [f32], n: i32, tables: &MdctTables) {
    let n_us = n as usize;
    let half = n_us >> 1;
    let quarter = n_us >> 2;
    let eighth = n_us >> 3;
    let (stage, rotation, middle, reversed) = (
        &tables.stage,
        &tables.rotation,
        &tables.middle,
        &tables.bit_reversed,
    );
    for x in &mut w[..half] {
        *x *= 0.5;
    }
    for i in half..n_us {
        w[i] = -w[n_us - i - 1];
    }
    // First butterfly pass: fold the spectrum against its mirror.
    for k in 0..quarter {
        let a = w[k * 4] - w[n_us - k * 4 - 1];
        let b = w[k * 4 + 2] - w[n_us - k * 4 - 3];
        let cos = stage[k * 2];
        let sin = stage[k * 2 + 1];
        w[n_us - k * 4 - 1] = a * cos - b * sin;
        w[n_us - k * 4 - 3] = a * sin + b * cos;
    }
    for k in 0..eighth {
        let hi_odd3 = w[k * 4 + half + 3];
        let hi_odd1 = w[k * 4 + half + 1];
        let lo_odd3 = w[k * 4 + 3];
        let lo_odd1 = w[k * 4 + 1];
        w[k * 4 + half + 3] = hi_odd3 + lo_odd3;
        w[k * 4 + half + 1] = hi_odd1 + lo_odd1;
        let cos = stage[half - 4 - k * 4];
        let sin = stage[half - 3 - k * 4];
        w[k * 4 + 3] = (hi_odd3 - lo_odd3) * cos - (hi_odd1 - lo_odd1) * sin;
        w[k * 4 + 1] = (hi_odd3 - lo_odd3) * sin + (hi_odd1 - lo_odd1) * cos;
    }
    // Middle butterfly stages, halving the span each time.
    let log = bit_length(n - 1);
    for pass in 0..(log - 3).max(0) as u32 {
        let span = n_us >> (pass + 2);
        let stride = 8_usize << pass;
        for block in 0..(2_usize << pass) {
            let hi = n_us - span * 2 * block;
            let lo = n_us - (block * 2 + 1) * span;
            for r in 0..(n_us >> (pass + 4)) {
                let off = r * 4;
                let hi1 = w[hi - 1 - off];
                let hi3 = w[hi - 3 - off];
                let lo1 = w[lo - 1 - off];
                let lo3 = w[lo - 3 - off];
                w[hi - 1 - off] = hi1 + lo1;
                w[hi - 3 - off] = hi3 + lo3;
                let cos = stage[stride * r];
                let sin = stage[stride * r + 1];
                w[lo - 1 - off] = (hi1 - lo1) * cos - (hi3 - lo3) * sin;
                w[lo - 3 - off] = (hi1 - lo1) * sin + (hi3 - lo3) * cos;
            }
        }
    }
    // Bit-reversal permutation of the odd samples of each group of eight.
    for (i, &r) in reversed[..eighth.saturating_sub(1)]
        .iter()
        .enumerate()
        .skip(1)
    {
        let j = r as usize;
        if i < j {
            let i8 = i * 8;
            let j8 = j * 8;
            w.swap(i8 + 1, j8 + 1);
            w.swap(i8 + 3, j8 + 3);
            w.swap(i8 + 5, j8 + 5);
            w.swap(i8 + 7, j8 + 7);
        }
    }
    for i in 0..half {
        w[i] = w[i * 2 + 1];
    }
    for k in 0..eighth {
        w[n_us - 1 - k * 2] = w[k * 4];
        w[n_us - 2 - k * 2] = w[k * 4 + 1];
        w[n_us - quarter - 1 - k * 2] = w[k * 4 + 2];
        w[n_us - quarter - 2 - k * 2] = w[k * 4 + 3];
    }
    // Middle pass.
    for k in 0..eighth {
        let cos = middle[k * 2];
        let sin = middle[k * 2 + 1];
        let a = w[k * 2 + half];
        let b = w[k * 2 + half + 1];
        let c = w[n_us - 2 - k * 2];
        let d = w[n_us - 1 - k * 2];
        let cross = (a - c) * sin + (b + d) * cos;
        w[k * 2 + half] = (a + c + cross) * 0.5;
        w[n_us - 2 - k * 2] = (a + c - cross) * 0.5;
        let cross2 = (b + d) * sin - (a - c) * cos;
        w[k * 2 + half + 1] = (b - d + cross2) * 0.5;
        w[n_us - 1 - k * 2] = (-b + d + cross2) * 0.5;
    }
    // Final rotation and unfolding into the output halves.
    for k in 0..quarter {
        let first = rotation[k * 2] * w[k * 2 + half] + rotation[k * 2 + 1] * w[k * 2 + 1 + half];
        let second = w[k * 2 + half] * rotation[k * 2 + 1] - rotation[k * 2] * w[k * 2 + 1 + half];
        w[k] = first;
        w[half - 1 - k] = second;
    }
    for k in 0..quarter {
        w[n_us - quarter + k] = -w[k];
    }
    for k in 0..quarter {
        w[k] = w[quarter + k];
    }
    for k in 0..quarter {
        w[quarter + k] = -w[quarter - k - 1];
    }
    for k in 0..quarter {
        w[half + k] = w[n_us - k - 1];
    }
}
