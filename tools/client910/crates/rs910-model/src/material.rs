//! Material dispatch and the GL program state of a batch, shared by the
//! oracles and the renderer.

use crate::texture::{AlphaMode, Material};

/// Which floor pass a batch belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloorWater {
    /// Ordinary floor (no water detail, not the underwater pass).
    #[default]
    Normal,
    /// Water detail floors (`waterDetail == 2`) route
    /// effects 2/4/8/9 to `EnvMappedWater`/`EnvMappedSea`.
    WaterDetail,
    /// Drawn inside the scene draw's underwater pass: the `UnderwaterGround*`
    /// programs.
    Underwater,
}

/// Program ids beyond the `Model` shader modes 0-6 used by the floor shader.
/// `EnvMappedWater` (effect 2, `waves false`).
pub const PROGRAM_ENV_WATER: i32 = 7;
/// `EnvMappedSea` (effects 4/8/9, `waves true`).
pub const PROGRAM_ENV_SEA: i32 = 8;
/// `Model` program `UnderwaterGround` (ShaderMode 4).
pub const PROGRAM_UNDERWATER_GROUND: i32 = 9;
/// `Model` program `UnderwaterGroundSpecular` (ShaderMode 5).
pub const PROGRAM_UNDERWATER_SPECULAR: i32 = 10;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialSpec {
    pub effect: u8,
    pub argument: u8,
    pub alpha_ref: u8,
    pub scroll: [f32; 2],
    pub lit: bool,
    pub model: bool,
    pub textured: bool,
    pub floor_water: FloorWater,
}
impl MaterialSpec {
    pub fn new(material: Option<&Material>, lit: bool, model: bool) -> Self {
        Self {
            effect: material.map_or(11, |m| m.effect),
            argument: material.map_or(0, |m| m.effect_param),
            alpha_ref: if model {
                material
                    .filter(|m| m.alpha == AlphaMode::AlphaTested)
                    .map_or(0, |m| m.alpha_threshold)
            } else {
                0
            },
            scroll: if model {
                material.map_or([0.; 2], |m| [m.speed_u, m.speed_v])
            } else {
                [0.; 2]
            },
            lit,
            model,
            textured: material.is_some(),
            floor_water: FloorWater::Normal,
        }
    }
    /// Tag a floor batch with its water pass (see [`FloorWater`]).
    #[must_use]
    pub fn with_floor_water(mut self, water: FloorWater) -> Self {
        self.floor_water = water;
        self
    }
    pub fn program(self) -> i32 {
        if !self.lit {
            return if matches!(self.effect, 1 | 7) && self.textured {
                6
            } else {
                3
            };
        }
        if !self.model {
            // Lit floors. Lighting is off on the programmable GL path and the
            // sea water shader is available once `EnvMappedWater` compiled.
            match (self.floor_water, self.effect) {
                (FloorWater::Underwater, 1) => return PROGRAM_UNDERWATER_SPECULAR,
                (FloorWater::Underwater, 2 | 4 | 6 | 7 | 8 | 9) => {}
                (FloorWater::Underwater, _) => return PROGRAM_UNDERWATER_GROUND,
                (FloorWater::WaterDetail, 2) => return PROGRAM_ENV_WATER,
                (FloorWater::WaterDetail, 4 | 8 | 9) => return PROGRAM_ENV_SEA,
                _ => {}
            }
        }
        match self.effect {
            1 => 1,
            7 => 2,
            6 => 3,
            5 if self.model => 5,
            _ => 0,
        }
    }
    pub fn uv_offset(self, millis: i32) -> [f32; 2] {
        if !self.textured {
            return [0.; 2];
        }
        self.scroll.map(|speed| {
            let value = (millis % 128000) as f32 / 1000. * speed;
            (if self.lit { value } else { value / 64. }) % 1.
        })
    }
}

/// A persistent toolkit state. Blend-disabled draws replace alpha too;
/// separate blend factors have no effect while GL_BLEND is disabled.
#[derive(Clone, Debug)]
pub struct MaterialState {
    pub blend_mode: i32,
    pub alpha_mode: i32,
    pub alpha_ref: u8,
    pub alpha_test: bool,
    pub blend: bool,
    pub depth_primary: bool,
    pub depth_secondary: bool,
    pub depth_test: bool,
    pub fog: bool,
    pub cull: i32,
    pub exponent: f32,
    cache: i32,
}
impl Default for MaterialState {
    fn default() -> Self {
        Self {
            blend_mode: 1,
            alpha_mode: -1,
            alpha_ref: 0,
            alpha_test: true,
            blend: true,
            depth_primary: true,
            depth_secondary: false,
            depth_test: false,
            fog: false,
            cull: 2,
            exponent: 0.,
            cache: 0,
        }
    }
}
impl MaterialState {
    pub fn begin_3d(&mut self) {
        if self.cache == 8 {
            return;
        }
        self.fog = true;
        self.depth_test = true;
        self.depth_secondary = true;
        self.blend_mode(1);
        self.alpha_ref(0);
        self.cache = 8;
    }
    pub fn blend_mode(&mut self, mode: i32) {
        if self.blend_mode == mode {
            return;
        }
        self.blend_mode = mode;
        self.alpha_test = matches!(mode, 1 | 3 | 128);
        self.blend = matches!(mode, 1 | 2 | 128);
        self.cache &= !28;
    }
    pub fn alpha_ref(&mut self, value: u8) {
        if self.alpha_ref == value {
            return;
        }
        self.alpha_ref = value;
        self.alpha_mode = if value == 0 { 0 } else { 3 };
        self.blend_mode(if value == 0 { 1 } else { 3 });
    }
    pub fn depth_secondary(&mut self, value: bool) {
        if self.depth_secondary != value {
            self.depth_secondary = value;
            self.cache &= !31;
        }
    }
    pub fn material(&mut self, spec: MaterialSpec) -> i32 {
        if spec.lit && spec.textured {
            match spec.argument {
                1 => self.exponent = 32.,
                2 => self.exponent = 4.,
                3 => self.exponent = 1.,
                _ => {}
            }
        }
        if spec.model {
            self.alpha_ref(spec.alpha_ref);
        }
        spec.program()
    }
    pub fn words(&self) -> [i32; 10] {
        [
            self.cull,
            self.alpha_test as i32,
            self.alpha_ref as i32,
            self.blend as i32,
            self.blend_mode,
            self.alpha_mode,
            self.depth_primary as i32,
            self.depth_secondary as i32,
            self.depth_test as i32,
            self.fog as i32,
        ]
    }
}

/// The waterfall shader's parameters; the time modulo is kept in double
/// precision.
pub fn waterfall_parameters(argument: u8, millis: i32) -> [f32; 4] {
    let downward = ((argument & 3) + 1) as f32 * -0.0005;
    let speed = (((argument >> 3) & 3) + 1) as f32 * 0.0005;
    let scale = if argument & 0x40 == 0 {
        0.00048828125
    } else {
        0.0009765625
    };
    [
        if argument & 0x80 != 0 { scale } else { -scale },
        scale,
        (millis as f32 * downward) % 1.,
        (speed as f64 * millis as f64 % 1.) as f32,
    ]
}
