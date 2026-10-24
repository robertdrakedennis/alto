//! The modern client's volumetric scattering (renderer plan §4(s)): the sun's light scattered
//! towards the eye along each pixel's view ray, shadowed by the cascades (sun shafts, lit haze),
//! over the lit frame before the tonemap. `CLIENT910_MODERN_VOLUMETRICS=off` skips the pass (the
//! frame before the lane byte for byte).
//!
//! # The law
//!
//! - Per pixel (per sample under MSAA) the world point from the depth, the eye distance `x` and
//!   the unit ray `v`; the jitter is `fract(dither + sample jitter)`; the march uses the Mie
//!   constants mixed between the ground's and the sky's by the `sky` share; for the sky the lit
//!   and unlit terms are scaled by `P.z` and the extinction is `exp(-x P.y P.z)` (`sky` is the
//!   pixel's depth scaled by a far-plane parameter); then the result is applied to the frame.
//! - The march: the sample count of samples from the eye along the ray up to
//!   `h = min(1.4 * shadow fade end, x)` at `fract((jitter + i) / n)`; each adds
//!   `(lit, 1 - lit) exp(-h s P.y)` with `lit` one hardware PCF tap of the cascade that holds the
//!   point (lit past the last cascade); both times `P.x h / n`; the extinction `exp(-x P.y)`;
//!   past `h` the rest of the ray lit: `+ P.x (x - h) exp(-x P.y)`; the shadow exaggeration (the
//!   lit share raised to `P.w`); the lit term times the phase function
//!   `g.w g.x / (g.y - g.z dot(v, h))^1.5` (Henyey-Greenstein with
//!   `(1 - g^2, 1 + g^2, 2g, 1/4pi)`).
//! - The apply: in-scattered `lit sun colour lit fog colour + unlit ambient colour unlit fog
//!   colour`, the colour times the extinction plus it.
//!
//! # This port
//!
//! Non-temporal (the modern client reprojects and accumulates frames, and upsamples a half-size
//! march): one full-size march per pixel with a fixed 4x4 Bayer dither and no per-frame jitter
//! (jitter 0), so fixed-clock frames repeat exactly. The depth: the scene depth's first sample;
//! the sky: depth 1 (the modern client's far-plane parameter is set on its CPU side; read here as
//! "the pixel is the far plane"). Point-light volumetrics are not done.
//!
//! Values the modern client sets from its CPU side (chosen; in brackets the names they were tuned
//! under): [`SAMPLES`] [`vol_samples`], the scattering [`SCATTERING`] [`vol_s`] and extinction
//! [`EXTINCTION`] [`vol_t`] per fine unit, the sky's share [`SKY_SHARE`], the shadow exaggeration
//! [`EXAGGERATION`], the phase asymmetry [`MIE_G`] (ground) and [`MIE_G_SKY`], the lit and unlit
//! fog colours [`LIT_FOG`] [`vol_lit`] and [`UNLIT_FOG`]. The 910 file-6 record's unknown block
//! `@138` (five floats, defaults `0.01, 1.5, 0.55, 3, 0.5`; Lumbridge `0.02, 1.2, 0.5, 2.88,
//! 0.5`) has the shape of these parameters (a density, a scale, an asymmetry near 0.55, an
//! exaggeration near 3, a sky share of 0.5); unproven, so it is not read; the chosen asymmetry,
//! exaggeration and sky share take the classic defaults.

/// Chosen: the march's sample count.
pub const SAMPLES: f32 = 16.0;
/// Chosen: the scattering (`P.x`), per fine unit.
pub const SCATTERING: f32 = 4.0e-6;
/// Chosen: the extinction (`P.y`), per fine unit.
pub const EXTINCTION: f32 = 4.0e-6;
/// Chosen: the sky's share (`P.z`; the classic default of the
/// record's block `@138`, last float).
pub const SKY_SHARE: f32 = crate::lighting::look::CLASSIC_VOLUMETRIC_SKY_SHARE;
/// Chosen: the shadow exaggeration (`P.w`; the classic default, `@138`
/// fourth float).
pub const EXAGGERATION: f32 = 3.0;
/// Chosen: the Mie asymmetry of the ground (the classic default, `@138` third
/// float).
pub const MIE_G: f32 = 0.55;
/// Chosen: the Mie asymmetry of the sky.
pub const MIE_G_SKY: f32 = 0.55;
/// Chosen: the lit fog colour (grey; times the sun colour).
pub const LIT_FOG: f32 = 4.0;
/// Chosen: the unlit fog colour (grey; times the ambient).
pub const UNLIT_FOG: f32 = 0.0;

/// The Mie constants for asymmetry `g`: `(1 - g^2, 1 + g^2, 2g, 1/4pi)`.
#[must_use]
pub fn mie(g: f32) -> [f32; 4] {
    [
        1.0 - g * g,
        1.0 + g * g,
        2.0 * g,
        1.0 / (4.0 * std::f32::consts::PI),
    ]
}

/// The phase function of `cos` for the Mie constants `m`.
#[must_use]
pub fn phase(cos: f32, m: [f32; 4]) -> f32 {
    m[3] * (m[0] / (m[1] - m[2] * cos).powf(1.5))
}

/// The shadow exaggeration: the lit share of `(lit, unlit)` raised to `q`, the total kept.
#[must_use]
pub fn exaggerate(v: [f32; 2], q: f32) -> [f32; 2] {
    let u = v[0] + v[1];
    if u <= 0.0 {
        return v;
    }
    let mut s = v[0] / u;
    if s > 0.0 && q > 0.0 {
        s = s.powf(q);
    }
    [u * s, u * (1.0 - s)]
}

/// The 4x4 Bayer matrix (this port's dither matrix), `/ 16`.
pub const DITHER: [f32; 16] = [
    0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0,
];

/// CPU mirror of the march for a ray of length
/// `dist` whose samples are lit where `lit(t)` (t: the distance along the
/// ray) says, with jitter `j`: `(lit term, extinction, unlit term)`
/// before the colours, phase included.
#[must_use]
pub fn march(
    dist: f32,
    shadow_range: f32,
    j: f32,
    cos_sun: f32,
    lit: impl Fn(f32) -> f32,
) -> [f32; 3] {
    let n = SAMPLES.max(1.0) as usize;
    let (s, t) = (SCATTERING, EXTINCTION);
    let v = shadow_range * 1.4;
    let h = v.min(dist);
    let m = 1.0 / n as f32;
    let mut q = j * m;
    let mut u = [0.0_f32; 2];
    for _ in 0..n {
        let f = q.fract();
        let x = (-h * f * t).exp();
        let l = lit(h * f).min(1.0);
        u[0] += l * x;
        u[1] += (1.0 - l) * x;
        q += m;
    }
    let step = h * m;
    u = [u[0] * s * step, u[1] * s * step];
    let ext = (-dist * t).exp();
    if dist > v {
        u[0] += s * (dist - v) * ext;
    }
    let u = exaggerate(u, EXAGGERATION);
    [u[0] * phase(cos_sun, mie(MIE_G)), ext, u[1]]
}

/// The half-size targets' size for a scene viewport `rect`: the viewport's width and
/// height halved whenever the effect's half-size flag is set, which its constructor sets and
/// nothing clears; at least 1.
#[must_use]
pub fn half_size(rect: [i32; 4]) -> [u32; 2] {
    [(rect[2] / 2).max(1) as u32, (rect[3] / 2).max(1) as u32]
}

/// The passes' slot (`crate::atmosphere::sky::PassUniforms`): `p0` (s, t,
/// sky share, exaggeration), `p1` the ground's Mie constants, `p2` the sky's, `p3` the lit
/// fog colour and the sample count, `p4` the unlit fog colour and the
/// shadow range, `c[0].xy` the half-size targets' size ([`half_size`]). `sky_share` is the
/// sky's share of the scattering (the look's; [`SKY_SHARE`] in the earlier look).
#[must_use]
pub fn uniforms(
    inv_view_proj: [[f32; 4]; 4],
    rect: [f32; 4],
    shadow_range: f32,
    half: [u32; 2],
    sky_share: f32,
) -> crate::atmosphere::sky::PassUniforms {
    let lit = LIT_FOG;
    crate::atmosphere::sky::PassUniforms {
        inv_view_proj,
        rect,
        p0: [SCATTERING, EXTINCTION, sky_share, EXAGGERATION],
        p1: mie(MIE_G),
        p2: mie(MIE_G_SKY),
        p3: [lit, lit, lit, SAMPLES],
        p4: [UNLIT_FOG, UNLIT_FOG, UNLIT_FOG, shadow_range],
        c: [
            [half[0] as f32, half[1] as f32, 0.0, 0.0],
            [0.0; 4],
            [0.0; 4],
            [0.0; 4],
        ],
        ..Default::default()
    }
}
