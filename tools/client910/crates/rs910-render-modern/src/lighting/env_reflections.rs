//! The modern client's RT5 environment mapping: which materials take it, how strongly, and the
//! global cube that feeds it. It replaces an earlier opt-in "effects 1 and 7" guess.
//!
//! - **Which materials**: the RT5 material record's second flag word, bit `0x2`, sets vertex flag
//!   bit 2, the shader's env mask (`fract(p * 16)`; the decoder keeps the word as
//!   `Material::flags2`). In the 910 data 300 materials carry it, all of them classic effect 7;
//!   none of the 2,011 effect-1 materials does.
//! - **The other two bits**: effect 1 sets vertex bit 0, the specular-map factor
//!   `D = 1 + 4 texel alpha`, and effect 6 bit 1, the emissive mix `mix(lit, albedo, G)`.
//!   Effect 7 sets neither. The model programs used to apply `D` to effects 1 and 7 (the classic
//!   "reflective" pair, `crate::models::materials::material_info`); now to effect 1 only
//!   ([`FLAG_SPEC_MAP`]). Effect 6 is already the unlit mix (`FLAG_UNLIT`); its alpha square is
//!   not drawn (unchanged).
//! - **The gate**: unless `TEXTURE_ALBEDO_GLOBAL`, both bits also need vertex bit 3, which the
//!   client sets for a material without the high-detail byte or whose alpha mode is alpha-tested
//!   or blended. The define follows the "Texturing" option (set at 1 and 2, cleared at 0), but
//!   whether the RT5 model program asks for it is not proven, so the env term is on by default
//!   where bit 3 is set whatever the define (flagged materials without the high-detail byte, or
//!   not opaque); the flagged high-detail opaque ones are opt-in
//!   (`ModernSettings::env_reflections`, `CLIENT910_MODERN_ENV_REFLECTIONS=all`); `D` stays
//!   ungated as before.
//! - **Strength**: `albedo = mix(albedo, env * SSAO * h * params.w, g * (0.8 + 0.2 (1 - N.V)^5))`,
//!   `g` the texel alpha, `h` the SH irradiance (always in the base define mask), before the
//!   lighting multiplies the albedo; the alpha becomes 1 (`u.q = min(u.q + s.p, 1)`).
//! - **The cube**: the environment record's global cube, a material id whose cube texture is
//!   loaded (the material's cube flag; its scale float is in no 910 material, so 1),
//!   cross-faded over [`CUBE_FADE_MS`] when the camera square's id changes (here blended at the
//!   sample, not into a 128² target). The modern client reads the id as the s16 after the
//!   skybox, which is 910 file 6's `sampler_material` (byte 184: the same fixed-order sequence,
//!   the only s16 between the skybox and the flag; the classic environment's sampler, the cube
//!   the faithful water reflects). Sampled at level 0 (the filtered variant's shader is not
//!   proven), with the classic cube direction convention (the `o.z = -o.z` of the modern
//!   client's own axes is not mapped), the texel decoded like the albedo's (inferred). Without an
//!   id (the world's default id is not traced) a **stand-in**: the probes' captured cube over
//!   the open ground's light.
//! - **Unproven, stand-ins**: `params.w` (the record's float at `+0x20`; 910 carries one float
//!   where the modern format reads three, which one is not provable; the record default is
//!   2.75): 1 ([`PARAMS_W`]); `h` taken relative to the global probe's up-facing irradiance
//!   (this backend's probes carry absolute light).
//!
//! The setting `ModernSettings::env_reflections` (`CLIENT910_MODERN_
//! ENV_REFLECTIONS=all`) adds the high-detail materials.

/// The RT5 env flag in [`crate::texture::Material::flags2`].
pub const ENV_FLAG: u32 = 0x2;

/// The instance flag of the env term (`water_body::FLAG_ENV_MAP`,
/// `p0.w & 64`).
pub const FLAG_ENV_MAP: u32 = 64;
/// The instance flag of the specular-map factor `D` (vertex bit 0): the
/// model programs apply `D` only with it.
pub const FLAG_SPEC_MAP: u32 = 128;

/// The record's environment mapping parameter (its cache source is not
/// proven): a stand-in 1.
pub const PARAMS_W: f32 = 1.0;

/// The global cube's cross-fade time.
pub const CUBE_FADE_MS: i64 = 5000;

/// The instance flags the modern client's material init gives `material` (module docs);
/// `env`: whether the flagged opaque high-detail materials take the env
/// term too (the unproven gate, [`crate::settings::EnvReflections::All`]).
#[must_use]
pub fn material_flags(
    material: &crate::texture::Material,
    env: crate::settings::EnvReflections,
) -> u32 {
    let high_detail = env == crate::settings::EnvReflections::All;
    let mut flags = 0;
    if material.effect == 1 {
        flags |= FLAG_SPEC_MAP;
    }
    let bit3 = !material.high_detail || material.alpha != crate::texture::AlphaMode::None;
    if material.flags2 & ENV_FLAG != 0 && (high_detail || bit3) {
        flags |= FLAG_ENV_MAP;
    }
    flags
}
