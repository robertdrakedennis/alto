//! Three look regressions of the final tour against the faithful frames, each traced to one input
//! and fixed there.
//!
//! 1. **The fog colour** (Falador's blue haze). The modern client fogs as `c * (1 - f) +
//!    fog_colour * f` in linear HDR, with the fog colour supplied by the CPU side. The classic
//!    renderer fills it from the environment's fog colour decoded with the gamma-correct
//!    `powf(c, 2.2)`; its fog range is the classic one (`end = view distance * 256`,
//!    `start = end * (1 - (4 depth + 1024) / 14844)`: at the classic far plane of 14,844 units
//!    exactly the classic toolkit's `far - ((depth + 256) << 2)`). This backend put the classic
//!    fog colour into the HDR target through the *inverse tonemap* (the rule for the clear
//!    colour, which is shown as is). That is right for an unblended background but not for a
//!    colour the fog blends towards: a display channel near 1 (Falador's fog is the colour
//!    9bbfff in hex, blue 255) maps to about 2.2 in HDR, so a surface 20% into the fog already showed its blue
//!    channel at 0.76 where the faithful display blend shows 0.48. The fog colour is now the
//!    decode times a scale ([`fog_colour`], [`FOG_COLOUR_SCALE`] 0.7): the classic renderer
//!    halves the decode relative to its own exposure; this backend's exposure shows a decoded
//!    display colour as itself, so the scale is calibrated instead, against the faithful display
//!    blend over Falador's fogged pixels (the table at the constant; a linear blend of the full
//!    decode is brighter than the display blend at partial fog, the half decode darker). The
//!    range and the eye-distance law were checked and match both renderers; the clear colour
//!    stays the display colour (a fully fogged surface shows 0.85 of it: the far plane, where
//!    the scene ends).
//! 2. **The probes' sky tint** (a cool, greyer ground). The probe ambient is `look sky ambient *
//!    SH(n) / luminance(global SH(up))`: the modern client's ambient colour times the
//!    second-order SH lighting of the map square, with this crate's normalisation for the
//!    ambient scale. The captured SH is of display values and up-facing it is mostly the sky's
//!    colour, so the luminance normalisation left the sky's chroma on every surface on top of
//!    the environment's own ambient colour (the sun colour, which the look calibration already
//!    carries): measured ungraded, the ground's hue moved +6 to +10 degrees and lost 0.05 to
//!    0.08 saturation against the probes-off and faithful frames. The normalisation is now per
//!    channel ([`PROBES_NORMALISED_PER_CHANNEL`]): open ground facing up keeps the look's
//!    ambient colour exactly, and what the probes add is relative to the open sky (occlusion,
//!    ground bounce, colour bleeding).
//! 3. **Unlit materials** (Draynor's green window). The classic renderer draws a model face
//!    whose material has effect 6 with its unlit program (floors likewise, via
//!    `rs910_model::material::MaterialSpec::program`): the texture times the vertex colour, no
//!    sun or ambient. This backend lit them, so the lit window (material 1429, effect 6) was as
//!    dark as the wall at night and the night grade turned its dim yellow green. The modern
//!    client has the same switch as a batch flag (`o = mix(o, albedo, G)`), which the RT5/RT7
//!    branches of this crate honour as instance flag 4; the effect-6 materials now set it
//!    ([`material_flags`]), and the terrain takes the unlit share of its three layers
//!    ([`terrain_wgsl`]). 973 materials of the pack have effect 6 (windows, lava, glowing
//!    floors: 21 overlay and underlay types).
//!
/// The scale of the decoded fog colour (the classic renderer uses 0.5 at its own exposure).
/// Calibrated at Falador
/// (`look2_tests::falador_fog_adds_what_the_display_blend_adds`): the mean
/// display error (mean over the channels) of the fogged colour against
/// the faithful display blend over 308k fogged pixels is 0.052 at 0.5,
/// 0.039 at 0.6, 0.034 at 0.7, 0.036 at 0.8 and 0.058 at 1.0, against
/// 0.194 with the old inverse-tonemapped colour. A fully fogged surface
/// shows the fog colour at `0.7^(1/2.2)` = 0.85 of its display value.
pub const FOG_COLOUR_SCALE: f32 = 0.7;

/// The HDR fog colour the forward passes blend towards, from the classic fog
/// colour `display` (0..1): the classic `powf(c, 2.2)` decode times
/// [`FOG_COLOUR_SCALE`] (module docs, fix 1); with the fix off, the inverse
/// tonemap of the decode (the colour the clear shows).
#[must_use]
pub fn fog_colour(display: [f32; 3]) -> [f32; 3] {
    display.map(|c| FOG_COLOUR_SCALE * crate::post::tonemap::display_to_linear(c.clamp(0.0, 1.0)))
}

/// The instance flag the forward programs read as unlit (`frame`'s
/// `FLAG_UNLIT`, `shaders::FORWARD_WGSL` `p0.w & 4`).
pub const FLAG_UNLIT: u32 = 4;

/// The classic unlit program choice for a lit draw: the material's effect 6
/// (`MaterialSpec::program`).
#[must_use]
pub fn draws_unlit(material: &crate::texture::Material) -> bool {
    material.effect == 6
}

/// The instance flags this lane adds for `material` (fix 3).
#[must_use]
pub fn material_flags(material: Option<&crate::texture::Material>) -> u32 {
    match material {
        Some(m) if draws_unlit(m) => FLAG_UNLIT,
        _ => 0,
    }
}

/// The terrain layer's property value for an unlit material (the table's
/// `params.w`, otherwise 0: see [`terrain_wgsl`]).
pub const TERRAIN_UNLIT: f32 = -1.0;
