//! The water body's effects (renderer plan §4(l)): the sun's shadow on the
//! water body, shoreline foam and caustics on the bed, each following the
//! modern client's water shading and each behind its own switch. The water
//! pass itself (the surface, reflection, refraction and extinction) is
//! [`crate::water_body`]; this module only adds terms to its body shading,
//! through [`WATER_FX_WGSL`]'s functions and the `fx`/`fx2` words of its
//! uniforms.
//!
//! # Shadow on the water body
//!
//! The modern water fragment takes one sun attenuation `c` (the cascade lookup
//! at the surface point pushed by the normal) and multiplies both the
//! specular and the diffuse by it; the diffuse is `albedo * (dot(n, toSun) *
//! 0.5 + 0.5) * (1 - fresnel) * sun colour * c`, the light of the opaque water
//! colour in the extinction. Lane Q-WATER2's body wrapped the sun with
//! `dot(-n, toSun)`: with the classic normal pointing up (`-y`) that is the
//! light from *below*, so a sun at 45 degrees lit the body at 0.15 instead of
//! 0.85 and the cascades, which scale only that term, barely showed on the
//! river (the ambient, this port's addition, dominated). With the switch on
//! the body takes the modern wrap and the specular's attenuation; the bed
//! colour's gain is recalibrated for the brighter light ([`LOOK`] `bed_gain`:
//! the shallows still meet the bank without a step). The reflection stays
//! unshadowed, as in the modern client (it has no attenuation).
//!
//! # Foam
//!
//! The albedo moves towards the foam map (times the foam scale, rgb times
//! alpha) by `s * q`, where `q = 1 - min(depth / foamDepth, 1)` (1 at the
//! shore, 0 from the foam depth on) and `s = min(1, a + b q) * max(0.2 -
//! |flow|, 0)`, with two travelling bands `a = max(0, cos(0.005 |depth| + t + 4
//! v))^8` and `b = max(0, cos(0.005 |depth| - t + 8 v))^4`, `t` the clock plus
//! `1e-4` of the position's X and Z, `v = clamp(noise(xz / 512) / 8, 0, 1)`.
//! So foam only forms in slow water, at most a fifth of the way to the foam
//! colour: subtle by construction. The 910 data: WATERTYPE op 9 names the
//! foam material (material 3203, a white texture with a varying alpha,
//! `rs910_config::nxt::water_type`), bound at the map-0 repeat as the foam UV;
//! the depth is the water body's shading depth, which Q-WATER2 bounds by the
//! shore distance from the bed synthesis ([`crate::water_body::beds`]), so the
//! foam follows the shorelines. The foam depth and scale are chosen values
//! ([`LOOK`]). The noise is this crate's gradient noise (the modern client's
//! noise field has no 910 source; only its role as a phase is kept).
//!
//! # Caustics
//!
//! The modern client draws caustics in two steps: the water's caustics-compute
//! variant sends one ray per water fragment, offset by `normal.xz * depth *`
//! the refraction scale and weighted `min(fade.x / depth * fade.y, 7 fade.y) *
//! smoothstep(fade.z, fade.w, depth)`, into an integer light map; the bed's
//! terrain and models (`C += sun colour * caustics * shadow` before the
//! albedo) read it back. This renderer draws no bed geometry (the classic
//! floor builds none at the default water detail; the bed is synthesised in
//! the water pass), so the map's density is evaluated where the ray lands in
//! closed form: rays from an evenly lit surface pile up by `1 / det(I + depth
//! * k * grad(n.xz))`, about `1 / (1 + depth * k * div(n.xz))`, and the
//! divergence comes from finite differences of the detail normal map at the
//! fragment. The bright excess over the even light (`max(1 / J - 1, 0)`) under
//! the depth weight is added as sun light (shadowed) to the synthesised bed
//! only. The refraction scale and the fade are chosen values ([`LOOK`]).

/// The switches' bits in the water uniforms' `fx.x`.
pub const FX_SHADOW: u32 = 1;
pub const FX_FOAM: u32 = 2;
pub const FX_CAUSTICS: u32 = 4;

/// The chosen values of the effects (modern-client constants that have no 910
/// source, or this port's calibrations; see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The bed colour's gain with the modern wrapped sun (Q-WATER2's
    /// `bed_brightness` was measured under the inverted wrap; this value
    /// keeps the river view's first water pixel next to the last bank pixel).
    pub bed_gain: f32,
    /// The depth (fine units) where the foam ends.
    pub foam_depth: f32,
    /// The foam map's scale.
    pub foam_scale: f32,
    /// The caustics' strength (the fade's y, the light per ray).
    pub caustics_strength: f32,
    /// The refraction scale: the ray's push per unit of depth and
    /// normal slope.
    pub caustics_refraction: f32,
    /// The fade's z and w: the depths over which the caustics fade in.
    pub caustics_fade_in: [f32; 2],
    /// The fade's x: beyond this depth the per-ray light falls as
    /// `1 / depth` (capped at 7 times the strength).
    pub caustics_falloff: f32,
}

/// See [`Look`].
pub const LOOK: Look = Look {
    bed_gain: 1.1,
    foam_depth: 64.0,
    foam_scale: 1.0,
    caustics_strength: 1.0,
    caustics_refraction: 1.5,
    caustics_fade_in: [4.0, 48.0],
    caustics_falloff: 48.0,
};

/// Every effect on (the `fx.x` bits).
pub const FX_ALL: u32 = FX_SHADOW | FX_FOAM | FX_CAUSTICS;

/// The `fx` and `fx2` words of the water uniforms for `flags`.
#[must_use]
pub fn uniforms(flags: u32) -> [[f32; 4]; 2] {
    let l = LOOK;
    [
        [flags as f32, l.bed_gain, l.foam_depth, l.foam_scale],
        [
            l.caustics_strength,
            l.caustics_refraction,
            l.caustics_fade_in[0],
            l.caustics_fade_in[1],
        ],
    ]
}

/// The foam weight (see the module docs) at shading depth
/// `depth`, flow length `flow`, clock `t` (seconds, the position's phase
/// included) and noise `v`: the share of the foam colour.
#[must_use]
pub fn foam_weight(depth: f32, flow: f32, t: f32, v: f32, foam_depth: f32) -> f32 {
    if foam_depth <= 0.0 {
        return 0.0;
    }
    let q = (1.0 - (depth / foam_depth).min(1.0)).clamp(0.0, 1.0);
    let a = (depth.abs() * 0.005 + t + v * 4.0).cos().max(0.0).powi(8);
    let b = (depth.abs() * 0.005 - t + v * 8.0).cos().max(0.0).powi(4);
    (a + b * q).min(1.0) * (0.2 - flow).max(0.0) * q
}

/// The caustics' bright excess (see the module docs) for normal-slope
/// divergence `div` (per fine unit) at `depth`.
#[must_use]
pub fn caustic_excess(div: f32, depth: f32, look: &Look) -> f32 {
    let j = (1.0 + depth * look.caustics_refraction * div).max(0.2);
    let [a, b] = look.caustics_fade_in;
    let t = ((depth - a) / (b - a)).clamp(0.0, 1.0);
    let fade = t * t * (3.0 - 2.0 * t);
    let per_ray = (look.caustics_falloff / depth.max(1e-3)).min(7.0) * look.caustics_strength;
    per_ray * fade * (1.0 / j - 1.0).max(0.0)
}
