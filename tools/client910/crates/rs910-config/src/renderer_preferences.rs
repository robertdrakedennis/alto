//! Versioned local graphics preferences, independent of the wire-visible
//! ClientOptions codec. The renderer and retained settings UI share this data.

const FULL_RESOLUTION_DIVISOR: i32 = 1;
const HALF_RESOLUTION_DIVISOR: i32 = 2;
const FULL_PERCENT: u16 = 100;
const EMPTY_VIEWPORT: u64 = 0;
const FULL_SHARE: f64 = 1.0;
const SCALE_STEPS: f64 = 20.0;
const SCALE_STEP_PERCENT: u16 = 5;
const MAX_FACTOR: f64 = 2.0;
const PERCENT_PER_FACTOR: f64 = 100.0;
const FORMAT_VERSION: &str = "1";
const NEAR_LEVEL: u8 = 0;
const MEDIUM_LEVEL: u8 = 1;
const FAR_LEVEL: u8 = 2;
const VERY_FAR_LEVEL: u8 = 3;
const MAXIMUM_LEVEL: u8 = 4;
const FIRST_CHOICE: usize = 0;
const SCENE_AXES: usize = 2;
const CHOICE_STEP: usize = 1;

/// A draw-distance choice with no invalid level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawDistance {
    Window,
    Near,
    Medium,
    Far,
    VeryFar,
    Maximum,
}

impl DrawDistance {
    pub const fn level(self) -> Option<u8> {
        match self {
            Self::Window => None,
            Self::Near => Some(NEAR_LEVEL),
            Self::Medium => Some(MEDIUM_LEVEL),
            Self::Far => Some(FAR_LEVEL),
            Self::VeryFar => Some(VERY_FAR_LEVEL),
            Self::Maximum => Some(MAXIMUM_LEVEL),
        }
    }
    pub fn from_level(level: Option<u8>) -> Option<Self> {
        Some(match level {
            None => Self::Window,
            Some(NEAR_LEVEL) => Self::Near,
            Some(MEDIUM_LEVEL) => Self::Medium,
            Some(FAR_LEVEL) => Self::Far,
            Some(VERY_FAR_LEVEL) => Self::VeryFar,
            Some(MAXIMUM_LEVEL) => Self::Maximum,
            _ => return None,
        })
    }
}

/// Local quality choices. Native shadow, AA and bloom controls retain their
/// existing ClientOptions owners and are deliberately absent here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RendererPreferences {
    pub render_scale: Option<RenderScale>,
    pub ao: AoMode,
    pub ao_resolution: AoResolution,
    pub draw_distance: DrawDistance,
    pub volumetrics: bool,
    pub reflections: Reflections,
    pub dof: bool,
}

impl Default for RendererPreferences {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualityPreset {
    Performance,
    Balanced,
    High,
    Ultra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererControl {
    Preset,
    RenderScale,
    DrawDistance,
    AmbientOcclusion,
    OcclusionResolution,
    Reflections,
    Volumetrics,
    DepthOfField,
}

impl RendererControl {
    pub const ALL: &[Self] = &[
        Self::Preset,
        Self::RenderScale,
        Self::DrawDistance,
        Self::AmbientOcclusion,
        Self::OcclusionResolution,
        Self::Reflections,
        Self::Volumetrics,
        Self::DepthOfField,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Preset => "preset",
            Self::RenderScale => "render_scale",
            Self::DrawDistance => "draw_distance",
            Self::AmbientOcclusion => "ao",
            Self::OcclusionResolution => "ao_resolution",
            Self::Reflections => "reflections",
            Self::Volumetrics => "volumetrics",
            Self::DepthOfField => "dof",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Preset => "Quality preset",
            Self::RenderScale => "Scene resolution",
            Self::DrawDistance => "Draw distance",
            Self::AmbientOcclusion => "Ambient occlusion",
            Self::OcclusionResolution => "Occlusion resolution",
            Self::Reflections => "Water reflections",
            Self::Volumetrics => "Volumetric lighting",
            Self::DepthOfField => "Depth of field",
        }
    }
    pub fn choices(self) -> &'static [&'static str] {
        match self {
            Self::Preset => &["Performance", "Balanced", "High", "Ultra"],
            Self::RenderScale => &["Auto", "50", "75", "100", "125", "150", "200"],
            Self::DrawDistance => &["Window", "Near", "Medium", "Far", "Very far", "Maximum"],
            Self::AmbientOcclusion => &["Off", "SSAO", "HBAO", "HBAO ultra"],
            Self::OcclusionResolution | Self::Reflections => {
                if self == Self::Reflections {
                    &["Off", "Half", "Full"]
                } else {
                    &["Half", "Full"]
                }
            }
            Self::Volumetrics | Self::DepthOfField => &["Off", "On"],
        }
    }
}

impl QualityPreset {
    pub const ALL: &[Self] = &[Self::Performance, Self::Balanced, Self::High, Self::Ultra];
    pub fn label(self) -> &'static str {
        match self {
            Self::Performance => "Performance",
            Self::Balanced => "Balanced",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }
    pub fn preferences(self) -> RendererPreferences {
        let mut p = RendererPreferences::DEFAULT;
        match self {
            Self::Performance => {
                p.ao = AoMode::Off;
                p.ao_resolution = AoResolution::Half;
                p.draw_distance = DrawDistance::Near;
                p.volumetrics = false;
                p.reflections = Reflections::Off;
            }
            Self::Balanced => {
                p.ao = AoMode::Ssao;
                p.ao_resolution = AoResolution::Half;
                p.draw_distance = DrawDistance::Medium;
                p.reflections = Reflections::Half;
            }
            Self::High => {}
            Self::Ultra => {
                p.ao = AoMode::HbaoUltra;
                p.draw_distance = DrawDistance::Maximum;
            }
        }
        p
    }
}

impl RendererPreferences {
    /// Matches the modern renderer's original defaults.
    pub const DEFAULT: Self = Self {
        render_scale: None,
        ao: AoMode::Hbao,
        ao_resolution: AoResolution::Full,
        draw_distance: DrawDistance::Far,
        volumetrics: true,
        reflections: Reflections::Full,
        dof: false,
    };
    pub fn preset(self) -> Option<QualityPreset> {
        QualityPreset::ALL
            .iter()
            .copied()
            .find(|p| p.preferences() == self)
    }
    pub fn value(self, control: RendererControl) -> String {
        match control {
            RendererControl::Preset => self.preset().map_or("Custom", QualityPreset::label).into(),
            RendererControl::RenderScale => self
                .render_scale
                .map_or("Auto".into(), |s| s.as_percent().to_string()),
            RendererControl::DrawDistance => match self.draw_distance {
                DrawDistance::Window => "Window",
                DrawDistance::Near => "Near",
                DrawDistance::Medium => "Medium",
                DrawDistance::Far => "Far",
                DrawDistance::VeryFar => "Very far",
                DrawDistance::Maximum => "Maximum",
            }
            .into(),
            RendererControl::AmbientOcclusion => match self.ao {
                AoMode::Off => "Off",
                AoMode::Ssao => "SSAO",
                AoMode::Hbao => "HBAO",
                AoMode::HbaoUltra => "HBAO ultra",
            }
            .into(),
            RendererControl::OcclusionResolution => match self.ao_resolution {
                AoResolution::Half => "Half",
                AoResolution::Full => "Full",
            }
            .into(),
            RendererControl::Reflections => match self.reflections {
                Reflections::Off => "Off",
                Reflections::Half => "Half",
                Reflections::Full => "Full",
            }
            .into(),
            RendererControl::Volumetrics => if self.volumetrics { "On" } else { "Off" }.into(),
            RendererControl::DepthOfField => if self.dof { "On" } else { "Off" }.into(),
        }
    }
    /// Apply a validated choice. Invalid input never partially changes state.
    pub fn set(&mut self, control: RendererControl, value: &str) -> bool {
        let v = value.trim().to_ascii_lowercase();
        match control {
            RendererControl::Preset => {
                let Some(preset) = QualityPreset::ALL
                    .iter()
                    .find(|p| p.label().eq_ignore_ascii_case(&v))
                else {
                    return false;
                };
                *self = preset.preferences();
            }
            RendererControl::RenderScale => {
                self.render_scale = if v == "auto" {
                    None
                } else {
                    let Some(scale) = RenderScale::parse(&v) else {
                        return false;
                    };
                    Some(scale)
                }
            }
            RendererControl::DrawDistance => {
                self.draw_distance = match v.as_str() {
                    "window" => DrawDistance::Window,
                    "near" => DrawDistance::Near,
                    "medium" => DrawDistance::Medium,
                    "far" => DrawDistance::Far,
                    "very far" => DrawDistance::VeryFar,
                    "maximum" => DrawDistance::Maximum,
                    _ => return false,
                }
            }
            RendererControl::AmbientOcclusion => {
                self.ao = match v.as_str() {
                    "off" => AoMode::Off,
                    "ssao" => AoMode::Ssao,
                    "hbao" => AoMode::Hbao,
                    "hbao ultra" => AoMode::HbaoUltra,
                    _ => return false,
                }
            }
            RendererControl::OcclusionResolution => {
                self.ao_resolution = match v.as_str() {
                    "half" => AoResolution::Half,
                    "full" => AoResolution::Full,
                    _ => return false,
                }
            }
            RendererControl::Reflections => {
                self.reflections = match v.as_str() {
                    "off" => Reflections::Off,
                    "half" => Reflections::Half,
                    "full" => Reflections::Full,
                    _ => return false,
                }
            }
            RendererControl::Volumetrics | RendererControl::DepthOfField => {
                let b = match v.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return false,
                };
                if control == RendererControl::Volumetrics {
                    self.volumetrics = b;
                } else {
                    self.dof = b;
                }
            }
        }
        true
    }
    pub fn step(&mut self, control: RendererControl, forward: bool) {
        let choices = control.choices();
        let current = choices.iter().position(|v| *v == self.value(control));
        let next = match current {
            None => FIRST_CHOICE,
            Some(i) if forward => (i + CHOICE_STEP) % choices.len(),
            Some(i) => (i + choices.len() - CHOICE_STEP) % choices.len(),
        };
        self.set(control, choices[next]);
    }
    pub fn encode(self) -> String {
        let mut text = format!("version={FORMAT_VERSION}\n");
        for &control in RendererControl::ALL {
            if control != RendererControl::Preset {
                text.push_str(&format!("{}={}\n", control.key(), self.value(control)));
            }
        }
        text
    }
    /// Accepts the legacy scale-only file; rejects future schemas, duplicate
    /// keys and invalid values instead of accepting a partially corrupt file.
    pub fn decode(text: &str) -> Result<Self, &'static str> {
        let mut p = Self::DEFAULT;
        let mut seen = std::collections::BTreeSet::new();
        let mut versioned = false;
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (key, value) = line
                .split_once('=')
                .ok_or("Invalid graphics preference line")?;
            let key = key.trim();
            if !seen.insert(key) {
                return Err("Duplicate graphics preference");
            }
            if key == "version" {
                if value.trim() != FORMAT_VERSION {
                    return Err("Unsupported graphics preference version");
                }
                versioned = true;
            } else {
                let control = RendererControl::ALL
                    .iter()
                    .copied()
                    .find(|c| *c != RendererControl::Preset && c.key() == key)
                    .ok_or("Unknown graphics preference")?;
                if !p.set(control, value) {
                    return Err("Invalid graphics preference value");
                }
            }
        }
        if !versioned && (seen.len() != CHOICE_STEP || !seen.contains("render_scale")) {
            return Err("Missing graphics preference version");
        }
        Ok(p)
    }
}

#[cfg(test)]
mod preference_tests {
    use super::*;
    #[test]
    fn versioned_preferences_migrate_and_validate_every_control() {
        for &preset in QualityPreset::ALL {
            let mut p = preset.preferences();
            assert_eq!(RendererPreferences::decode(&p.encode()), Ok(p));
            for &control in RendererControl::ALL {
                for &choice in control.choices() {
                    assert!(p.set(control, choice));
                    assert_eq!(RendererPreferences::decode(&p.encode()), Ok(p));
                }
            }
        }
        let migrated = RendererPreferences::decode("render_scale=75\n").unwrap();
        assert_eq!(migrated.render_scale, RenderScale::parse("75"));
        assert_eq!(migrated.ao, RendererPreferences::DEFAULT.ao);
        for invalid in [
            "version=99",
            "garbage",
            "render_scale=10",
            "version=1\nao=broken",
            "version=1\nao=Off\nao=HBAO",
            "version=1\nextra=On",
        ] {
            assert!(RendererPreferences::decode(invalid).is_err(), "{invalid}");
        }
    }
}
/// The ambient occlusion mode: the four values of the modern client's option
/// `"AmbientOcclusion"` (see the occlusion pass; the default
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
    pub fn size(self, scene: [u32; SCENE_AXES]) -> [u32; SCENE_AXES] {
        const MIN_TARGET_EXTENT: u32 = 1;
        scene.map(|v| v.div_ceil(self.divisor()).max(MIN_TARGET_EXTENT))
    }
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
            Self::Half => Some(HALF_RESOLUTION_DIVISOR),
            Self::Full => Some(FULL_RESOLUTION_DIVISOR),
        }
    }
}

/// The render scale: the scene's pixels per scene-viewport pixel along each
/// axis, in percent (the modern client's option `"GameRenderScale"`, 50..200
/// percent, applied as `value * 0.01` to the viewport's size). Away from
/// 100 the scene renders at that scale and its frame is resampled into the
/// viewport under the faithful UI (the scene upscale).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderScale(u16);

impl RenderScale {
    /// The viewport's own resolution (the modern client's default is 100,
    /// or `round10(9600 / dpi)` above 96 dpi, and its window object's dpi is
    /// a constant 96 that is never read from the display).
    pub const FULL: Self = Self(FULL_PERCENT);
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
        if scale_factor < Self::HIGH_DPI_FROM || viewport_pixels == EMPTY_VIEWPORT {
            return Self::FULL;
        }
        let share = (Self::HIGH_DPI_BUDGET / viewport_pixels as f64)
            .sqrt()
            .min(FULL_SHARE);
        let percent = ((share * SCALE_STEPS).round() as u16 * SCALE_STEP_PERCENT)
            .clamp(Self::MIN_PERCENT, Self::MAX_PERCENT)
            .min(FULL_PERCENT);
        Self(percent)
    }

    /// `0.5`..`2` (a factor) or `50`..`200` (a percentage, `%` optional).
    #[must_use]
    pub fn parse(v: &str) -> Option<Self> {
        let v = v.trim().trim_end_matches('%');
        let x: f64 = v.parse().ok()?;
        let percent = if x <= MAX_FACTOR {
            x * PERCENT_PER_FACTOR
        } else {
            x
        };
        if !percent.is_finite() {
            return None;
        }
        Self::percent(percent.round() as u16)
    }
}
