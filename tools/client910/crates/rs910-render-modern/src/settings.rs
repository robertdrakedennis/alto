//! The modern renderer's quality settings ([`ModernSettings`]): what a
//! player (or a later options screen) chooses, as data the subsystems read.
//!
//! Where the client's options have a proven meaning for the renderer they
//! drive it: the shadow options (`sceneryShadows`, `shadowQuality`,
//! `characterShadows`, per frame through
//! [`crate::frame::ModernRenderer::set_shadow_settings`]), the
//! anti-aliasing level (the forward target's sample count and FXAA,
//! [`crate::frame::ModernRenderer::set_samples`]) and the faithful toolkit's
//! bloom state ([`crate::frame::ModernRenderer::set_faithful_bloom`]). The
//! rest has no proven option: its default is the renderer's chosen value,
//! and `CLIENT910_MODERN_*` variables override it for development
//! ([`ModernSettings::from_env`], read once by the shell; never written to
//! `ClientOptions` or the preferences, so nothing CS2 or the server sees
//! changes).
//!
//! | Setting | Variable | Values (default first) |
//! |---|---|---|
//! | [`ModernSettings::shadows`] | `CLIENT910_MODERN_SHADOWS` | the options; `off`, `low`, `medium`, `high`, `ultra`, `ultraplus` |
//! | [`ModernSettings::ao`] | `CLIENT910_MODERN_AO` | `hbao`, `off`, `ssao`, `hbao-ultra` |
//! | [`ModernSettings::env_reflections`] | `CLIENT910_MODERN_ENV_REFLECTIONS` | `proven`, `all` |
//! | [`ModernSettings::far`] | `CLIENT910_MODERN_FAR` | `2`, `off`, `0`..`4` |
//! | [`ModernSettings::volumetrics`] | `CLIENT910_MODERN_VOLUMETRICS` | `on`, `off` |
//! | [`ModernSettings::bloom`] | `CLIENT910_MODERN_BLOOM` | the options; `on`, `off` |
//! | [`ModernSettings::fxaa`] | `CLIENT910_MODERN_FXAA` | with one sample; `on`, `off` |
//! | [`ModernSettings::dof`] | `CLIENT910_MODERN_DOF` | `off`, `on` |
//! | [`ModernSettings::look`] | `CLIENT910_MODERN_LOOK` | `verified`, `classic-calibrated` |
//! | [`ModernSettings::reflections`] | `CLIENT910_MODERN_REFLECTIONS` | `full`, `half`, `off` |
//! | [`ModernSettings::render_scale`] | `CLIENT910_MODERN_RENDER_SCALE` | `auto` (100% on a low-DPI display, [`RenderScale::auto`] on a high-DPI one); `0.5`..`2`, or a percentage `50`..`200` |
//!
//! The point-light shadows have no setting of their own: they are on exactly
//! when the sun shadows are, at the sun shadows' quality
//! ([`crate::shadows::presets::point_preset`]).
//!
//! An unrecognised value logs a warning and keeps the default.

use rs910_far_scene::far_level::FarLevel;

/// The sun shadows' quality source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shadows {
    /// The client's shadow options (the default).
    Options,
    /// A fixed quality whatever the options say (`None`: off).
    Fixed(Option<ShadowQuality>),
}

/// The ambient occlusion mode: the four values of the modern client's option
/// `"AmbientOcclusion"` (see [`crate::post::ao`]; the default
/// [`AoMode::Hbao`] is the Ultra preset's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AoMode {
    Off,
    Ssao,
    Hbao,
    HbaoUltra,
}

/// The ambient occlusion target's size relative to the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AoResolution {
    Full,
    Half,
}

impl AoResolution {
    /// Scene pixels per occlusion texel.
    #[must_use]
    pub const fn divisor(self) -> u32 {
        const FULL_RESOLUTION_DIVISOR: u32 = 1;
        const HALF_RESOLUTION_DIVISOR: u32 = 2;
        match self {
            Self::Full => FULL_RESOLUTION_DIVISOR,
            Self::Half => HALF_RESOLUTION_DIVISOR,
        }
    }

    /// The occlusion target's extent, including an odd final scene pixel.
    #[must_use]
    pub fn size(self, scene: [u32; 2]) -> [u32; 2] {
        const MIN_TARGET_EXTENT: u32 = 1;
        scene.map(|v| v.div_ceil(self.divisor()).max(MIN_TARGET_EXTENT))
    }
}

/// Which RT5 materials take the global environment map
/// ([`crate::lighting::env_reflections`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvReflections {
    /// The materials the modern client's vertex gate proves (the default).
    Proven,
    /// Also the flagged opaque high-detail materials (the gate's unproven
    /// define set).
    All,
}

/// Which values the look uses where the modern client sets them in code
/// ([`crate::lighting::look`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LookMode {
    /// The values the modern client proves (the environment record's tone
    /// mapping and filmic operator, the record's colour remap slots at the
    /// data weights, the lights without a multiplier) and the ambient
    /// captured per map square ([`LookMode::captured_ambient`]). What the
    /// evidence leaves open is a labelled stand-in tuned against the look
    /// reference ([`crate::lighting::look::Look::verified`]). The default.
    Verified,
    /// The earlier look: calibrated by region statistics against the look
    /// reference with its own light multipliers, the earlier tone map and the
    /// zone probes' ambient. Kept as a choice (`classic-calibrated`) for
    /// comparison; nothing selects it by default.
    ClassicCalibrated,
}

impl LookMode {
    /// Whether the ambient is the per-square irradiance captured from the
    /// world ([`crate::lighting::ambient`]) rather than the zone probes'.
    #[must_use]
    pub fn captured_ambient(self) -> bool {
        self == Self::Verified
    }
}

/// See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModernSettings {
    pub shadows: Shadows,
    pub ao: AoMode,
    pub ao_resolution: AoResolution,
    pub env_reflections: EnvReflections,
    /// The far scene's draw-distance level (`None`: the classic window only).
    pub far: Option<FarLevel>,
    /// The volumetric scattering pass.
    pub volumetrics: bool,
    /// Bloom (`None`: the faithful toolkit's bloom state).
    pub bloom: Option<bool>,
    /// FXAA (`None`: with a single-sample forward target).
    pub fxaa: Option<bool>,
    /// Depth of field (off in NXT gameplay).
    pub dof: bool,
    pub look: LookMode,
    /// The water's planar reflection.
    pub reflections: Reflections,
    /// The scene's resolution against the scene viewport's pixels
    /// ([`crate::frame::scale`]); `None`: automatic, by the display's scale
    /// factor ([`RenderScale::auto`]). The shell's saved choice and this
    /// setting's variable override it
    /// ([`crate::frame::ModernRenderer::set_display`]).
    pub render_scale: Option<RenderScale>,
}

/// The water's planar reflection: the modern client's option `"Reflections"`
/// (values 0-4, presets Low 0, Medium 1, High 2, Ultra 3, Ultra+ 4; applying
/// a preset sets the reflection effect's quality -1, 1, 2, 3, 3, and the
/// reflection map is sized for quality 1 at half the swapchain's width and
/// height, for 2 and 3 at its full size, or at fixed 1024x512 and 2048x1024
/// under a device flag not traced). The default
/// [`Reflections::Full`] is the High to Ultra+ presets' (a stand-in like
/// [`AoMode`]'s: no 910 option maps to it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reflections {
    /// No planar reflection (quality -1): the water reflects its
    /// environment only.
    Off,
    /// The reflection at half the scene viewport's width and height
    /// (quality 1).
    Half,
    /// At the scene viewport's size (qualities 2 and 3).
    Full,
}

impl Reflections {
    /// The reflection target's size divisor (`None`: no reflection).
    #[must_use]
    pub fn divisor(self) -> Option<i32> {
        match self {
            Self::Off => None,
            Self::Half => Some(2),
            Self::Full => Some(1),
        }
    }
}

/// The render scale: the scene's pixels per scene-viewport pixel along each
/// axis, in percent (the modern client's option `"GameRenderScale"`, 50..200
/// percent, applied as `value * 0.01` to the viewport's size). Away from
/// 100 the scene renders at that scale and its frame is resampled into the
/// viewport under the faithful UI ([`crate::frame::scale`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderScale(u16);

impl RenderScale {
    /// The viewport's own resolution (the modern client's default is 100,
    /// or `round10(9600 / dpi)` above 96 dpi, and its window object's dpi is
    /// a constant 96 that is never read from the display).
    pub const FULL: Self = Self(100);
    /// The scale factor from which a display counts as high-DPI.
    pub const HIGH_DPI_FROM: f64 = 1.25;
    /// The pixels of scene a high-DPI window renders at most (1600 x 1000):
    /// a full-size Retina window is GPU-bound at about 27 ms a frame at its
    /// own 3200 x 2000, and about 7 ms at this budget.
    pub const HIGH_DPI_BUDGET: f64 = 1_600_000.0;
    /// The option's range.
    pub const MIN_PERCENT: u16 = 50;
    pub const MAX_PERCENT: u16 = 200;

    /// `percent` of the viewport's pixels per axis (`None`: outside
    /// [`Self::MIN_PERCENT`]..=[`Self::MAX_PERCENT`]).
    #[must_use]
    pub fn percent(percent: u16) -> Option<Self> {
        (Self::MIN_PERCENT..=Self::MAX_PERCENT)
            .contains(&percent)
            .then_some(Self(percent))
    }

    /// The scale in percent.
    #[must_use]
    pub fn as_percent(self) -> u16 {
        self.0
    }

    /// The automatic scale (a stand-in for the modern client's, which never
    /// reads the display): 100 on a low-DPI display, and on a high-DPI one
    /// (`scale_factor` from [`Self::HIGH_DPI_FROM`]) the share of a
    /// `viewport_pixels` scene viewport that renders at most
    /// [`Self::HIGH_DPI_BUDGET`] pixels, per axis, in steps of 5 and within
    /// the option's range: 50 for a full-size window at scale factor 2, 100
    /// for a window of up to 1600 x 1000 physical pixels.
    #[must_use]
    pub fn auto(scale_factor: f64, viewport_pixels: u64) -> Self {
        if scale_factor < Self::HIGH_DPI_FROM || viewport_pixels == 0 {
            return Self::FULL;
        }
        let share = (Self::HIGH_DPI_BUDGET / viewport_pixels as f64)
            .sqrt()
            .min(1.0);
        let percent = ((share * 20.0).round() as u16 * 5)
            .clamp(Self::MIN_PERCENT, Self::MAX_PERCENT)
            .min(100);
        Self(percent)
    }

    /// `0.5`..`2` (a factor) or `50`..`200` (a percentage, `%` optional).
    #[must_use]
    pub fn parse(v: &str) -> Option<Self> {
        let v = v.trim().trim_end_matches('%');
        let x: f64 = v.parse().ok()?;
        let percent = if x <= 2.0 { x * 100.0 } else { x };
        if !percent.is_finite() {
            return None;
        }
        Self::percent(percent.round() as u16)
    }
}

impl Default for ModernSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl ModernSettings {
    /// The defaults (see the module docs).
    pub const DEFAULT: Self = Self {
        shadows: Shadows::Options,
        ao: AoMode::Hbao,
        ao_resolution: AoResolution::Full,
        env_reflections: EnvReflections::Proven,
        far: Some(FarLevel::DEFAULT),
        volumetrics: true,
        bloom: None,
        fxaa: None,
        dof: false,
        look: LookMode::Verified,
        reflections: Reflections::Full,
        render_scale: None,
    };

    /// The defaults with the `CLIENT910_MODERN_*` overrides of the module
    /// docs (the shell reads them once).
    #[must_use]
    pub fn from_env() -> Self {
        let s = Self::from_vars(|name| std::env::var(name).ok());
        if s != Self::DEFAULT {
            log::info!("[modern] settings {s:?}");
        }
        s
    }

    /// [`Self::from_env`] over any variable source.
    #[must_use]
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let mut s = Self::DEFAULT;
        let read = |name: &str, apply: &mut dyn FnMut(&str) -> bool| {
            if let Some(v) = var(name) {
                let v = v.trim().to_ascii_lowercase();
                if !apply(&v) {
                    log::warn!("[modern] {name}={v:?} not understood; the default stays");
                }
            }
        };
        read("CLIENT910_MODERN_SHADOWS", &mut |v| {
            match ShadowQuality::parse(v) {
                Some(q) => s.shadows = Shadows::Fixed(q),
                None => return false,
            }
            true
        });
        read("CLIENT910_MODERN_AO", &mut |v| {
            s.ao = match v {
                "off" | "none" => AoMode::Off,
                "ssao" => AoMode::Ssao,
                "hbao" => AoMode::Hbao,
                "hbao-ultra" | "ultra" => AoMode::HbaoUltra,
                _ => return false,
            };
            true
        });
        read("CLIENT910_MODERN_AO_RESOLUTION", &mut |v| {
            s.ao_resolution = match v {
                "full" => AoResolution::Full,
                "half" => AoResolution::Half,
                _ => return false,
            };
            true
        });
        read("CLIENT910_MODERN_ENV_REFLECTIONS", &mut |v| {
            s.env_reflections = match v {
                "proven" => EnvReflections::Proven,
                "all" => EnvReflections::All,
                _ => return false,
            };
            true
        });
        read("CLIENT910_MODERN_FAR", &mut |v| {
            if v == "off" {
                s.far = None;
                return true;
            }
            match v.parse::<u8>().ok().and_then(FarLevel::new) {
                Some(level) => s.far = Some(level),
                None => return false,
            }
            true
        });
        read("CLIENT910_MODERN_VOLUMETRICS", &mut |v| {
            parse_bool(v).map(|b| s.volumetrics = b).is_some()
        });
        read("CLIENT910_MODERN_BLOOM", &mut |v| {
            parse_bool(v).map(|b| s.bloom = Some(b)).is_some()
        });
        read("CLIENT910_MODERN_FXAA", &mut |v| {
            parse_bool(v).map(|b| s.fxaa = Some(b)).is_some()
        });
        read("CLIENT910_MODERN_DOF", &mut |v| {
            parse_bool(v).map(|b| s.dof = b).is_some()
        });
        read("CLIENT910_MODERN_REFLECTIONS", &mut |v| {
            s.reflections = match v {
                "off" => Reflections::Off,
                "half" => Reflections::Half,
                "full" => Reflections::Full,
                _ => return false,
            };
            true
        });
        read("CLIENT910_MODERN_RENDER_SCALE", &mut |v| {
            if v == "auto" {
                s.render_scale = None;
                return true;
            }
            RenderScale::parse(v)
                .map(|r| s.render_scale = Some(r))
                .is_some()
        });
        read("CLIENT910_MODERN_LOOK", &mut |v| {
            s.look = match v {
                "verified" => LookMode::Verified,
                "classic-calibrated" | "calibrated" => LookMode::ClassicCalibrated,
                _ => return false,
            };
            true
        });
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ao_resolution_defaults_to_full_and_rounds_odd_extents_up() {
        const ODD_SCENE: [u32; 2] = [481, 321];
        const HALF_EXTENT: [u32; 2] = [241, 161];
        assert_eq!(ModernSettings::DEFAULT.ao_resolution, AoResolution::Full);
        assert_eq!(AoResolution::Full.size(ODD_SCENE), ODD_SCENE);
        assert_eq!(AoResolution::Half.size(ODD_SCENE), HALF_EXTENT);
        let settings = ModernSettings::from_vars(|name| {
            (name == "CLIENT910_MODERN_AO_RESOLUTION").then(|| "half".into())
        });
        assert_eq!(settings.ao_resolution, AoResolution::Half);
        assert_eq!(settings.ao, ModernSettings::DEFAULT.ao);
    }

    #[test]
    fn the_automatic_render_scale_follows_the_display_and_the_pixel_budget() {
        let auto = |factor, pixels| RenderScale::auto(factor, pixels).as_percent();
        // A low-DPI display never scales, whatever the window.
        assert_eq!(auto(1.0, 6_400_000), 100);
        // A high-DPI window of up to 1600 x 1000 pixels renders in full.
        assert_eq!(auto(2.0, 1_500_000), 100);
        // A full-size one renders about 1600 x 1000 pixels (50% of 3200 x 2000).
        assert_eq!(auto(2.0, 6_400_000), 50);
        assert_eq!(auto(2.0, 2_560_000), 80);
        // Never below the option's range.
        assert_eq!(auto(3.0, 40_000_000), 50);
        // The settings: no value is automatic, a value (or the variable) fixes it.
        assert_eq!(ModernSettings::DEFAULT.render_scale, None);
        let var = |v: &'static str| {
            ModernSettings::from_vars(|n| (n == "CLIENT910_MODERN_RENDER_SCALE").then(|| v.into()))
                .render_scale
        };
        assert_eq!(var("auto"), None);
        assert_eq!(var("75"), RenderScale::percent(75));
        assert_eq!(var("0.5"), RenderScale::percent(50));
        assert_eq!(var("7"), None);
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "on" | "1" | "true" => Some(true),
        "off" | "0" | "false" => Some(false),
        _ => None,
    }
}

/// The graphics quality of the sun shadows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShadowQuality {
    Low = 0,
    Medium = 1,
    High = 2,
    Ultra = 3,
    /// The modern client's fifth level (`SHADOWS_QUALITY_ULTRA_PLUS`,
    /// [`crate::shadows::presets`]); the older table has four, so the
    /// pre-lane table treats it as [`ShadowQuality::Ultra`].
    UltraPlus = 4,
}

impl ShadowQuality {
    /// Every level, lowest first.
    pub const ALL: [Self; 5] = [
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Ultra,
        Self::UltraPlus,
    ];

    /// `off|low|medium|high|ultra|ultraplus` (`CLIENT910_MODERN_SHADOWS`):
    /// `Some(None)` for off, `None` for anything else.
    #[must_use]
    pub fn parse(name: &str) -> Option<Option<Self>> {
        match name {
            "off" => Some(None),
            "low" => Some(Some(Self::Low)),
            "medium" => Some(Some(Self::Medium)),
            "high" => Some(Some(Self::High)),
            "ultra" => Some(Some(Self::Ultra)),
            "ultraplus" => Some(Some(Self::UltraPlus)),
            _ => None,
        }
    }
}
