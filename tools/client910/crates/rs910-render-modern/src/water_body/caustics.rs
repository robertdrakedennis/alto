//! Caustics on the real river bed (renderer plan §4(n)): the modern client's
//! two-step caustics over M10's terrain. The GPU half (the ray map, its pass
//! and the resolve) is `crate::frame::water::caustics`.
//!
//! # The two steps
//!
//! 1. **Rays**: the water is drawn once more from a caustics view; each
//!    fragment takes its water normal `n` and depth `d` and adds one ray into
//!    an integer light map at its own position pushed by `n.xz * d *` the
//!    refraction scale, weighted `min(fade.x / d * fade.y, 7 fade.y) *
//!    smoothstep(fade.z, fade.w, d)` in fixed point (scale 10000, atomic
//!    add).
//! 2. **Bed**: a terrain fragment below the caustics plane reads the map at
//!    its position through the caustics view (a 3x3 kernel, the centre 5, the
//!    ring 1, over 12), fades it out towards the map's edges
//!    (`smoothstep(0.4, 0.5, |c|)`), keeps it only in full sun (`step(1,
//!    shadow)`) and times the SSAO, and adds the sun colour times the caustics
//!    to the light before the albedo.
//!
//! # This crate
//!
//! - The ray map: [`MAP_RES`]² texels of [`TEXEL`] fine units, world-aligned
//!   around the camera target (the corner snapped to whole texels, so the
//!   pattern does not crawl as the camera moves), in two `u32` channels: the
//!   rays landing in a texel and the same rays at their unrefracted origin.
//!   The rays are drawn at [`COMPUTE_RES`]² (4 per texel).
//! - The light: lane Q-FX's calibration adds only the bright excess over the
//!   even light (`water_body::effects`: a flat surface adds nothing), so the
//!   resolve takes the kernel over `landed - origin` per texel, clamps at
//!   0 and divides by the rays per texel and the fixed-point scale: in the
//!   limit of many rays this is Q-FX's closed form `per_ray * fade * (1 / J -
//!   1)`, now from rays that really land on the drawn bed.
//! - The bed: the term in the M10 terrain shader, bilinear between texels (the
//!   map is sampled through a filter), `step` on the sun visibility with a
//!   0.001 tolerance (the PCF weights sum to 1 in floating point); the plane
//!   is the frame's water plane (M7's reflection plane: the water nearest the
//!   camera target).
//! - The constants are lane Q-FX's (`crate::water_body::effects::LOOK`: strength, the
//!   refraction scale, the fade-in, the falloff).

/// The ray map's texels per side.
pub const MAP_RES: u32 = 512;
/// The rays' raster per side (chosen: four rays per texel).
pub const COMPUTE_RES: u32 = 1024;
/// Fine units per map texel (chosen: a sixteenth of a tile; the map covers
/// 32 tiles around the camera target).
pub const TEXEL: f32 = 32.0;
/// The fixed-point scale of the ray sums.
pub const FIXED_POINT: f32 = 10_000.0;

/// The caustics uniform block (`Caustics` in the WGSL).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CausticUniforms {
    /// xy: the map corner (camera-local x, z); z: 1 / texel size; w:
    /// texels per side.
    pub map: [f32; 4],
    /// x: the strength (the fade's y); y: the refraction scale; zw: the
    /// fade's z and w (the fade-in depths).
    pub look: [f32; 4],
    /// x: the fade's x (the falloff depth); y: the fixed-point scale; z: the
    /// plane height (camera-local classic y, down); w: 1 = on.
    pub extra: [f32; 4],
    /// xy: the ray raster's centre (camera-local x, z); z: 1 / its half
    /// side; w: rays per texel.
    pub proj: [f32; 4],
}

/// The map corner in scene-local fine units around camera target `origin`
/// (scene-local), snapped to whole texels.
#[must_use]
pub fn map_corner(origin: [f32; 3]) -> [f32; 2] {
    let half = MAP_RES as f32 * TEXEL * 0.5;
    [
        ((origin[0] - half) / TEXEL).floor() * TEXEL,
        ((origin[2] - half) / TEXEL).floor() * TEXEL,
    ]
}

/// The frame's uniforms: the map around scene-local camera target `origin`,
/// the water plane `plane` (camera-local; `None`: no water, off).
#[must_use]
pub fn uniforms(origin: [f32; 3], plane: Option<f32>, on: bool) -> CausticUniforms {
    let look = crate::water_body::effects::LOOK;
    let side = MAP_RES as f32 * TEXEL;
    let [cx, cz] = map_corner(origin);
    let (x0, z0) = (cx - origin[0], cz - origin[2]);
    let per_texel = (COMPUTE_RES as f32 / MAP_RES as f32).powi(2);
    CausticUniforms {
        map: [x0, z0, 1.0 / TEXEL, MAP_RES as f32],
        look: [
            look.caustics_strength,
            look.caustics_refraction,
            look.caustics_fade_in[0],
            look.caustics_fade_in[1],
        ],
        extra: [
            look.caustics_falloff,
            FIXED_POINT,
            plane.unwrap_or(0.0),
            if on && plane.is_some() { 1.0 } else { 0.0 },
        ],
        proj: [x0 + side * 0.5, z0 + side * 0.5, 2.0 / side, per_texel],
    }
}

/// One ray's light (its weight, before the fixed-point
/// scale) at water depth `depth`.
#[must_use]
pub fn ray_weight(depth: f32, look: &crate::water_body::effects::Look) -> f32 {
    let [a, b] = look.caustics_fade_in;
    let t = ((depth - a) / (b - a)).clamp(0.0, 1.0);
    let fade = t * t * (3.0 - 2.0 * t);
    (look.caustics_falloff / depth.max(1e-3)).min(7.0) * look.caustics_strength * fade
}

/// The resolve of texel `(x, y)` of a `n`-texel map of `(landed, origin)`
/// fixed-point sums: the 3x3 kernel over the excess, clamped at 0, per
/// ray (the CPU mirror of `cs_resolve`).
#[must_use]
pub fn resolve(rays: &[[u32; 2]], n: usize, x: usize, y: usize, per_texel: f32) -> f32 {
    let mut sum = 0.0_f32;
    for dy in -1_i32..=1 {
        for dx in -1_i32..=1 {
            let (xx, yy) = (x as i32 + dx, y as i32 + dy);
            if xx < 0 || yy < 0 || xx >= n as i32 || yy >= n as i32 {
                continue;
            }
            let [landed, origin] = rays[yy as usize * n + xx as usize];
            let k = if dx == 0 && dy == 0 { 5.0 } else { 1.0 };
            sum += k * (landed as f32 - origin as f32);
        }
    }
    (sum / 12.0).max(0.0) / (FIXED_POINT * per_texel)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The map is world-aligned: the corner moves in whole texels, holds the
    /// camera target near its centre, and the ray raster covers it exactly.
    #[test]
    fn map_follows_the_camera_in_whole_texels() {
        let side = MAP_RES as f32 * TEXEL;
        for origin in [
            [0.0, 0.0, 0.0],
            [12_345.6, -300.0, 23_456.7],
            [31.9, 0.0, 32.1],
        ] {
            let [cx, cz] = map_corner(origin);
            assert_eq!(cx % TEXEL, 0.0);
            assert_eq!(cz % TEXEL, 0.0);
            assert!((origin[0] - cx - side / 2.0).abs() < TEXEL);
            assert!((origin[2] - cz - side / 2.0).abs() < TEXEL);
            let u = uniforms(origin, Some(10.0), true);
            assert_eq!(u.map[0], cx - origin[0]);
            // The raster's half side and centre span the map.
            assert_eq!(u.proj[2], 2.0 / side);
            assert_eq!(u.proj[0] - u.map[0], side / 2.0);
            assert_eq!(u.proj[3], 4.0);
            assert_eq!(u.extra[3], 1.0);
        }
        assert_eq!(uniforms([0.0; 3], None, true).extra[3], 0.0);
        assert_eq!(uniforms([0.0; 3], Some(0.0), false).extra[3], 0.0);
    }
}
