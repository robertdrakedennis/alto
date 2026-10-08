//! The modern client's light scattering, the aerial perspective (renderer plan §4(s)): each
//! surface's in-scattering and extinction along the view ray, folded together with the distance
//! fog exactly as the modern client's geometry programs fold them.
//!
//! # The law
//!
//! - With the view direction `v`, the eye distance `o` and the direction towards the sun `h`:
//!   `t = max(0, o - P.y)`, `p = max(0, dot(v, h))^2`, the tint `u = mix(tint colour, 1, p)` (white
//!   towards the sun), `m = -t P.x 0.001`; the extinction is `exp(m out_amount)` and the
//!   in-scattering is `in_amount (1 - exp(m u))` (`P` is the four-float scattering parameter
//!   block).
//! - In the vertex stage of the terrain, models, water, billboards and particles the vertex
//!   computes both, then the fog `f` of the eye distance folds in: `in = mix(in, fog colour, f)`,
//!   `out *= 1 - f`. The fragment programs apply `c out + in`.
//! - The sky's own use of the law is [`sky`]'s.
//!
//! # This port
//!
//! [`WGSL`]'s `scatter(x, f, world)` takes the colour the modules already fog (`x = c (1 - f) +
//! F f`, the fidelity pass's `apply_fog` and the terrain's and water's own) and returns
//! `x T + I (1 - f) + F f (1 - T)`, which is the modern client's `c T (1 - f) + mix(I, F, f)`
//! exactly; so the fog stays the modules' own (per vertex) and the fold matches. The scattering
//! is evaluated per fragment from the interpolated camera-local position (the modern client
//! does it per vertex, then interpolates): the same function, and no new varyings in the shared
//! vertex stages. Patched in (text, each anchor asserted): the forward module (models RT5/RT7,
//! the probe shading, billboards and particles; `shaders::forward_wgsl`), the terrain
//! (`crate::terrain::terrain_wgsl`) and the water surface (`water_body`). Only with the fidelity
//! shading on (the pre-fidelity module keeps its fog).
//!
//! The values. The 910 map file 6 record of the camera's square carries the scattering block
//! (`rs910_config::nxt::map_environment`: `@44` four floats, `@60` tint, out- and in-scattering;
//! inferred from the classic renderer's store offsets, `nxt-data-formats.md` §3): Lumbridge
//! `(4e-5, 13500, 1, 1.5)`, tint 0.6, out 77/255, in `(77, 77, 128)/255`. The classic defaults
//! when a square has none: out 0.3, in 0.3, tint 0.6, `(1e-5, 17500, 8, 1.5)`. The classic
//! renderer sets the uniforms from these fields unchanged (out, in, tint and the four floats,
//! each faded per field on a square change). What the modern client's CPU side does between the
//! record and the first scattering parameter is not known (the record's `4e-5` in the shader's
//! `0.001` law is invisible), so the density and the scale of the in-scattering are calibrated
//! (chosen constants, below) against the modern client's look reference (`ref/nxt-look-reference/`,
//! statistics in the plan §4(s)); the colours, the start distance and the tint come from the
//! record.
//!
//! The uniforms ride in the `Frame` block's unused `w` members (the block is 256 bytes and the
//! probe capture slots are 256 apart): the colours as three 8-bit channels (the record's values
//! are `n / 255`), the density, the start, the in-scattering scale and the on flag ([`pack`]).
/// The classic default out-scattering amount (0.3).
pub const DEF_OUT: [f32; 3] = [0.3; 3];
/// The classic default in-scattering amount (0.3).
pub const DEF_IN: [f32; 3] = [0.3; 3];
/// The classic default scattering tint (0.6).
pub const DEF_TINT: [f32; 3] = [0.6; 3];
/// The classic default scattering parameters (1e-5, 17500, 8, 1.5).
pub const DEF_PARAMS: [f32; 4] = [1.0e-5, 17_500.0, 8.0, 1.5];

/// Chosen (calibrated, plan §4(s)): the extinction per fine unit per unit
/// of the record's first float (`m = -t x DENSITY_SCALE`; the modern client's shader
/// multiplies its uniform by 0.001, and how its CPU side fills that from the record is
/// not known).
pub const DENSITY_SCALE: f32 = 0.25;
/// Chosen (calibrated): the in-scattering's HDR scale (the modern client's in-scattering
/// amount is in its own exposure's units).
pub const INSCATTER_SCALE: f32 = crate::lighting::look::CLASSIC_INSCATTER_SCALE;
/// Chosen (calibrated): distances of the record in this backend's view
/// range. The modern client's reference shows the haze of a draw distance several times
/// the classic one (the land runs to the horizon); this backend draws the classic
/// plan (the fog ends at the far plane, 14,844 units at the default), so
/// the record's start and density are expressed over the shorter range:
/// `start * VIEW_SCALE`, `density / VIEW_SCALE`.
pub const VIEW_SCALE: f32 = 1.0;

/// One square's scattering block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scattering {
    pub out: [f32; 3],
    pub inscatter: [f32; 3],
    pub tint: [f32; 3],
    pub params: [f32; 4],
    /// The in-scattering's HDR scale (the look's; [`INSCATTER_SCALE`] by default).
    pub inscatter_scale: f32,
}

impl Default for Scattering {
    fn default() -> Self {
        Self {
            out: DEF_OUT,
            inscatter: DEF_IN,
            tint: DEF_TINT,
            params: DEF_PARAMS,
            inscatter_scale: INSCATTER_SCALE,
        }
    }
}

impl Scattering {
    /// From a file-6 record's `(scattering_params, [tint, out, in])`.
    #[must_use]
    pub fn from_record(params: [f32; 4], colours: [[f32; 3]; 3]) -> Self {
        Self {
            out: colours[1],
            inscatter: colours[2],
            tint: colours[0],
            params,
            inscatter_scale: INSCATTER_SCALE,
        }
    }

    /// This block with the look's in-scattering scale.
    #[must_use]
    pub fn with_inscatter_scale(self, inscatter_scale: f32) -> Self {
        Self {
            inscatter_scale,
            ..self
        }
    }

    /// Per field linear blend (the classic renderer fades each field over the transition).
    #[must_use]
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let l = |a: f32, b: f32| a + (b - a) * t;
        let l3 = |a: [f32; 3], b: [f32; 3]| [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2])];
        Self {
            out: l3(self.out, other.out),
            inscatter: l3(self.inscatter, other.inscatter),
            tint: l3(self.tint, other.tint),
            params: [0, 1, 2, 3].map(|i| l(self.params[i], other.params[i])),
            inscatter_scale: l(self.inscatter_scale, other.inscatter_scale),
        }
    }

    /// The shader's law in this backend's units: `(density per fine unit,
    /// start distance, in-scattering scale)` (module docs, the chosen
    /// constants).
    #[must_use]
    pub fn law(&self) -> [f32; 3] {
        let view = VIEW_SCALE;
        [
            self.params[0] * DENSITY_SCALE / view,
            self.params[1] * view,
            self.inscatter_scale,
        ]
    }
}

/// The scattering of this frame: the camera square's record, faded per
/// field over `crate::lighting::environment::TURN_MS` on the logic clock when it
/// changes (the classic renderer fades each field).
#[derive(Debug, Default)]
pub struct AtmosphereState {
    from: Option<Scattering>,
    to: Option<Scattering>,
    start_ms: i64,
}

impl AtmosphereState {
    /// This frame's scattering for the square's `target` at `now_ms`.
    pub fn update(&mut self, target: Scattering, now_ms: i64) -> Scattering {
        let current = self.at(now_ms);
        if self.to != Some(target) {
            self.from = Some(current.unwrap_or(target));
            self.to = Some(target);
            self.start_ms = now_ms;
        }
        self.at(now_ms).unwrap_or(target)
    }

    fn at(&self, now_ms: i64) -> Option<Scattering> {
        let (from, to) = (self.from?, self.to?);
        let t = ((now_ms - self.start_ms) as f32 / crate::lighting::environment::TURN_MS as f32)
            .clamp(0.0, 1.0);
        Some(from.lerp(&to, t))
    }
}

/// Three 0..1 channels as 8-bit values in an f32's bits with the exponent
/// kept normal (`0x3F` in the top byte: the value lies in [0.5, 2)).
#[must_use]
pub fn pack_colour(c: [f32; 3]) -> f32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    f32::from_bits(0x3F00_0000 | (q(c[0]) << 16) | (q(c[1]) << 8) | q(c[2]))
}

/// [`pack_colour`]'s inverse (what the WGSL's `atmos_unpack` reads).
#[must_use]
pub fn unpack_colour(v: f32) -> [f32; 3] {
    let u = v.to_bits();
    [(u >> 16) & 255, (u >> 8) & 255, u & 255].map(|c| c as f32 / 255.0)
}

/// The scattering in the `Frame` block's free members (module docs), each
/// the `w` (index 3) of its vector: `sun_dir` out, `sun_colour` in,
/// `sky_ambient` tint, `ground_ambient` density; `fog_range` `z` start and
/// `w` the in-scattering scale. The scattering is on where the density
/// (`ground_ambient.w`) is above 0; `params.w` is the modern look's flag
/// (`crate::lighting::look::FRAME_FLAG`), so this block must not touch it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Packed {
    pub sun_dir_w: f32,
    pub sun_colour_w: f32,
    pub sky_ambient_w: f32,
    pub ground_ambient_w: f32,
    pub fog_range_zw: [f32; 2],
}

/// [`Packed`] for `s`.
#[must_use]
pub fn pack(s: &Scattering) -> Packed {
    let [density, start, scale] = s.law();
    Packed {
        sun_dir_w: pack_colour(s.out),
        sun_colour_w: pack_colour(s.inscatter),
        sky_ambient_w: pack_colour(s.tint),
        ground_ambient_w: density,
        fog_range_zw: [start, scale],
    }
}

/// CPU mirror of the in/out-scattering law (module docs) with the packed
/// law: `(extinction, in-scattering)` for the unit view direction `dir`,
/// eye distance `dist` and the direction towards the sun `sun`.
#[must_use]
pub fn in_out_scattering(
    s: &Scattering,
    dir: [f32; 3],
    dist: f32,
    sun: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    let [density, start, scale] = s.law();
    let q = |c: [f32; 3]| unpack_colour(pack_colour(c));
    let (out, inscatter, tint) = (q(s.out), q(s.inscatter), q(s.tint));
    let t = (dist - start).max(0.0);
    let p = (dir[0] * sun[0] + dir[1] * sun[1] + dir[2] * sun[2])
        .max(0.0)
        .powi(2);
    let m = -t * density;
    let ext = out.map(|o| (m * o).exp());
    let ins = [0, 1, 2].map(|i| {
        let u = tint[i] + (1.0 - tint[i]) * p;
        inscatter[i] * scale * (1.0 - (m * u).exp())
    });
    (ext, ins)
}

#[cfg(test)]
mod tests;

pub mod fog;
pub mod sky;
pub mod sky_fade;
pub mod volumetrics;
