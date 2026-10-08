//! The modern client's ambient occlusion modes (renderer plan §4(x)), in
//! place of M8's calibrated SSAO. The shader maths is our own translation.
//!
//! - **Modes**: the option `"AmbientOcclusion"` 0..3 (Off, SSAO, HBAO, HBAO
//!   Ultra; 4 "Nvidia HBAO+" draws nothing and is unreachable). 1 draws SSAO
//!   once, with no blur; 2 draws the horizon-based pass with [`HBAO_A`]; 3
//!   draws it with [`HBAO_A`] and then a second time over the same target
//!   with [`HBAO_B`], keeping the minimum of the two ([`PASS_B_BLEND`]). Both
//!   HBAO modes end with the two geometry-aware blur passes (kernel radius 3,
//!   depth sharpness [`BLUR_SHARPNESS`]).
//! - **HBAO** ([`HBAO_WGSL`]): 4 directions x 4 steps, the pixel radius
//!   `min(H, H R / z)` (`H` the viewport height, inferred from the shader's
//!   own use of the lookup scale as the target size), parameters `(R, -1/R^2,
//!   bias, power)`, the output `pow(ao, power)`.
//! - **SSAO** (M8's `fs_ssao`): parameters `(150, 48, 0.0005, |W 150 P00|
//!   0.14)`, 4 minimum samples, 10 base samples, and a far depth equal to the
//!   projection's far minus near depth (read from the blur's own view-depth
//!   unprojection of the same values).
//! - **Noise** ([`noise`]): two 4x4 textures from the LCG `x <- (31415821 x + 1)
//!   mod 2^30` seeded 123456789: HBAO texel `(cos(r1 pi/4), sin(r1 pi/4), r2,
//!   0)`, SSAO the next 64 values; here the `PostFrame.directions`
//!   block (16 texels), read per pixel `(x mod 4, y mod 4)` (HBAO; the row
//!   order and GL's bottom-up rows are inferred) or per sample `i` (SSAO:
//!   texel `i` at nearest filtering, inferred).
//! - **Presets** ([`PRESET_MODES`]): Minimum, Low, Medium, High Off; Ultra
//!   HBAO; Ultra+ HBAO Ultra. The classic option (0..2, default 0, with no
//!   consumer in the classic client) has no proven link to the modern
//!   option, and this client has no modern preset, so the mode is not taken
//!   from the options: AO stays on as before ([`DEFAULT_MODE`], HBAO: the
//!   Ultra preset; a labelled stand-in).
//!
//! The mode is `ModernSettings::ao` (default [`DEFAULT_MODE`]).

pub use crate::settings::AoMode as Mode;

/// The option value of `mode`.
#[must_use]
pub fn value(mode: Mode) -> u32 {
    match mode {
        Mode::Off => 0,
        Mode::Ssao => 1,
        Mode::Hbao => 2,
        Mode::HbaoUltra => 3,
    }
}

/// Whether `mode` ends with the geometry-aware blur (both HBAO modes; SSAO
/// has none).
#[must_use]
pub fn blurs(mode: Mode) -> bool {
    matches!(mode, Mode::Hbao | Mode::HbaoUltra)
}

/// The graphics presets' AO (Minimum, Low, Medium, High, Ultra, Ultra+).
pub const PRESET_MODES: [Mode; 6] = [
    Mode::Off,
    Mode::Off,
    Mode::Off,
    Mode::Off,
    Mode::Hbao,
    Mode::HbaoUltra,
];

/// The default mode (module docs: a labelled stand-in; AO on, as the Ultra
/// preset has it; `ModernSettings::ao`).
pub const DEFAULT_MODE: Mode = PRESET_MODES[4];

/// One horizon-based occlusion parameter set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HbaoSet {
    /// The radius (world units: 512 a tile, as the classic fine units).
    pub radius: f32,
    /// The angle bias (the third parameter).
    pub bias: f32,
    /// The output power (the fourth parameter).
    pub power: f32,
}

impl HbaoSet {
    /// The parameters: radius, falloff `-1 / R^2`, bias, power.
    #[must_use]
    pub fn params(self) -> [f32; 4] {
        [
            self.radius,
            -1.0 / (self.radius * self.radius),
            self.bias,
            self.power,
        ]
    }
}

/// Set A: HBAO and the first pass of HBAO Ultra.
pub const HBAO_A: HbaoSet = HbaoSet {
    radius: 250.0,
    bias: 0.1,
    power: 2.75,
};
/// Set B: HBAO Ultra's second pass.
pub const HBAO_B: HbaoSet = HbaoSet {
    radius: 750.0,
    bias: 0.1,
    power: 2.0,
};

/// SSAO mode's first parameter: the radius the projected fourth is built from.
pub const SSAO_RADIUS: f32 = 150.0;
/// SSAO's second parameter: the intensity.
pub const SSAO_INTENSITY: f32 = 48.0;
/// SSAO's third parameter: the depth-proportional bias.
pub const SSAO_BIAS: f32 = 0.0005;
/// The fourth parameter is `|W R P00| * this`, with `R` the radius above.
pub const SSAO_RADIUS_SCALE: f32 = 0.14;
/// SSAO's minimum and base sample counts.
pub const SSAO_MIN_SAMPLES: f32 = 4.0;
pub const SSAO_BASE_SAMPLES: f32 = 10.0;

/// The geometry-aware blur's depth sharpness: per world unit of view
/// depth.
pub const BLUR_SHARPNESS: f32 = 0.1;

/// How HBAO Ultra's second pass meets the first in the target: blend on,
/// RGB `ONE, ONE, MIN`, alpha the defaults (`ONE, ZERO, ADD`), all channels
/// written. So the target keeps the smaller (darker) of the two passes'
/// occlusions.
pub const PASS_B_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Min,
    },
    alpha: wgpu::BlendComponent::REPLACE,
};

/// The noise: the HBAO texels `(cos a, sin a, r2, 0)`, `a = r1 pi/4`, then
/// the SSAO texels (four draws each), from the LCG (value `x / 2^30`).
#[must_use]
pub fn noise() -> ([[f32; 4]; 16], [[f32; 4]; 16]) {
    let mut x: u64 = 123_456_789;
    let mut next = || {
        x = (31_415_821 * x + 1) % (1 << 30);
        x as f64 / f64::from(1u32 << 30)
    };
    let mut hbao = [[0.0; 4]; 16];
    for t in &mut hbao {
        let a = next() * std::f64::consts::FRAC_PI_4;
        let r2 = next();
        *t = [a.cos() as f32, a.sin() as f32, r2 as f32, 0.0];
    }
    let mut ssao = [[0.0; 4]; 16];
    for t in &mut ssao {
        *t = [next() as f32, next() as f32, next() as f32, next() as f32];
    }
    (hbao, ssao)
}

/// SSAO's sample directions: texel `i`'s `normalize(xy * 2 - 1)`.
#[must_use]
pub fn ssao_directions(texels: &[[f32; 4]; 16]) -> [[f32; 4]; 16] {
    texels.map(|t| {
        let (x, y) = (t[0] * 2.0 - 1.0, t[1] * 2.0 - 1.0);
        let l = (x * x + y * y).sqrt().max(1e-12);
        [x / l, y / l, 0.0, 0.0]
    })
}

/// The projection facts the passes need: the viewport `[w, h]` in pixels
/// and the view-to-clip matrix (classic view space, wgpu depth).
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub viewport: [f32; 2],
    pub proj: glam::Mat4,
}

impl View {
    /// `|W R NDC.x(1, 0, -1)| * 0.14`: SSAO's fourth parameter, the sampling
    /// radius in pixels at unit depth.
    #[must_use]
    pub fn ssao_radius_pixels(&self) -> f32 {
        let c = self.proj * glam::Vec4::new(1.0, 0.0, -1.0, 1.0);
        (self.viewport[0] * SSAO_RADIUS * (c.x / c.w)).abs() * SSAO_RADIUS_SCALE
    }

    /// SSAO's far depth: the view depth at the far plane minus the near's;
    /// `None` when the projection has no finite far plane.
    #[must_use]
    pub fn depth_range(&self) -> Option<f32> {
        let inv = self.proj.inverse();
        let depth = |z: f32| {
            let v = inv * glam::Vec4::new(0.0, 0.0, z, 1.0);
            v.z / v.w
        };
        let range = (depth(1.0) - depth(0.0)).abs();
        (range.is_finite() && range > 0.0).then_some(range)
    }
}

/// The `PostFrame` values of `mode` (slots `ssao`, `ssao2`, `directions`;
/// HBAO's sets go in the `Pass` slots, [`HbaoSet::params`]).
pub fn post_frame(frame: &mut crate::post::PostFrame, mode: Mode, view: &View) {
    let (hbao, ssao) = noise();
    match mode {
        Mode::Ssao => {
            frame.ssao = [
                view.ssao_radius_pixels(),
                SSAO_INTENSITY,
                SSAO_BIAS,
                SSAO_BASE_SAMPLES,
            ];
            frame.ssao2 = [
                view.depth_range().unwrap_or(crate::post::SSAO_Z_FAR),
                SSAO_MIN_SAMPLES,
                BLUR_SHARPNESS,
                0.0,
            ];
            frame.directions = ssao_directions(&ssao);
        }
        Mode::Hbao | Mode::HbaoUltra | Mode::Off => {
            frame.ssao2[2] = BLUR_SHARPNESS;
            frame.directions = hbao;
        }
    }
}

/// The geometry and sampling area in occlusion texels. A fractional viewport
/// preserves the projected scene; the scissor includes each touched texel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Area {
    pub rect: [f32; 4],
    pub clip: [u32; 4],
}

impl Area {
    pub(crate) fn new(rect: [i32; 4], clip: [u32; 4], divisor: u32) -> Self {
        let scale = divisor as f32;
        Self {
            rect: rect.map(|v| v as f32 / scale),
            clip: [
                clip[0] / divisor,
                clip[1] / divisor,
                clip[2].div_ceil(divisor),
                clip[3].div_ceil(divisor),
            ],
        }
    }
}
