//! The modern renderer's quality settings ([`ModernSettings`]): what a
//! player chooses in Graphics, as data the subsystems read.
//!
//! Where the client's options have a proven meaning for the renderer they
//! drive it: the shadow options (`sceneryShadows`, `shadowQuality`,
//! `characterShadows`, per frame through
//! [`crate::frame::ModernRenderer::set_shadow_settings`]), the
//! anti-aliasing level (the forward target's sample count and FXAA,
//! [`crate::frame::ModernRenderer::set_samples`]) and the game's
//! bloom setting ([`crate::frame::ModernRenderer::set_scene_bloom`]). The
//! local quality controls use the versioned RendererPreferences record in
//! rs910-config. Defaults preserve the previous frame. Development overrides
//! resolve above saved choices ([`ModernSettings::resolve`]); native option
//! bytes, CS2 vars and packets retain their existing contract.
//!
//! | Setting | Variable | Values (default first) |
//! |---|---|---|
//! | [`ModernSettings::shadows`] | `CLIENT910_MODERN_SHADOWS` | the options; `off`, `low`, `medium`, `high`, `ultra`, `ultraplus` |
//! | [`ModernSettings::ao`] | `CLIENT910_MODERN_AO` | `hbao`, `off`, `ssao`, `hbao-ultra` |
//! | [`ModernSettings::env_reflections`] | `CLIENT910_MODERN_ENV_REFLECTIONS` | `proven`, `all` |
//! | [`ModernSettings::far`] | `CLIENT910_MODERN_FAR` | `2`, `off`, `0`..`4` |
//! | [`ModernSettings::volumetrics`] | `CLIENT910_MODERN_VOLUMETRICS` | `off`, `on` |
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

use rs910_config::renderer_preferences::RendererPreferences;
pub use rs910_config::renderer_preferences::{AoMode, AoResolution, Reflections, RenderScale};
use rs910_far_scene::far_level::FarLevel;

/// The sun shadows' quality source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shadows {
    /// The client's shadow options (the default).
    Options,
    /// A fixed quality whatever the options say (`None`: off).
    Fixed(Option<ShadowQuality>),
}

/// Which RT5 materials take the global environment map
/// ([`crate::lighting::global_env_mapping`]).
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
    /// Bloom (`None`: the client's scene bloom state).
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
        ao_resolution: AoResolution::Half,
        env_reflections: EnvReflections::Proven,
        far: Some(FarLevel::DEFAULT),
        volumetrics: false,
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
        let s = Self::resolve(RendererPreferences::DEFAULT);
        if s != Self::DEFAULT {
            log::info!("[modern] settings {s:?}");
        }
        s
    }

    /// [`Self::from_env`] over any variable source.
    #[must_use]
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        Self::from_preferences_with_vars(RendererPreferences::DEFAULT, var)
    }

    /// Resolve saved choices under development overrides, captured once per
    /// process so renderer recreation and live editing use the same policy.
    pub fn resolve(preferences: RendererPreferences) -> Self {
        static OVERRIDES: std::sync::OnceLock<std::collections::BTreeMap<&'static str, String>> =
            std::sync::OnceLock::new();
        const VARIABLES: &[&str] = &[
            "CLIENT910_MODERN_SHADOWS",
            "CLIENT910_MODERN_AO",
            "CLIENT910_MODERN_AO_RESOLUTION",
            "CLIENT910_MODERN_ENV_REFLECTIONS",
            "CLIENT910_MODERN_FAR",
            "CLIENT910_MODERN_VOLUMETRICS",
            "CLIENT910_MODERN_BLOOM",
            "CLIENT910_MODERN_FXAA",
            "CLIENT910_MODERN_DOF",
            "CLIENT910_MODERN_REFLECTIONS",
            "CLIENT910_MODERN_RENDER_SCALE",
            "CLIENT910_MODERN_LOOK",
        ];
        let overrides = OVERRIDES.get_or_init(|| {
            VARIABLES
                .iter()
                .filter_map(|&name| std::env::var(name).ok().map(|value| (name, value)))
                .collect()
        });
        Self::from_preferences_with_vars(preferences, |name| overrides.get(name).cloned())
    }

    pub fn preferences(self) -> RendererPreferences {
        RendererPreferences {
            render_scale: self.render_scale,
            ao: self.ao,
            ao_resolution: self.ao_resolution,
            draw_distance: rs910_config::renderer_preferences::DrawDistance::from_level(
                self.far.map(FarLevel::index),
            )
            .expect("validated renderer distance"),
            volumetrics: self.volumetrics,
            reflections: self.reflections,
            dof: self.dof,
        }
    }

    /// Pure resolver for fixtures and callers that supply their own overrides.
    pub fn from_preferences_with_vars(
        preferences: RendererPreferences,
        var: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let mut s = Self {
            render_scale: preferences.render_scale,
            ao: preferences.ao,
            ao_resolution: preferences.ao_resolution,
            far: preferences.draw_distance.level().and_then(FarLevel::new),
            volumetrics: preferences.volumetrics,
            reflections: preferences.reflections,
            dof: preferences.dof,
            ..Self::DEFAULT
        };
        let read = |name: &str, apply: &mut dyn FnMut(&str) -> bool| {
            if let Some(v) = var(name) {
                let v = v.trim().to_ascii_lowercase();
                if !apply(&v) {
                    log::warn!("[modern] {name}={v:?} not understood; the saved choice stays");
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
