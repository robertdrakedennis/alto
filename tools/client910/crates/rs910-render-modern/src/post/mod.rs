//! The modern renderer's post chain: SSAO on the ambient term, eye adaptation, bloom, the tonemap
//! and grading composite, FXAA. The CPU half: which effects run, their constants, the uniform
//! blocks and the CPU mirrors of the shader maths (tests). The GPU half is `crate::frame::post`;
//! the WGSL is [`wgsl`].
//!
//! # The chain
//!
//! After the forward pass, per frame:
//!
//! 1. **Scene luminance**: a 3x3 box of the frame into a 64x64 target holding
//!    `log(luminance(rgb))`, then 4x4 boxes (16 taps at +-0.5/+-1.5 texels, `* 0.0625`) down to
//!    16, 4 and 1 texels, the last level `exp(avg)`: the scene's geometric-mean luminance.
//! 2. **Adaptation** into a 1x1 target, swapping the current and previous targets each frame:
//!    `max(0.001, adapted(curr, prev, dt, 1 / exposure speed))`; both targets are cleared once
//!    after (re)creation.
//! 3. **Bright pass** into a target at the size of the chosen downsample level (or full size),
//!    then a **Kawase blur** (kernels of 7x7 to 127x127, 2-10 passes) or a separable 13-tap
//!    Gaussian, then the **composite**: the bloom added in HDR (`f += bloom * scale`), the tone
//!    map with its operator, a clamp, the sRGB-style `sqrt` encode, then the colour correction
//!    (the environment's remap LUTs). So bloom, tone map, grading, in that order (the grading
//!    stays after the tone map).
//! 4. **FXAA** over the composite when the forward target has one sample.
//!
//! The modern client's defaults: min black 0.0, max white 5.25, exposure key 2.75, auto
//! exposure 0.0..5.0, exposure speed 0.5, eye adaptation on, Kawase blur on at quality 3, bright
//! threshold 1.35, bloom scale 1.0, tone mapping on with operator 0 (advanced RGB Reinhard),
//! FXAA off at quality 1. Its Bloom option (default 2) picks 1: Kawase quality 3, downsample 0;
//! 2: quality 4, downsample 0; 3: quality 5, full size. Its antialiasing mode (default 2) is 0
//! none, 1 FXAA, 2 MSAA, 3 MSAA and FXAA; FXAA quality 0/1/2/3 is preset 10/15/25/39 with subpix
//! 0.75, edge threshold 0.166 and edge threshold min 0.0833. Depth of field sits behind an
//! effect bit that no option or call site sets in normal play, so it is not part of this chain.
//!
//! Shader maths the chain relies on: the adaptation law is
//! `mix(prev, curr, 1 - exp(-dt / (tau * speedMul)))` with the rods' time constant 0.2 s and the
//! cones' 0.4 s (`speedMul` slows it); operator 0 multiplies the luminance above the black
//! level by `key / avgLum`, clamps it to the auto exposure range and passes it through the
//! extended Reinhard curve, and the colour is multiplied by that value; the SSAO estimator is a
//! screen-space one (a ring of taps at `radius / z` pixels, the count falling with depth,
//! `max(0, dot(x, n) - z * bias) / |x|^2`), not a hemisphere; FXAA reads luma from green.
//!
//! # This backend (see each item for the choice made)
//!
//! - **SSAO**: the frame's opaque entities and floors into a 1x face normal/position target,
//!   the screen-space estimator ([`SSAO_RADIUS`], [`SSAO_INTENSITY`], [`SSAO_BIAS`], the sample
//!   counts: this crate's values, the modern client sets them in native code; the tap
//!   directions [`ssao_directions`] stand in for its noise texture), the X and Y depth-weighted
//!   7-tap blur, then the forward pass multiplies its hemisphere ambient by it: the ambient
//!   only, as the modern client's terrain and model programs do. The occlusion itself is the
//!   modern client's AO modes ([`ao`]; `ModernSettings::ao`).
//! - **Eye adaptation**: the measurement chain above, the adaptation law ([`adapt`]) at the
//!   modern client's speed ([`EXPOSURE_SPEED`]); the time step is the **logic clock's**
//!   (`logic_clock`), so fixed-clock frames repeat. The exposure applied is this backend's
//!   `clamp(key / adapted, min, max)` ([`exposure`]): the tonemap is calibrated so that exposure
//!   1 shows the faithful colours, and the classic renderer has no auto exposure, so
//!   [`EXPOSURE_MAX`] is 1 (a dark scene is never lifted above the classic look) and
//!   [`EXPOSURE_KEY`] keeps the calibrated daytime views at 1; only scenes brighter than those
//!   are pulled down (to [`EXPOSURE_MIN`]).
//! - **Tone mapping**: this backend's calibrated curve by default. The modern operator with its
//!   `sqrt` encode ([`reinhard_adv_rgb`], the `reinhard` switch) is ported: its output scales
//!   with the scene's absolute HDR level (`colour * curve`), which the modern client sets with
//!   its own lighting intensities, and on this backend's classically calibrated HDR it darkens
//!   and desaturates the frames against the faithful ones, so it is not the default. Measured
//!   on six offline views: the whole frames' mean value lands within 0.012 of the faithful ones
//!   (the calibrated curve is 0.03-0.06 brighter) and the best-fitting exposure is 0.95-1.05 of
//!   the operator's key; but lit ground loses its hue (the river bank: faithful `(51, 43, 25)`,
//!   calibrated `(56, 57, 48)`, the operator `(38, 39, 40)`). Without the grading the calibrated
//!   curve gives the faithful bank exactly `(52, 43, 25)` and the operator keeps the hue at 0.7
//!   of the brightness `(36, 29, 16)`: the operator's linear display response puts the darker
//!   midtones through the environment's levels and remap LUTs, which turn them grey-blue. A key
//!   that restored the midtones (about twice the operator's) would overexpose the sky and the
//!   whole-frame match, so no exposure recalibration fixes it; it stays behind the switch until
//!   the grading is re-derived for its response.
//! - **Bloom**: the bright pass at the modern threshold ([`BLOOM_THRESHOLD`], in this backend's
//!   HDR scale where 1.0 is a white display colour, so only over-white light blooms: the
//!   `HDR_SCALE` glows and bright specular), the default Bloom option (medium: half size,
//!   Kawase quality 4, the 63x63 kernel [`KAWASE_63`]) and scale, added in the composite. The
//!   classic `bloom` preference as the faithful toolkit applies it (the shell's scene bloom
//!   state) turns it on and off; capability answers are the faithful profile's.
//! - **Glows**: with the chain on, model billboards blend with their coverage squared (the
//!   modern billboard program squares texel times colour, alpha included): glows blended in
//!   linear light with the plain coverage were about twice the faithful brightness; squared
//!   coverage in linear light is the display-space blend over dark ground.
//! - **FXAA**: the FXAA 3.11 quality path after the composite, on the display-referred frame,
//!   when the forward target has one sample. The backend's MSAA follows the classic
//!   `antiAliasing` level (the shell rebuilds the modern renderer when the faithful sample count
//!   changes: level 0 is 1x plus FXAA, a level with MSAA is the modern MSAA mode at that count);
//!   quality 1 (the default: preset 15, eight search steps) with the modern thresholds.
//! - **Grading order**: bloom (in HDR), tonemap, display encode, then the classic levels and
//!   remapping (the modern composite has no levels; the classic ones are kept).
//! - **Depth of field**: not part of this chain (see [`dof`]).

/// Which optional post effects a frame runs (the SSAO follows the AO mode;
/// eye adaptation and the composite operator always run).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub bloom: bool,
    pub fxaa: bool,
}

impl Effects {
    /// This frame's effects: the settings', else the faithful toolkit's
    /// bloom state and FXAA with a single-sample forward target (module
    /// docs).
    #[must_use]
    pub fn resolve(
        settings: &crate::settings::ModernSettings,
        faithful_bloom: bool,
        samples: u32,
    ) -> Self {
        Self {
            bloom: settings.bloom.unwrap_or(faithful_bloom),
            fxaa: settings.fxaa.unwrap_or(samples <= 1),
        }
    }
}

// ---- Eye adaptation -------------------------------------------------------

/// The 64x64 log-luminance target's edge (each later level is a quarter of
/// the previous edge).
pub const LUMINANCE_SIZE: u32 = 64;
/// The luminance chain's levels after the first (16, 4, 1).
pub const LUMINANCE_LEVELS: [u32; 3] = [16, 4, 1];
/// The luminance weights.
pub const LUMA: [f32; 3] = [0.2125, 0.7154, 0.0721];
/// The smallest valid luminance: added to and the floor of every luminance,
/// and the floor of the adapted value.
pub const MIN_VALID_LUMINANCE: f32 = 1e-5;
/// The modern client's default exposure speed (0.5); the shader's speed
/// multiplier is its inverse.
pub const EXPOSURE_SPEED: f32 = 0.5;
/// The adapted luminance at which this backend's exposure is 1 (module
/// docs): above the geometric-mean luminance of the calibrated daytime
/// views (the four fixed-clock offline tiles measure 0.018-0.040,
/// `calibration_views_keep_exposure_one`), so those keep exposure 1.
pub const EXPOSURE_KEY: f32 = 0.08;
/// This backend's exposure clamp (module docs).
pub const EXPOSURE_MIN: f32 = 0.5;
pub const EXPOSURE_MAX: f32 = 1.0;
/// The largest time step one frame adapts by (a frame after a long stall
/// moves at most this far; seconds).
pub const MAX_TIME_STEP: f32 = 0.25;

/// The luminance of a colour: Rec. 709-style weights
/// plus `MIN_VALID_LUMINANCE`, floored at it.
#[must_use]
pub fn luminance(rgb: [f32; 3]) -> f32 {
    (rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2] + MIN_VALID_LUMINANCE)
        .max(MIN_VALID_LUMINANCE)
}

/// One adaptation step (the modern client's rods-and-cones law): the rods' share `0.04 / (0.04 + curr)` blends a
/// 0.2 s rod time constant with a 0.4 s cone one, the step is
/// `mix(prev, curr, 1 - exp(-dt / (tau * speedMul)))`, floored at
/// `MIN_VALID_LUMINANCE` (twice, as the shader does); `dt` in seconds.
#[must_use]
pub fn adapt(prev: f32, curr: f32, dt: f32) -> f32 {
    let rods = 0.04 / (0.04 + curr);
    let tau = rods * 0.2 + (1.0 - rods) * 0.4;
    let speed_mul = 1.0 / EXPOSURE_SPEED;
    let k = (1.0 - (-(dt / (tau * speed_mul))).exp()).clamp(0.0, 1.0);
    let next = prev + (curr - prev) * k;
    next.max(MIN_VALID_LUMINANCE).max(MIN_VALID_LUMINANCE)
}

/// This backend's exposure for an adapted luminance (module docs).
#[must_use]
pub fn exposure(adapted: f32) -> f32 {
    (EXPOSURE_KEY / adapted.max(MIN_VALID_LUMINANCE)).clamp(EXPOSURE_MIN, EXPOSURE_MAX)
}

// ---- The modern tone mapping (the `reinhard` switch) -----------------------

/// The tone-map defaults of the modern client's post configuration: min
/// black luminance, max white luminance, exposure key, min and max auto
/// exposure.
pub const MIN_BLACK_LUM: f32 = 0.0;
pub const MAX_WHITE_LUM: f32 = 5.25;
pub const REINHARD_KEY: f32 = 2.75;
pub const MIN_AUTO_EXPOSURE: f32 = 0.0;
pub const MAX_AUTO_EXPOSURE: f32 = 5.0;

// ---- The environment record's tone map ---------------------------------
//
// Proven for the modern client: the world's tone-map values are the
// environment record's (copied into the post configuration and uploaded every
// frame); the post initialiser's configuration and the record's default
// constructor both hold the values below, which a square without a record
// keeps.

/// The min black luminance, max white luminance, exposure key and min and max
/// auto exposure of the default environment record.
pub const RECORD_MIN_BLACK_LUM: f32 = 0.01;
pub const RECORD_MAX_WHITE_LUM: f32 = 1.5;
pub const RECORD_EXPOSURE_KEY: f32 = 0.55;
pub const RECORD_MIN_AUTO_EXPOSURE: f32 = 0.0;
pub const RECORD_MAX_AUTO_EXPOSURE: f32 = 3.0;

/// The tone-map operator by the configuration's index: 0 `REINHARD_ADVANCED_RGB`,
/// 3 `FILMIC_UNCHARTED2`, 4 `FILMIC_HABLE` (the three the 910 records
/// select); 1, 2 and 5 (`REINHARD_ADVANCED_LUM`, `FILMIC_HEJL_DAWSON`,
/// `ACES_APPROX`) are not in the 910 data and are not ported.
pub const OPERATOR_REINHARD_ADVANCED_RGB: i32 = 0;
pub const OPERATOR_FILMIC_UNCHARTED2: i32 = 3;
pub const OPERATOR_FILMIC_HABLE: i32 = 4;
/// No `TONE_MAPPING_ENABLED` (the record's flag clear): the colour is only
/// clamped.
pub const OPERATOR_NONE: i32 = -1;
/// The default operator (the record default and the post initialiser's, both
/// 3).
pub const RECORD_OPERATOR: i32 = OPERATOR_FILMIC_UNCHARTED2;
/// The environment's transition time when the camera's square changes
/// (5000 ms unless the record's owner sets one).
pub const TONE_MAP_FADE_MS: i64 = 5000;

/// The Uncharted 2 and Hable filmic curve constants `(A, B, C, D, E, F)`.
pub const UNCHARTED2: [f32; 6] = [0.22, 0.3, 0.1, 0.2, 0.01, 0.3];
pub const HABLE: [f32; 6] = [0.15, 0.5, 0.1, 0.2, 0.02, 0.3];

/// The filmic curve `(x (A x + C B) + D E) / (x (A x + B) + D F) - E / F`.
#[must_use]
pub fn filmic_op(x: f32, k: [f32; 6]) -> f32 {
    let [a, b, c, d, e, f] = k;
    (x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f) - e / f
}

/// The tone map's exposure limits.
#[derive(Clone, Copy, Debug)]
pub struct ExposureLimits {
    pub min_black: f32,
    pub max_white: f32,
    pub key: f32,
    pub min_auto: f32,
    pub max_auto: f32,
}

/// The filmic tone map: the exposed colour (the colour times
/// `clamp(key / avg, min_auto, max_auto)`), then the curve over the curve at
/// the white point, minus the curve at the black level.
#[must_use]
pub fn filmic_tone_map(v: [f32; 3], avg: f32, limits: &ExposureLimits, k: [f32; 6]) -> [f32; 3] {
    let ExposureLimits {
        min_black,
        max_white,
        key,
        min_auto,
        max_auto,
    } = *limits;
    let e = (key / avg).clamp(min_auto, max_auto);
    let white = filmic_op(max_white, k);
    let black = filmic_op(min_black, k);
    v.map(|x| filmic_op(x * e, k) / white - black)
}

/// The advanced RGB Reinhard operator (operator 0, the modern client's
/// default configuration): the luminance above
/// the black level, scaled by `key / avgLum` and clamped to the auto
/// exposure range, through the extended Reinhard curve; the colour is
/// multiplied by that value (not divided by its luminance: the shader's
/// own form).
#[must_use]
pub fn reinhard_adv_rgb(v: [f32; 3], avg_lum: f32) -> [f32; 3] {
    let r = (luminance(v) - MIN_BLACK_LUM).max(0.0);
    let c = (r * (REINHARD_KEY / avg_lum)).clamp(MIN_AUTO_EXPOSURE, MAX_AUTO_EXPOSURE);
    let p = c * (1.0 + c / (MAX_WHITE_LUM * MAX_WHITE_LUM)) / (1.0 + c);
    v.map(|x| x * p)
}

// ---- Bloom ----------------------------------------------------------------

/// The modern bright-pass threshold (1.35), on the scene's HDR
/// luminance (1.0: a white display colour in this backend, `post::tonemap`).
pub const BLOOM_THRESHOLD: f32 = 1.35;
/// The modern bloom scale (1.0).
pub const BLOOM_SCALE: f32 = 1.0;
/// The published Kawase iteration sets (Intel's "An investigation of fast
/// real-time GPU-based image blur algorithms", 2014) that the modern
/// kernels are named after, with its pass counts (2, 3, 4, 5, 7, 10
/// passes); each pass is a Kawase filter (four taps at `(iteration + 0.5)`
/// texels diagonally).
pub const KAWASE_35: [u32; 5] = [0, 1, 2, 2, 3];
pub const KAWASE_63: [u32; 7] = [0, 1, 2, 3, 4, 4, 5];
pub const KAWASE_127: [u32; 10] = [0, 1, 2, 3, 4, 5, 7, 8, 9, 10];
/// The bloom's Kawase passes: the default Bloom option (2, medium).
pub const BLOOM_KERNEL: &[u32] = &KAWASE_63;
/// The bright pass's downsample (the Bloom option 2's: level 0, half size).
pub const BLOOM_DOWNSAMPLE: u32 = 2;

/// The bright pass (without the tone-mapped variant, which the modern
/// client's bright-pass filter does not enable): a colour whose luminance passes [`BLOOM_THRESHOLD`] goes through
/// whole, clamped to [`MAX_AUTO_EXPOSURE`]; anything else is black.
#[must_use]
pub fn bright(rgb: [f32; 3]) -> [f32; 3] {
    if luminance(rgb) > BLOOM_THRESHOLD {
        rgb.map(|c| c.clamp(0.0, MAX_AUTO_EXPOSURE))
    } else {
        [0.0; 3]
    }
}

// ---- SSAO ------------------------------------------------------------------

/// The sample count near the eye.
pub const SSAO_BASE_SAMPLES: f32 = 16.0;
/// The fewest samples far away.
pub const SSAO_MIN_SAMPLES: f32 = 6.0;
/// The far distance of the sample-count falloff `base * zFar / (zFar + base * z)`
/// (scene fine units).
pub const SSAO_Z_FAR: f32 = 120_000.0;
/// The sampling radius in scene fine units (the shader's radius is this times
/// the projection's pixel scale, so the screen radius is that over `z`).
pub const SSAO_RADIUS: f32 = 96.0;
/// The occlusion's intensity (scene units: the estimator sums
/// `max(0, dot(x, n) - bias) / |x|^2`, an inverse length).
pub const SSAO_INTENSITY: f32 = 24.0;
/// The depth-proportional bias of `dot(x, n)`.
pub const SSAO_BIAS: f32 = 0.002;
/// The blur's depth sharpness (per scene unit; a 16-unit step halves a tap's weight's exponent budget).
pub const SSAO_BLUR_SHARPNESS: f32 = 1.0 / 16.0;
/// Most samples a pixel takes.
pub const SSAO_SAMPLES: usize = 16;

/// The screen-space sample directions, one per sample: the modern shader reads
/// them from its noise texture along a fixed line (the same for every pixel)
/// and normalises them; this backend uses the golden angle (the noise
/// texture's content is not known).
#[must_use]
pub fn ssao_directions() -> [[f32; 4]; SSAO_SAMPLES] {
    let mut out = [[0.0; 4]; SSAO_SAMPLES];
    for (i, d) in out.iter_mut().enumerate() {
        let a = i as f32 * 2.399_963;
        *d = [a.cos(), a.sin(), 0.0, 0.0];
    }
    out
}

// ---- FXAA -----------------------------------------------------------------

/// The modern client's FXAA defaults.
pub const FXAA_SUBPIX: f32 = 0.75;
pub const FXAA_EDGE_THRESHOLD: f32 = 0.166;
pub const FXAA_EDGE_THRESHOLD_MIN: f32 = 0.0833;

// ---- Uniforms -------------------------------------------------------------

/// The post passes' `PostFrame` uniform block ([`wgsl`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PostFrame {
    /// The scene viewport `x, y, w, h` in frame pixels.
    pub rect: [f32; 4],
    /// Its scissor `l, t, r, b` (within the frame).
    pub clip: [f32; 4],
    /// The bloom targets' size `w, h` and the luminance grid's cell `w, h`.
    pub sizes: [f32; 4],
    /// SSAO: radius in pixels at unit depth, intensity,
    /// bias, base samples.
    pub ssao: [f32; 4],
    /// SSAO: z far, min samples, blur sharpness, -.
    pub ssao2: [f32; 4],
    pub directions: [[f32; 4]; SSAO_SAMPLES],
    /// Bloom: threshold, scale, on, -.
    pub bloom: [f32; 4],
    /// Exposure: key, min, max, adaptation on.
    pub exposure: [f32; 4],
    /// Adaptation: time step (s), speed multiplier, reset (1: take the
    /// current luminance), -.
    pub adapt: [f32; 4],
    /// The modern tone map: min black, max white, key, min auto exposure.
    pub tonemap: [f32; 4],
    /// Max auto exposure, 1 = the modern operator (`reinhard`), the scene's
    /// light unit (`crate::lighting::look::Look::scale`), and the operator index
    /// (`OPERATOR_*`; 0 before the lane).
    pub tonemap2: [f32; 4],
    /// FXAA: subpix, edge threshold, edge threshold min, FXAA follows.
    pub fxaa: [f32; 4],
}

/// One pass's `Pass` block (dynamic offset: Kawase iteration, blur
/// direction, the luminance level's flags).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PassParams {
    pub p: [f32; 4],
    pub pad: [[f32; 4]; 15],
}

impl PassParams {
    #[must_use]
    pub fn new(p: [f32; 4]) -> Self {
        Self {
            p,
            pad: [[0.0; 4]; 15],
        }
    }
}

#[cfg(test)]
mod tests;

pub mod ao;
pub mod dof;
pub mod grading;
pub mod tonemap;
