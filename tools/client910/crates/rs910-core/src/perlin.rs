//! Map height noise: the fallback
//! height of level-0 tiles without an explicit height,
//! `-perlin(x + 932731, z + 556238) * 8 << 2` over world tile coordinates.
//!
//! One copy for the whole client (Phase 2.1). It was `maploader.rs`'s; the
//! `map.rs` and `protocol910/terrain.rs` ports gave the same results on every
//! input tested (Phase 2.1; checked against client910's floor golden in
//! `scene_goldens::maploader`) and were merged into it. `terrain.rs` used wrapping `x - 1`/`x + 1`; this copy (like `map.rs`)
//! keeps the plain operators, which only differ for `x`/`z` next to
//! `i32::MIN`/`MAX` (a debug-build overflow panic where the original wraps).

use crate::trig;

/// Height noise at a world tile: three octaves of value noise. 32-bit
/// wrapping arithmetic.
#[must_use]
pub fn perlin(x: i32, z: i32) -> i32 {
    let octaves = perlin_scale(x.wrapping_add(45365), z.wrapping_add(91923), 4)
        .wrapping_sub(128)
        .wrapping_add(
            perlin_scale(x.wrapping_add(10294), z.wrapping_add(37821), 2).wrapping_sub(128) >> 1,
        )
        .wrapping_add(perlin_scale(x, z, 1).wrapping_sub(128) >> 2);
    let height = (f64::from(octaves) * 0.3) as i32 + 35;
    height.clamp(10, 60)
}

/// One octave: smooth noise on a grid of `freq` tiles, cosine-interpolated.
#[must_use]
pub fn perlin_scale(x: i32, z: i32, freq: i32) -> i32 {
    let x0 = x / freq;
    let fx = x & (freq - 1);
    let z0 = z / freq;
    let fz = z & (freq - 1);
    let c00 = smooth_noise(x0, z0);
    let c10 = smooth_noise(x0 + 1, z0);
    let c01 = smooth_noise(x0, z0 + 1);
    let c11 = smooth_noise(x0 + 1, z0 + 1);
    let top = interpolate(c00, c10, fx, freq);
    let bottom = interpolate(c01, c11, fx, freq);
    interpolate(top, bottom, fz, freq)
}

/// Cosine interpolation between `from` and `to`: note the Q14 cosine mixed with the
/// 65536 scale — shipped that way, kept that way.
#[must_use]
pub fn interpolate(from: i32, to: i32, step: i32, freq: i32) -> i32 {
    let weight = (65536 - trig::cos_raw((step * 8192 / freq) as usize)) >> 1;
    ((65536 - weight).wrapping_mul(from) >> 16).wrapping_add(to.wrapping_mul(weight) >> 16)
}

/// Noise averaged over the tile, its edge neighbours and its corner neighbours.
#[must_use]
pub fn smooth_noise(x: i32, z: i32) -> i32 {
    let corners = noise(x - 1, z - 1)
        .wrapping_add(noise(x + 1, z - 1))
        .wrapping_add(noise(x - 1, z + 1))
        .wrapping_add(noise(x + 1, z + 1));
    let edges = noise(x - 1, z)
        .wrapping_add(noise(x + 1, z))
        .wrapping_add(noise(x, z - 1))
        .wrapping_add(noise(x, z + 1));
    let centre = noise(x, z);
    centre / 4 + corners / 16 + edges / 8
}

/// The classic integer hash of a tile.
#[must_use]
pub fn noise(x: i32, z: i32) -> i32 {
    let seed = z.wrapping_mul(57).wrapping_add(x);
    let mixed = (seed << 13) ^ seed;
    let hash = mixed
        .wrapping_mul(mixed)
        .wrapping_mul(15731)
        .wrapping_add(789_221)
        .wrapping_mul(mixed)
        .wrapping_add(1_376_312_589)
        & i32::MAX;
    (hash >> 19) & 0xFF
}
