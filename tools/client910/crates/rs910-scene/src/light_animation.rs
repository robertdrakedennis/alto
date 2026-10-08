//! Intensity animation of static point lights and its deterministic noise row.
//! The scene draw supplies the client logic cycle, not elapsed render time.
use crate::env::StaticLight;
use std::sync::OnceLock;

pub fn intensity(light: &StaticLight, cycle: i32, steady: bool) -> f32 {
    let wave = if steady {
        2048
    } else {
        let phase = (light.speed.wrapping_mul(cycle) / 50).wrapping_add(light.phase) & 2047;
        match light.wave {
            1 => (crate::trig::sin(phase << 3) >> 4) + 1024,
            2 => phase,
            3 => noise()[phase as usize] >> 1,
            4 => (phase >> 10) << 11,
            5 => (if phase < 1024 { phase } else { 2048 - phase }) << 1,
            _ => 2048,
        }
    };
    light
        .amplitude
        .wrapping_mul(wave)
        .wrapping_shr(11)
        .wrapping_add(light.offset) as f32
        / 2048.
}

/// Q12 fade curve, using truncating fixed-point arithmetic.
fn fade(x: i32) -> i32 {
    let cube = (((x * x) >> 12) * x) >> 12;
    (cube * (((x * (x * 6 - 61440)) >> 12) + 40960)) >> 12
}

/// Bounded random index, deliberately different from a plain `next_int(n)`.
fn bounded(random: &mut crate::font_layout::Random, bound: i32) -> usize {
    if (bound as u32).is_power_of_two() {
        return ((random.next_int() as u32 as u64 * bound as u64) >> 32) as usize;
    }
    let limit = i32::MIN.wrapping_sub((4294967296_u64 % bound as u64) as i32);
    let value = loop {
        let value = random.next_int();
        if value < limit {
            break value;
        }
    };
    // Signed remainder with its unsigned-shift correction.
    let sign = (value as u32 >> 31) as i32;
    ((value.wrapping_add(sign) % bound) + ((value >> 31) & (bound - 1))) as usize
}

/// Seeded permutation table (including the zero at initial slot 255).
fn permutation(seed: i64) -> [u8; 512] {
    let mut out = [0; 512];
    for (i, value) in out.iter_mut().take(255).enumerate() {
        *value = i as u8;
    }
    let mut random = crate::font_layout::Random::default();
    random.set_seed(seed);
    for i in 0..255 {
        let last = 255 - i;
        let selected = bounded(&mut random, last as i32);
        let value = out[selected];
        out.swap(selected, last);
        out[511 - i] = value;
    }
    out
}

/// Noise sampled on row 0 only. The table is generated with height 1, so Y and
/// its fade are exactly zero and the second interpolated row has weight 0.
fn row_sample(x: i32, period: i32, p: &[u8; 512]) -> i32 {
    let cell = x >> 12;
    let next = if cell + 1 >= period { 0 } else { cell + 1 };
    let x = x & 4095;
    let hash = p[0] as usize;
    let gradient = |cell: i32, x: i32| match p[hash + (cell & 255) as usize] & 3 {
        0 | 2 => x,
        _ => -x,
    };
    let a = gradient(cell, x);
    let b = gradient(next, x - 4096);
    (((b - a) * fade(x)) >> 12) + a
}

/// The light waveform table: a seeded four-octave noise sum (2048 samples,
/// seed 35, period 8, persistence 0.4), generated once; no captured waveform
/// is shipped.
pub fn noise() -> &'static [i32; 2048] {
    static TABLE: OnceLock<[i32; 2048]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let p = permutation(35);
        let mut out = [0; 2048];
        let persistence = ((0.4_f32 * 4096.) as i32) as f32 / 4096.;
        for octave in 0..4 {
            let amplitude = ((persistence as f64).powi(octave) * 4096.) as i16 as i32;
            let frequency = 1 << octave;
            for (i, value) in out.iter_mut().enumerate() {
                let x = ((i as i32) << 12) / 2048 * 8;
                let sample = row_sample(((frequency << 12) * x) >> 12, 8 * frequency, &p);
                let sum = *value + ((amplitude * sample) >> 12);
                *value = if octave == 3 { (sum >> 1) + 2048 } else { sum };
            }
        }
        out
    })
}

#[cfg(test)]
#[path = "light_animation_tests.rs"]
mod tests;
