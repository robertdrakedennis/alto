//! The modern renderer's lighting, post-processing and colour pipeline, calibrated against the
//! modern client's own frames instead of the classic renderer's.
//!
//! Earlier versions calibrated this backend so that its frames showed the faithful (classic)
//! colours: a tonemap that is the identity below a knee, an exposure capped at 1, a sun and a
//! two-colour ambient scaled so that a shadowed tile keeps the classic hard-shadow darkening
//! (`crate::lighting::environment::look`), probes normalised to that ambient, the classic floor
//! shade map on the terrain, the classic levels in the grade and a fog colour fitted to the
//! classic display blend. The modern client is a full lighting and post revamp: its frames are
//! the reference for the look, the classic frames only a data check. With this module on (the
//! earlier default) the backend follows the modern client instead:
//!
//! - **Tone mapping, exposure, grade**: the modern composite: the bloom added in HDR, the tone
//!   map with operator 0 (advanced RGB Reinhard; the operator is a client-side config choice
//!   whose default is 0) over the adapted luminance, the clamp, the `sqrt` sRGB-style encode,
//!   then the colour correction (the colour times `1 - Σ w` plus each remap LUT times its
//!   weight). No levels (the modern client has none), no exposure cap (the operator's own auto
//!   exposure, min to max auto exposure on the scaled luminance).
//! - **Lighting**: the sunlight block without this crate's classic balance: sun colour =
//!   decoded colour × diffuse, ambient colour = decoded colour × ambient, each times this
//!   module's calibration of the value the modern client sets in code ([`Look::sun`],
//!   [`Look::ambient`]); one ambient colour (there is no hemisphere: the probes give the
//!   direction).
//! - **Probes**: the ambient colour times the second-order SH map-square lighting, with the
//!   captured SH as it is: the probes carry the scene's own light (no normalisation to the
//!   classic-calibrated sky ambient).
//! - **Fog colour**: `powf(c, 2.2) * 0.5`, not the earlier 0.7 fitted on classic frames.
//! - **Terrain**: no classic floor shade map (`crate::terrain::shade`, absent from the modern
//!   client; interiors darken through SSAO, shadows and probes).
//! - **Values the modern client sets in code**: the exposure key, bloom threshold and strength,
//!   the sun and sky intensities, the SSAO strength and radius: calibrated against the official
//!   modern-client Lumbridge screenshots by region statistics; each constant of
//!   [`Look::CALIBRATED`] names what it is.
//!
//! # Proven values
//!
//! The modern client was traced for the values above that it sets in code. What is proven
//! replaces the calibration in the verified look (`ModernSettings::look`, the
//! default: [`Look::verified`]):
//!
//! - `lights`: sun colour = the record's diffuse times the decoded colour, ambient colour = the
//!   decoded colour times the record's ambient times the Brightness factor, packed into the
//!   sunlight block: no sun, ambient or light unit multiplier ([`Look::sun`],
//!   [`Look::ambient`], [`Look::scale`] 1). The Brightness option's factor `1 + 0.25 (s - 2)`
//!   has no proven relation to the classic brightness preference, so the classic factor inside
//!   `EnvFrame::sun.ambient` stays.
//! - `tonemap`: the world's tone-map bloom values and operator are the camera square's
//!   environment record's (`crate::lighting::environment::ToneMapBlock`; the defaults of
//!   `crate::post::RECORD_*` without one), and the default operator is the filmic Uncharted 2
//!   (index 3), which the 910 records select almost everywhere: the composite takes the filmic
//!   tone map ([`post_wgsl`]).
//! - `grade`: the colour remap weightings are `(1 - Σw, w)` with the data weights
//!   ([`Look::grade`] 1).
//! - `ambient`: the ambient is the per-square irradiance block captured from the world
//!   ([`crate::lighting::ambient`]), not the zone probes'.
//!
//! Proven to be as they were: the fog colour's half (fog colour = `0.5 × lin(c)`), the bloom
//! threshold and scale. Kept although unproven: the sky's display level (`Look::sky`; the modern
//! client has its own sky), the SSAO values (replaced by the modern AO modes in `post::ao`). A
//! far band is part of the verified look too (`rs910_far_scene::far_level`).
//!
//! What no proof gives is a labelled stand-in, tuned against the look reference by image
//! statistics and nothing else ([`Look::verified`]): the sun gain, the in-scattering scale and
//! the volumetric sky share.

use crate::lighting::environment::Lighting;
use crate::post::tonemap::display_to_linear;
use crate::settings::LookMode;

/// The in-scattering scale of the distance scattering in the earlier look (the
/// scale `crate::atmosphere::Scattering` starts with).
pub const CLASSIC_INSCATTER_SCALE: f32 = 1.5;
/// The volumetric scattering's share on the sky in the earlier look.
pub const CLASSIC_VOLUMETRIC_SKY_SHARE: f32 = 0.5;

/// The values the modern client sets in code that this look uses (module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The sun colour over `decoded colour × diffuse`.
    pub sun: f32,
    /// The ambient colour over `decoded colour × ambient`.
    pub ambient: f32,
    /// The fog colour over the decoded fog colour.
    pub fog: f32,
    /// The tone-map bloom composite's minimum black luminance, maximum
    /// white luminance, exposure key, and minimum and maximum auto exposure.
    pub min_black: f32,
    pub max_white: f32,
    pub key: f32,
    pub min_auto: f32,
    pub max_auto: f32,
    /// The bright pass's brightness threshold and the composite's bloom
    /// scale.
    pub bloom_threshold: f32,
    pub bloom_scale: f32,
    /// The SSAO intensity and the radius in classic fine units (the radius
    /// parameter over the projection's pixel scale).
    pub ssao_intensity: f32,
    pub ssao_radius: f32,
    /// The scene's light unit: every light and emissive input of the HDR
    /// target (sun, ambient, point lights, fog, sky, unlit and HDR-scaled
    /// materials) times this, applied where the post chain reads the
    /// target (the luminance chain, the bright pass, the composite; exactly
    /// the same as scaling each input, the forward shading being linear in
    /// them). The modern client's lights are code values in its own unit; with
    /// this backend's darker classic albedos (HSL colours, classic textures)
    /// the reference's brightness needs a larger unit.
    pub scale: f32,
    /// The sky's intensity: the display-referred sky inputs over their
    /// display value in the composite ([`Self::sky_exposure`]).
    pub sky: f32,
    /// The colour correction's strength: the environment's remap weights
    /// `w` enter the colour remap weightings as `(1 - grade Σw, grade w)`
    /// (the modern client passes `(1 - Σw, w)`).
    pub grade: f32,
    /// The tone map's operator (`crate::post::OPERATOR_*`): 0, the advanced
    /// RGB Reinhard, before the verified look.
    pub operator: i32,
    /// The distance scattering's in-scattering scale (`crate::atmosphere::
    /// Scattering::inscatter_scale`).
    pub scatter_inscatter: f32,
    /// The volumetric scattering's share on the sky
    /// (`crate::atmosphere::volumetrics::uniforms`).
    pub volumetric_sky_share: f32,
}

impl Look {
    /// The modern client's values where it has them (the sunlight
    /// derivation unscaled, the post config defaults, the fog's half decode)
    /// and the earlier SSAO: the starting point of the calibration.
    pub const BASE: Self = Self {
        sun: 1.0,
        ambient: 1.0,
        fog: 0.5,
        min_black: crate::post::MIN_BLACK_LUM,
        max_white: crate::post::MAX_WHITE_LUM,
        key: crate::post::REINHARD_KEY,
        min_auto: crate::post::MIN_AUTO_EXPOSURE,
        max_auto: crate::post::MAX_AUTO_EXPOSURE,
        bloom_threshold: crate::post::BLOOM_THRESHOLD,
        bloom_scale: crate::post::BLOOM_SCALE,
        ssao_intensity: crate::post::SSAO_INTENSITY,
        ssao_radius: crate::post::SSAO_RADIUS,
        scale: 1.0,
        sky: 1.0,
        grade: 1.0,
        operator: crate::post::OPERATOR_REINHARD_ADVANCED_RGB,
        scatter_inscatter: CLASSIC_INSCATTER_SCALE,
        volumetric_sky_share: CLASSIC_VOLUMETRIC_SKY_SHARE,
    };

    /// The calibrated look (region statistics of four Lumbridge framings
    /// against the four modern reference screenshots; the modern tone-map
    /// and bloom constants kept, they fit):
    ///
    /// - `sun` 2, `ambient` 3 (the sky's light over the sun's, as the modern
    ///   probes carry it): the sunlit-to-shadowed luminance ratio of ground
    ///   and stone on screen 1.55 against the reference's 1.44-1.95
    ///   (median 1.6); the plain balance (1, 1) gives 2.0, (2, 2) 1.7;
    /// - `scale` 3: sunlit ground and stone at mean display luminance 0.36,
    ///   shadowed 0.23, against the reference's 0.36 and 0.22;
    /// - `sky` 1.5: the classic sky (Lumbridge's fog colour) at display value
    ///   0.97 against the reference sky's 0.97-0.98 (at 1: 0.72-0.80);
    /// - `ssao_intensity` 192, `ssao_radius` 192 (classic units, 0.375 of a
    ///   tile): the reference's contact occlusion under crenellations,
    ///   ledges and at wall bases; the earlier 24 and 96 darkened 1-4% of the
    ///   pixels by more than 5% (none by 15%), these 5-27% (1-10% by 15%;
    ///   the darkest percentile at 0.65-0.85 of the unoccluded light);
    /// - `grade` 0.25: the Lumbridge remap LUT (a classic-era asset, weight
    ///   1.0) at full strength turns shadowed ground and stone grey-blue
    ///   and green (hue 100-155, saturation 0.1-0.3) where the reference's
    ///   shadows keep the lit colour's warm hue (18-29, 0.23-0.48); a
    ///   quarter keeps its green in the grass and the reference's frame
    ///   saturation (median 0.38 against 0.34).
    pub const CALIBRATED: Self = Self {
        sun: 2.0,
        ambient: 3.0,
        scale: 3.0,
        sky: 1.5,
        grade: 0.25,
        ssao_intensity: 192.0,
        ssao_radius: 192.0,
        ..Self::BASE
    };

    /// This look with the proven values (module docs):
    /// sun, ambient and light unit 1; the default tone-map record (the
    /// camera square's block replaces it each frame,
    /// [`Self::with_tone_map`]); the data weights of the grade.
    #[must_use]
    pub fn proven(self) -> Self {
        let mut l = self;
        l.sun = 1.0;
        l.ambient = 1.0;
        l.scale = 1.0;
        l = l.with_tone_map(&crate::lighting::environment::ToneMapBlock::DEFAULT);
        l.grade = 1.0;
        l
    }

    /// The verified look: the proven values ([`Self::proven`]) and the
    /// stand-ins below, each tuned against the look reference's image
    /// statistics (`docs/renderer/modern-renderer.md`, "The look") and nothing
    /// else, because the evidence does not give them:
    ///
    /// - `sun` 2.8 (the proven look has no multiplier): a gain on the sun that
    ///   stands for the 910 data's darker albedos (its tile colours and
    ///   textures against the modern assets'; the ambient, which the shadows
    ///   show, stays at its proven level). The roof view's luminance
    ///   percentiles land on the reference's: median 0.324 against 0.324,
    ///   darkest 5% 0.115 against 0.109, brightest 5% 0.625 against 0.630,
    ///   mean 0.335 against 0.339; the castle view's shadow contrast (the 5th
    ///   over the 95th percentile) is 0.168 against 0.146.
    /// - `scatter_inscatter` 0.65 (the distance scattering's in-scattering
    ///   scale, 1.5 in the earlier look): the aerial view's mean luminance
    ///   0.474 against 0.433 and the blue over green of its haze 0.78 against
    ///   0.81 (1.5 gives 0.39 and 0.98: at the proven light level it left the
    ///   near ground in a lilac fog, saturation 0.15 against 0.42; the
    ///   darkest 5% of the aerial view stay lifted, 0.28 against 0.15).
    /// - `volumetric_sky_share` 0.1 (the volumetric scattering's share on the
    ///   sky, 0.5 in the earlier look): the sky's colour. The reference sky is
    ///   `(0.83, 0.86, 0.98)` (saturation 0.15, blue over red 1.18); with the
    ///   sun gain above, 0.5 whitens it to `(0.97, 0.97, 0.96)` and 0.1 gives
    ///   `(0.81, 0.87, 0.94)` (saturation 0.14, blue over red 1.16).
    ///
    /// The sky's own level ([`Self::sky_exposure`]) is the earlier look's,
    /// unchanged: the sky then reads against the reference within the cloud
    /// detail the 910 sky has.
    #[must_use]
    pub fn verified() -> Self {
        let mut l = Self::CALIBRATED.proven();
        l.sun = 2.8;
        l.scatter_inscatter = 0.65;
        l.volumetric_sky_share = 0.1;
        l
    }

    /// The look of `mode`: [`Self::verified`], or the earlier
    /// [`Self::CALIBRATED`].
    #[must_use]
    pub fn for_mode(mode: LookMode) -> Self {
        match mode {
            LookMode::Verified => Self::verified(),
            LookMode::ClassicCalibrated => Self::CALIBRATED,
        }
    }

    /// This look with an environment record's tone-map block (the tone-map
    /// values and operator, `crate::lighting::environment::ToneMapBlock`).
    #[must_use]
    pub fn with_tone_map(self, block: &crate::lighting::environment::ToneMapBlock) -> Self {
        let [min_black, max_white, key, min_auto, max_auto] = block.values;
        Self {
            min_black,
            max_white,
            key,
            min_auto,
            max_auto,
            operator: if block.enabled {
                block.operator
            } else {
                crate::post::OPERATOR_NONE
            },
            ..self
        }
    }

    /// The frame's lighting: the sunlight block (module docs) times
    /// the calibration, one ambient colour for both hemispheres (the
    /// fallback before the probes are captured, and the probe capture's
    /// own ambient).
    #[must_use]
    pub fn lighting(&self, env: &rs910_scene::env::EnvFrame, sun_dir: [f32; 3]) -> Lighting {
        let colour = env.sun_rgb.map(|c| display_to_linear(c.clamp(0.0, 1.0)));
        let ambient = colour.map(|c| c * env.sun.ambient * self.ambient);
        Lighting {
            sun_dir,
            sun_colour: colour.map(|c| c * env.sun.diffuse_half * 2.0 * self.sun),
            sky_ambient: ambient,
            ground_ambient: ambient,
        }
    }

    /// This look's values in the post chain's block (`crate::post::PostFrame`):
    /// the tone-map bloom and bright-pass constants and the SSAO's intensity
    /// and radius (`pixels_per_unit`: the projection's pixel scale at unit
    /// depth).
    pub fn post_frame(&self, frame: &mut crate::post::PostFrame, pixels_per_unit: f32) {
        frame.tonemap = [self.min_black, self.max_white, self.key, self.min_auto];
        frame.tonemap2[0] = self.max_auto;
        frame.bloom[0] = self.bloom_threshold;
        frame.bloom[1] = self.bloom_scale;
        frame.ssao[0] = self.ssao_radius * pixels_per_unit;
        frame.ssao[1] = self.ssao_intensity;
        frame.tonemap2[2] = self.scale;
        frame.tonemap2[3] = self.operator as f32;
    }

    /// The operator's largest factor (the advanced RGB Reinhard at the scaled
    /// luminance of the maximum auto exposure): what a sky-bright colour is
    /// multiplied by.
    #[must_use]
    pub fn max_factor(&self) -> f32 {
        let c = self.max_auto;
        c * (1.0 + c / (self.max_white * self.max_white)) / (1.0 + c)
    }

    /// The exposure the display-referred sky inputs (the clear colour, the
    /// classic sky layers and sky models, placed by the inverse of this
    /// backend's old tonemap) are divided by, so that the modern composite
    /// shows them at their display value: a sky is brighter than the
    /// scene's key, so the operator multiplies it by [`Self::max_factor`],
    /// after the light unit [`Self::scale`]; the `sqrt` encode then shows
    /// the 2.2 decode within a gamma of 1.1, times [`Self::sky`]'s square
    /// root. (The atmosphere lane replaces the classic sky with the modern
    /// scattering sky.)
    ///
    /// With a filmic operator no fixed factor exists (the
    /// exposure is the frame's `key / avg`), and the modern client shades its own sky; the
    /// sky inputs keep the level [`Self::CALIBRATED`] gave them (nothing is
    /// fitted to the new operator).
    #[must_use]
    pub fn sky_exposure(&self) -> f32 {
        if self.operator != crate::post::OPERATOR_REINHARD_ADVANCED_RGB {
            let before = Self::CALIBRATED;
            return before.scale * before.max_factor() / before.sky;
        }
        self.scale * self.max_factor() / self.sky
    }

    /// The colour remap weightings for the grading block (`crate::post::grading::
    /// PostUniforms::weights`: base, then the slot weights): the slot
    /// weights times [`Self::grade`], the base the remainder.
    pub fn grade_weights(&self, weights: &mut [f32; 4]) {
        let w = [weights[1], weights[2], weights[3]].map(|w| w * self.grade);
        *weights = [1.0 - (w[0] + w[1] + w[2]), w[0], w[1], w[2]];
    }

    /// The composite without bloom (the CPU mirror of it before the colour
    /// correction): the HDR colour times the light unit through the
    /// operator's tone map (the advanced RGB Reinhard, or the filmic ones,
    /// `crate::post::filmic_tone_map`) at the adapted luminance `adapted`
    /// (of the scaled target), clamped, then the `sqrt` encode.
    #[must_use]
    pub fn tone_map(&self, hdr: [f32; 3], adapted: f32) -> [f32; 3] {
        let v = hdr.map(|c| c * self.scale);
        let filmic = |k| {
            crate::post::filmic_tone_map(
                v,
                adapted,
                &crate::post::ExposureLimits {
                    min_black: self.min_black,
                    max_white: self.max_white,
                    key: self.key,
                    min_auto: self.min_auto,
                    max_auto: self.max_auto,
                },
                k,
            )
        };
        let mapped = match self.operator {
            crate::post::OPERATOR_NONE => v,
            crate::post::OPERATOR_FILMIC_UNCHARTED2 => filmic(crate::post::UNCHARTED2),
            crate::post::OPERATOR_FILMIC_HABLE => filmic(crate::post::HABLE),
            _ => {
                let r = (crate::post::luminance(v) - self.min_black).max(0.0);
                let c = (r * (self.key / adapted)).clamp(self.min_auto, self.max_auto);
                let p = c * (1.0 + c / (self.max_white * self.max_white)) / (1.0 + c);
                v.map(|x| x * p)
            }
        };
        mapped.map(|x| x.clamp(0.0, 1.0).sqrt())
    }

    /// The fog colour of the classic fog colour `display` (0..1).
    #[must_use]
    pub fn fog_colour(&self, display: [f32; 3]) -> [f32; 3] {
        display.map(|c| self.fog * display_to_linear(c.clamp(0.0, 1.0)))
    }
}

/// The frame uniforms' `params.w`: the forward modules' probe ambient takes the
/// SH unnormalised (`lighting/probes.wgsl`, `probe_ambient`).
pub const FRAME_FLAG: f32 = 1.0;
