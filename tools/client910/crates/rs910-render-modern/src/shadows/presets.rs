//! The modern client's shadow quality presets, its shadow filter library and
//! its point-light shadows (renderer plan §4(u)).
//!
//! # Grades
//!
//! Every value below is graded: **proven** (read from the modern client's
//! binary or its shader library), **inferred** (a choice this crate makes
//! where neither shows the value) or **calibrated**.
//!
//! ## Sun shadows
//!
//! - **Quality presets** (proven): level 0-4 = 2/3/4/4/4 cascades,
//!   1024/1024/1024/2048/4096-texel tiles (the atlas is twice the tile,
//!   capped at 16384), shadow distance 10000/12500/17500/25000/40000, depth
//!   bias per cascade `-0.006, -0.005, -0.0045, -0.004` (levels 0-2),
//!   `-0.0055, -0.0045, -0.004, -0.0035` (3), `-0.0045, -0.0035, -0.003,
//!   -0.0025` (4); the bias goes into each cascade's texture matrix as its
//!   depth offset. The split near distance 5.0 and λ 0.55 are the same as
//!   the earlier table.
//! - **Filter, selection and fade per level** (proven, desktop branch): LOW
//!   the 2x2 box, MED the approximate 4x4 box (both select the cascade by
//!   bounding sphere, linear fade); HIGH, ULTRA and ULTRA_PLUS the 4x4 box
//!   (best map, smooth fade). The 4x4 approximation's corner taps are
//!   `(-2,-2)`, `(-2,1)`, `(1,-2)`, `(1,1)`, then the other twelve of
//!   `-2..1`; `shadow_filter` equals them.
//! - **Single-tap depth** (proven): `near + (distance - near) * 0.7`; past it
//!   every level uses the 2x2 box.
//! - **Fade** (proven): from `split[n-2] + (split[n-1] - split[n-2]) * 0.8`
//!   to the shadow distance (`crate::shadows::FADE_FROM` was already the same
//!   0.8, chosen).
//! - **Tile extents** (proven): the minimum atlas extents are the tiles inset
//!   by one atlas texel, the maximum extents the whole tiles; selection by
//!   map tests and clamps against the inset tiles, the sphere selection
//!   clamps against the whole tiles.
//! - **Shadow strength** (proven): the mapping strength times the model
//!   batch's receive bit (1 for the terrain and the water), and the sunlight
//!   evaluation skips the lookup for batches with the emissive bit; neither
//!   bit has a 910 source, so every surface receives with a factor of 1.
//! - **Normal bias** (proven): 12 world units. An earlier version used 32,
//!   the value of a terrain shader define that no program reads.
//! - **Not adopted** (the modern client's record has them, their consumer is
//!   not traced): a record word of -10000 (likely the caster pull-back; the
//!   classic table kept -7250 at that place) and a second λ-like 0.55; the
//!   pull-back 7250 stays. The cascades' one-texel viewport inset (each
//!   cascade's viewport is `(1, 1, res - 2, res - 2)` inside its tile) is not
//!   reproduced; the filters' borders keep the taps inside the tile.
//! - **The option mapping** (inferred): the classic-era client passes its
//!   shadow quality option value straight to the quality function and
//!   switches shadows off with a separate option; the modern client's quality
//!   function takes five levels. The 910 client stores `shadowQuality` 0-4
//!   (default 1) and reads it nowhere, so this crate takes the value as the
//!   modern level: 0 LOW, 1 MED (the default), 2 HIGH, 3 ULTRA, 4
//!   ULTRA_PLUS; shadows go off only when neither `sceneryShadows` nor
//!   `characterShadows` casts.
//!
//! ## The filter library
//!
//! Every depth filter the modern client generates, in [`POINT_SHADOW_WGSL`]'s
//! `shadow_filter_on` (by index, [`Filter`]): the single tap, the 2x2 box,
//! the approximate 4x4 box, the 4x4 box, the 8x8 box
//! (`smoothstep(2, 58, total)`) and the 8-tap and 12-tap disk filters
//! (proven: the tap tables are float pairs scaled by 1.75 and rounded away
//! from zero to whole texel offsets by the generator; the 12 taps are the
//! classic 12-tap Poisson disk). An exponential filter (an exponential of a
//! hardware-compare tap minus the depth) is referenced by no quality define
//! and is not ported. The translucency filters need the translucency map,
//! which this renderer does not draw.
//!
//! ## Point-light shadows
//!
//! The decisions (candidates, priority, slot pool, level, atlas layout, face
//! culling) and their grades are in [`crate::shadows::point`]; this module
//! holds the quality tables and the shader block.
//!
//! - **Shader** (proven): the light-to-fragment vector is offset along the
//!   normal by `bias.y * (2 - w)` (`w` the attenuation), then the shadow
//!   attenuation takes the cube face and its atlas rectangle from the
//!   direction with z negated, the reference `d² / r² + bias.z`, the level's
//!   filter (desktop: LOW 2x2 box, MED the 12-tap disk, HIGH the 4x4
//!   approximation, ULTRA and ULTRA_PLUS the 4x4 box), the fade by view
//!   depth (linear LOW/MED, smooth above), `min(1, e + 1 - bias.w)`; only
//!   within the maximum view distance. The caster pass writes `d²` times the
//!   light's inverse squared far clip as the depth.
//! - **Presets** (proven): shadowed lights 2/2/3/4/4, resolution levels
//!   2/3/3/4/4, face size 256/256/512/512/1024 at level 0 (level `k` is
//!   `face >> k`), maximum view distance 10000/12500/16666.7/25000/25000
//!   with the fade from 0.85 of it, the normal bias -50 world units and the
//!   depth bias by level `-0.0045, -0.0095, -0.0145, -0.0195`. The quality is
//!   the sun shadows' quality, and point shadows exist exactly when the sun
//!   shadows do.
//! - **Chosen here** (unproven): the atlas packing, the slot release, the
//!   reuse of faces across frames and the per-light strength (1; the
//!   filter bias's w) are stand-ins; see [`crate::shadows::point`]. The
//!   half-resolution copy of the atlas that the reference keeps for its
//!   volumetric march is not made: this renderer's volumetrics do not march
//!   point lights, so nothing would read it.
//!
//! # Settings
//!
//! There is no separate point-shadow setting: the sun shadows' quality
//! chooses the preset.

use crate::shadows::{Filter as SunFilter, Profile, Quality};

/// The share of the way from the split near distance to the shadow
/// distance past which every level uses the 2x2 box (confirmed-static).
pub const LOW_QUALITY_FRACTION: f32 = 0.7;
/// The receiver's normal bias (proven), world units.
pub const NORMAL_BIAS: f32 = 12.0;
/// How far past a texel the 4x4 filters reach (texels): the taps span -2..1
/// after a half-texel shift plus the bilinear compare's texel.
const REACH_4X4: f64 = 3.5;

/// The preset of `level` (see the module docs).
#[must_use]
pub fn profile(level: Quality) -> Profile {
    let (cascades, resolution, distance, bias) = match level {
        Quality::Low => (2, 1024, 10_000.0, [-0.006, -0.005, -0.0045, -0.004]),
        Quality::Medium => (3, 1024, 12_500.0, [-0.006, -0.005, -0.0045, -0.004]),
        Quality::High => (4, 1024, 17_500.0, [-0.006, -0.005, -0.0045, -0.004]),
        Quality::Ultra => (4, 2048, 25_000.0, [-0.0055, -0.0045, -0.004, -0.0035]),
        Quality::UltraPlus => (4, 4096, 40_000.0, [-0.0045, -0.0035, -0.003, -0.0025]),
    };
    let (filter, select_by_map, smooth_fade) = match level {
        Quality::Low => (SunFilter::Box2x2, false, false),
        Quality::Medium => (SunFilter::Box4x4Approx, false, false),
        _ => (SunFilter::Box4x4, true, true),
    };
    Profile {
        quality: level,
        cascades,
        resolution,
        distance,
        filter,
        select_by_map,
        smooth_fade,
        bias,
        low_quality_fraction: Some(LOW_QUALITY_FRACTION),
        normal_bias: NORMAL_BIAS,
        reach_texels: REACH_4X4,
        // The minimum atlas extents (proven: each tile inset by one atlas
        // texel) where the selection is by map; the whole tile (the maximum
        // extents) for the sphere's clamp.
        extents_inset: if select_by_map { 1.0 } else { 0.0 },
    }
}

/// A filter of [`POINT_SHADOW_WGSL`]'s `shadow_filter_on` (see the module
/// docs, "The filter library").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Box2x2 = 0,
    Box4x4Approx = 1,
    Box4x4 = 2,
    Tap1x1 = 3,
    Box8x8 = 4,
    Disk8 = 5,
    Disk12 = 6,
}

/// One level's point-light shadow parameters (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointPreset {
    pub level: Quality,
    /// Shadowed lights at most (the slot pool's size).
    pub lights: usize,
    /// Resolution levels per slot: level `k` has faces of `face >> k`.
    pub levels: usize,
    /// One cube face's size in texels at level 0.
    pub face: u32,
    /// The maximum view distance: no shadow past this view depth, and no
    /// candidate light whose sphere is farther.
    pub max_view_distance: f32,
    /// The fade to lit starts at this share of the maximum view distance.
    pub fade_from: f32,
    /// The filter bias's y: world units along the normal (scaled by
    /// `2 - w` in the shader).
    pub normal_bias: f32,
    pub filter: Filter,
    /// `POINT_SHADOW_FADE_SMOOTH` (else the linear fade).
    pub smooth_fade: bool,
}

/// The point-shadow depth bias of each resolution level (proven): the lower
/// the resolution, the more bias.
pub const POINT_DEPTH_BIAS: [f32; 4] = [-0.0045, -0.0095, -0.0145, -0.0195];

/// The point-shadow preset of `level` (see the module docs).
#[must_use]
pub fn point_preset(level: Quality) -> PointPreset {
    let (lights, levels, face, max_view_distance) = match level {
        Quality::Low => (2, 2, 256, 10_000.0),
        Quality::Medium => (2, 3, 256, 12_500.0),
        Quality::High => (3, 3, 512, 16_666.666),
        Quality::Ultra => (4, 4, 512, 25_000.0),
        Quality::UltraPlus => (4, 4, 1024, 25_000.0),
    };
    let (filter, smooth_fade) = match level {
        Quality::Low => (Filter::Box2x2, false),
        Quality::Medium => (Filter::Disk12, false),
        Quality::High => (Filter::Box4x4Approx, true),
        Quality::Ultra | Quality::UltraPlus => (Filter::Box4x4, true),
    };
    PointPreset {
        level,
        lights,
        levels,
        face,
        max_view_distance,
        fade_from: 0.85,
        normal_bias: -50.0,
        filter,
        smooth_fade,
    }
}

/// At most this many shadowed lights (the id list is a vec4).
pub const MAX_SHADOWED: usize = 4;

/// One shadowed light of the frame as the shader block holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowedLight {
    /// The light's slot in the frame's light list (1-based, as the forward
    /// pass numbers them).
    pub light_slot: u32,
    /// The resolution level its faces are sampled at.
    pub level: usize,
    /// The atlas texture coordinates of the level's faces
    /// ([`crate::shadows::point::AtlasLayout::uv`]): scale and texel size,
    /// and the six faces' extents.
    pub scale: [f32; 4],
    pub faces: [[f32; 4]; 6],
}

/// One shadowed light's block (the modern client's member order).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PointShadowLight {
    /// The enabled and fade view params: x enabled, y fade start, z fade end, w
    /// `1 / (end - start)`.
    pub fade: [f32; 4],
    /// The filter bias params: x unused, y normal bias, z depth bias, w strength.
    pub bias: [f32; 4],
    /// xy the atlas face UV scale, zw the atlas face UV texel size.
    pub scale: [f32; 4],
    /// The atlas face UV extents of +X, -X, +Y, -Y, +Z, -Z.
    pub faces: [[f32; 4]; 6],
}

/// The point-light shadow block plus the ids and parameters (the shadowed
/// lights' ids and the maximum view distance).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PointShadowUniforms {
    pub lights: [PointShadowLight; MAX_SHADOWED],
    /// The shadowed lights' slots in the frame's light list (1-based, as
    /// the forward pass numbers them; 0 none).
    pub ids: [u32; 4],
    /// x max view distance, y [`Filter`], z 1 = smooth fade, w the shadowed
    /// count (0: the lookup returns lit).
    pub params: [f32; 4],
}

impl PointShadowUniforms {
    /// The block for this frame's shadowed lights under `preset`.
    #[must_use]
    pub fn new(preset: &PointPreset, lights: &[ShadowedLight]) -> Self {
        let mut out = Self::default();
        let end = preset.max_view_distance;
        let start = end * preset.fade_from;
        for (k, l) in lights.iter().take(MAX_SHADOWED).enumerate() {
            out.ids[k] = l.light_slot;
            out.lights[k] = PointShadowLight {
                fade: [1.0, start, end, 1.0 / (end - start)],
                bias: [
                    0.0,
                    preset.normal_bias,
                    POINT_DEPTH_BIAS[l.level.min(POINT_DEPTH_BIAS.len() - 1)],
                    1.0,
                ],
                scale: l.scale,
                faces: l.faces,
            };
        }
        out.params = [
            end,
            preset.filter as u32 as f32,
            if preset.smooth_fade { 1.0 } else { 0.0 },
            lights.len().min(MAX_SHADOWED) as f32,
        ];
        out
    }
}

/// One cube face's caster uniforms (`PointCaster` in the WGSL), in a
/// 256-byte dynamic-offset slot.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PointCasterUniforms {
    /// xyz the light (camera-local), w `1 / r²` (the shadow depth scale).
    pub light: [f32; 4],
    /// x the face (0-5: +X, -X, +Y, -Y, +Z, -Z), y the near distance, z the
    /// far distance (the light's radius).
    pub face: [f32; 4],
    pub pad: [[f32; 4]; 14],
}

impl Default for PointCasterUniforms {
    fn default() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

/// The caster slot size (the dynamic offset alignment).
pub const CASTER_SLOT: u64 = 256;

/// The face (0-5) whose frustum a caster sphere at `d` (from the light,
/// camera-local) with radius `m` can reach: `Vec3To2DCubeMapFaceTexCoords`'
/// axes (the direction with z negated), a 90° frustum per face.
#[must_use]
pub fn faces_met(d: [f32; 3], m: f32) -> u8 {
    let v = [d[0], d[1], -d[2]];
    let mut mask = 0;
    for f in 0..6 {
        let axis = f / 2;
        let sign = if f % 2 == 0 { 1.0 } else { -1.0 };
        let ma = v[axis] * sign;
        let reach = m * std::f32::consts::SQRT_2;
        let inside = (0..3)
            .filter(|&a| a != axis)
            .all(|a| v[a].abs() <= ma + reach);
        if ma + m > 0.0 && inside {
            mask |= 1 << f;
        }
    }
    mask
}
