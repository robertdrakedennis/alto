//! The post-process chain: three effects (bloom, levels, colour remapping),
//! their parameters, the pass schedule, the colour-remap look-up volumes
//! (16^3) and the environment's per-frame feed, plus the GPU half that runs
//! the chain.
//!
//! The capture is opened by the full-screen environment layer (see
//! `ui_backend::PostRegion`): the scene is drawn into a canvas-sized
//! framebuffer cleared black, then every live effect runs in chain order,
//! ping-ponging between two spare textures, the last pass of the last effect
//! writing the layer bounds of the screen. The fragment programs are the
//! bloom (bright pass, blur, composite), levels and colour remapping
//! techniques.
use std::{collections::HashMap, rc::Rc};
use wgpu::util::DeviceExt;

/// Bloom passes 0..2 draw a 256x256 region.
pub const BLOOM_SIZE: u32 = 256;
/// Colour-remap volume edge.
pub const LUT_SIZE: usize = 16;

/// The effects the toolkit installs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    Bloom,
    Levels,
    ColourRemapping,
}

impl Effect {
    /// The chain position key: bloom first, then levels, then colour
    /// remapping.
    pub fn order(self) -> i32 {
        match self {
            Self::Bloom => 0,
            Self::Levels => 1,
            Self::ColourRemapping => 2,
        }
    }
    /// Pass count (bloom 4, the filters 1).
    pub fn passes(self) -> usize {
        match self {
            Self::Bloom => 4,
            Self::Levels | Self::ColourRemapping => 1,
        }
    }
    /// Framebuffer data type (`0` 8-bit unsigned, `1` 16-bit float, `2`
    /// 32-bit float).
    pub fn data_type(self) -> i32 {
        match self {
            Self::Bloom => 1,
            Self::Levels | Self::ColourRemapping => 0,
        }
    }
}

/// The bloom effect's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BloomParams {
    pub threshold: f32,
    pub white_point_sq: f32,
    pub intensity: f32,
    pub sample_scale: f32,
}

impl Default for BloomParams {
    /// The neutral bloom parameters.
    fn default() -> Self {
        Self {
            threshold: 1.0,
            white_point_sq: 1.0,
            intensity: 0.25,
            sample_scale: 1.0,
        }
    }
}

/// The levels effect's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelsParams {
    pub gamma: f32,
    pub input_min: f32,
    pub input_max: f32,
    pub output_min: f32,
    pub output_max: f32,
}

impl Default for LevelsParams {
    /// The identity levels.
    fn default() -> Self {
        Self::from_array([1.0, 0.0, 1.0, 0.0, 1.0])
    }
}

impl LevelsParams {
    /// `[gamma, inputMin, inputMax, outputMin, outputMax]`, the
    /// environment's levels order.
    pub fn from_array(v: [f32; 5]) -> Self {
        Self {
            gamma: v[0],
            input_min: v[1],
            input_max: v[2],
            output_min: v[3],
            output_max: v[4],
        }
    }
    /// Whether the levels leave every colour unchanged.
    pub fn is_noop(&self) -> bool {
        self.gamma == 1.0
            && self.input_min == 0.0
            && self.input_max == 1.0
            && self.output_min == 0.0
            && self.output_max == 1.0
    }
}

/// A colour remapper: the `256x16` remapping sprite behind a colour
/// remapping map id.
#[derive(Clone, Debug)]
pub struct Remapper {
    pub id: i32,
    /// The sprite's ARGB pixels, row-major `256x16`.
    pub argb: Rc<[i32]>,
}

impl PartialEq for Remapper {
    /// The original client compares the cached objects by identity; the
    /// cache keys them by sprite id.
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

/// The colour remapping effect's parameters: up to three remappers with
/// their weights, the number of samples, and the source colour's weight.
#[derive(Clone, Debug, PartialEq)]
pub struct ColourRemapParams {
    pub remappers: [Option<Remapper>; 3],
    pub weights: [f32; 3],
    pub count: i32,
    pub base: f32,
}

impl Default for ColourRemapParams {
    /// No remappers, source colour only.
    fn default() -> Self {
        Self {
            remappers: [None, None, None],
            weights: [0.0; 3],
            count: 1,
            base: 1.0,
        }
    }
}

impl ColourRemapParams {
    /// Drop weighted empty slots towards the front, count the positive
    /// weights, and give the source colour the remainder.
    pub fn set(
        mut a: Option<Remapper>,
        mut aw: f32,
        mut b: Option<Remapper>,
        mut bw: f32,
        mut c: Option<Remapper>,
        mut cw: f32,
    ) -> Self {
        if c.is_none() && cw > 0.0 {
            cw = 0.0;
        }
        if b.is_none() && bw > 0.0 {
            b = c.take();
            bw = cw;
            cw = 0.0;
        }
        if a.is_none() && aw > 0.0 {
            a = b.take();
            b = c.take();
            aw = bw;
            bw = cw;
            cw = 0.0;
        }
        let count = [aw, bw, cw].iter().filter(|w| **w > 0.0).count() as i32;
        Self {
            remappers: [a, b, c],
            weights: [aw, bw, cw],
            count,
            base: 1.0 - (aw + bw + cw),
        }
    }
    /// Whether the remapping leaves every colour unchanged.
    pub fn is_noop(&self) -> bool {
        self.count == 0
            || self.base == 1.0
            || self.weights[0] + self.weights[1] + self.weights[2] == 0.0
            || self.remappers.iter().all(Option::is_none)
    }
}

/// The effect statics one frame reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Params {
    pub bloom: BloomParams,
    pub levels: LevelsParams,
    pub remap: ColourRemapParams,
}

impl Params {
    /// Whether an effect leaves the frame unchanged; bloom is never a no-op.
    pub fn is_noop(&self, effect: Effect) -> bool {
        match effect {
            Effect::Bloom => false,
            Effect::Levels => self.levels.is_noop(),
            Effect::ColourRemapping => self.remap.is_noop(),
        }
    }
}

/// The ordered effect vector and the widest framebuffer data type any added
/// effect asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chain {
    effects: Vec<Effect>,
    pub data_type: i32,
}

impl Chain {
    /// Levels and colour remapping are always installed; bloom is added by
    /// [`Chain::add`] once the bloom preference is reconciled.
    pub fn with_filters() -> Self {
        let mut chain = Self::default();
        chain.add(Effect::Levels);
        chain.add(Effect::ColourRemapping);
        chain
    }
    /// Insert before the first effect with a larger position key, raising
    /// the chain's data type, unless the effect is already in the chain
    /// (returns whether it was added). Every effect's program compiles on
    /// this device.
    pub fn add(&mut self, effect: Effect) -> bool {
        if self.contains(effect) {
            return false;
        }
        let at = self
            .effects
            .iter()
            .position(|e| effect.order() < e.order())
            .unwrap_or(self.effects.len());
        self.effects.insert(at, effect);
        self.data_type = self.data_type.max(effect.data_type());
        true
    }
    /// Remove an effect; the chain's data type is not lowered.
    pub fn remove(&mut self, effect: Effect) {
        self.effects.retain(|e| *e != effect);
    }
    /// Whether the effect is in the chain.
    pub fn contains(&self, effect: Effect) -> bool {
        self.effects.contains(&effect)
    }
    #[cfg(test)]
    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }
    /// Open a capture when some enabled effect is live; the capture's own
    /// empty-chain early-out then cannot trigger.
    pub fn capture(&self, params: &Params) -> bool {
        self.effects.iter().any(|e| !params.is_noop(*e))
    }
    /// When any effect is a no-op, only the live ones run (in chain order).
    pub fn live(&self, params: &Params) -> Vec<Effect> {
        self.effects
            .iter()
            .copied()
            .filter(|e| !params.is_noop(*e))
            .collect()
    }
}

/// Where one pass writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassOutput {
    /// One of the three canvas-sized textures.
    Texture(usize),
    /// The screen, bounded by the layer.
    Screen,
}

/// One pass of the schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub effect: Effect,
    pub pass: usize,
    /// The capture for pass 0, the previous pass's output after.
    pub input: usize,
    /// The scene copy (read by the bloom composite).
    pub scene: usize,
    pub output: PassOutput,
    /// The final pass of the final effect.
    pub last: bool,
}

/// The pass schedule over physical textures `0` (the capture, holding the
/// scene), `1` and `2` (the ping-pong pair): intermediate passes write the
/// current spare, the last pass of a non-final effect writes the scene
/// back, and the pair swaps after every pass. The write-back goes to the
/// spare texture that is then renamed the scene, because a wgpu pass cannot
/// sample its own attachment (the original client samples and renders the
/// capture together; every read is at its own pixel).
pub fn schedule(live: &[Effect]) -> Vec<Step> {
    let (mut scene, mut a, mut b) = (0usize, 1usize, 2usize);
    let mut steps = Vec::new();
    for (i, &effect) in live.iter().enumerate() {
        let passes = effect.passes();
        let final_effect = i + 1 == live.len();
        for pass in 0..passes {
            let last_pass = pass + 1 == passes;
            let input = if pass == 0 { scene } else { a };
            let output = if !last_pass {
                PassOutput::Texture(b)
            } else if final_effect {
                PassOutput::Screen
            } else {
                PassOutput::Texture(b)
            };
            steps.push(Step {
                effect,
                pass,
                input,
                scene,
                output,
                last: final_effect && last_pass,
            });
            if last_pass && !final_effect {
                // Write-back: the written texture becomes the scene; the
                // old scene texture is free.
                std::mem::swap(&mut scene, &mut b);
            }
            std::mem::swap(&mut a, &mut b);
        }
    }
    steps
}

/// The position and texture-coordinate extents a pass uploads, as `[region
/// x, region y, texcoord x, texcoord y]` fractions of a `fbo`-sized
/// framebuffer: the drawn region over the framebuffer, and the texture
/// extent (the `surface`-sized screen on the last pass).
pub fn pass_geometry(
    effect: Effect,
    pass: usize,
    fbo: [u32; 2],
    surface: [u32; 2],
    last: bool,
) -> [f32; 4] {
    let (w, h) = (fbo[0] as f32, fbo[1] as f32);
    let (mut rw, mut rh) = (fbo[0] as i32, fbo[1] as i32);
    let (mut tw, mut th) = if last {
        (surface[0] as i32, surface[1] as i32)
    } else {
        (rw, rh)
    };
    if effect == Effect::Bloom {
        match pass {
            0 => {
                rw = BLOOM_SIZE as i32;
                rh = BLOOM_SIZE as i32;
            }
            1 | 2 => {
                rw = BLOOM_SIZE as i32;
                rh = BLOOM_SIZE as i32;
                tw = rw;
                th = rh;
            }
            _ => {}
        }
    }
    [rw as f32 / w, rh as f32 / h, tw as f32 / w, th as f32 / h]
}

/// The 16^3 look-up volume of a remapping sprite: the `256x16` sprite is 16
/// slices of `16x16`, slice `z` in columns `16z..16z+16`; texel `(x, y, z)`
/// takes sprite pixel `(16z + x, y)` as RGBA.
pub fn lut_texels(argb: &[i32]) -> Vec<u8> {
    let mut out = vec![0u8; LUT_SIZE * LUT_SIZE * LUT_SIZE * 4];
    for x in 0..LUT_SIZE {
        for y in 0..LUT_SIZE {
            for z in 0..LUT_SIZE {
                let pixel = argb[y * 256 + z * 16 + x];
                let at = (y * 16 + z * 256 + x) * 4;
                out[at] = (pixel >> 16) as u8;
                out[at + 1] = (pixel >> 8) as u8;
                out[at + 2] = pixel as u8;
                out[at + 3] = 0xff;
            }
        }
    }
    out
}

/// The colour remapping shader on the CPU, for tests: the clamped source
/// colour weighted by `base` plus each counted LUT's trilinear sample at
/// `0.03125 + c * 0.9375`.
#[cfg(test)]
pub fn remap_reference(params: &ColourRemapParams, luts: &[Vec<u8>], rgb: [f32; 3]) -> [f32; 3] {
    let c = rgb.map(|v| v.clamp(0.0, 1.0));
    let mut out = c.map(|v| v * params.base);
    for slot in 0..params.count.clamp(0, 3) as usize {
        let Some(lut) = luts.get(slot) else { continue };
        let coord = c.map(|v| 0.03125 + v * 0.9375);
        let s = sample_lut(lut, coord);
        for i in 0..3 {
            out[i] += s[i] * params.weights[slot];
        }
    }
    out
}

#[cfg(test)]
fn sample_lut(lut: &[u8], coord: [f32; 3]) -> [f32; 3] {
    // GL_LINEAR on a 16^3 volume: texel centres at (i + 0.5) / 16.
    let axis = |c: f32| {
        let t = c * LUT_SIZE as f32 - 0.5;
        let i0 = t.floor();
        let f = t - i0;
        let wrap = |i: f32| (i as i32).rem_euclid(LUT_SIZE as i32) as usize;
        (wrap(i0), wrap(i0 + 1.0), f)
    };
    let (x0, x1, fx) = axis(coord[0]);
    let (y0, y1, fy) = axis(coord[1]);
    let (z0, z1, fz) = axis(coord[2]);
    let texel = |x: usize, y: usize, z: usize| {
        let at = (y * 16 + z * 256 + x) * 4;
        [0, 1, 2].map(|i| lut[at + i] as f32 / 255.0)
    };
    let mut out = [0.0f32; 3];
    for (z, wz) in [(z0, 1.0 - fz), (z1, fz)] {
        for (y, wy) in [(y0, 1.0 - fy), (y1, fy)] {
            for (x, wx) in [(x0, 1.0 - fx), (x1, fx)] {
                let t = texel(x, y, z);
                for i in 0..3 {
                    out[i] += t[i] * wx * wy * wz;
                }
            }
        }
    }
    out
}

/// The levels shader on the CPU, for tests.
#[cfg(test)]
pub fn levels_reference(p: &LevelsParams, rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(|v| {
        let c = v.clamp(0.0, 1.0);
        let x = ((c - p.input_min) / (p.input_max - p.input_min)).clamp(0.0, 1.0);
        let g = x.powf(p.gamma).clamp(0.0, 1.0);
        p.output_min + g * (p.output_max - p.output_min)
    })
}

/// A small LRU cache (8 entries) of remappers by sprite id: only a `256x16`
/// sprite yields a remapper, and only hits are cached.
pub struct RemapperCache {
    pack: Option<crate::cache::Pack>,
    cache: Vec<Remapper>,
}

impl RemapperCache {
    const CAPACITY: usize = 8;
    pub fn new(pack: Option<crate::cache::Pack>) -> Self {
        Self {
            pack,
            cache: Vec::new(),
        }
    }
    pub fn get(&mut self, id: i32) -> Option<Remapper> {
        if let Some(at) = self.cache.iter().position(|r| r.id == id) {
            let hit = self.cache.remove(at);
            self.cache.push(hit.clone());
            return Some(hit);
        }
        let pack = self.pack.as_ref()?;
        let bytes = crate::js5_fetch::fetch_file(pack, "sprites", id as u32).ok()??;
        let frames = crate::sprite_data::Data::decode(&bytes).ok()?;
        let data = frames.first()?;
        if data.width != 256 || data.height != 16 {
            return None;
        }
        let remapper = Remapper {
            id,
            argb: data.argb(false).ok()?.into(),
        };
        if self.cache.len() == Self::CAPACITY {
            self.cache.remove(0);
        }
        self.cache.push(remapper.clone());
        Some(remapper)
    }
}

/// Feed the chain's parameters from the current environment. The override
/// levels and remapping are only ever reset, never set, so the current
/// environment always wins.
pub fn update_full(
    chain: &Chain,
    params: &mut Params,
    env: &crate::env::EnvFrame,
    remappers: &mut RemapperCache,
) {
    // The sample scale keeps its default of 1.
    params.bloom = BloomParams {
        threshold: env.bloom[2],
        white_point_sq: env.bloom[0],
        intensity: env.bloom[1],
        sample_scale: 1.0,
    };
    if chain.contains(Effect::Levels) {
        params.levels = LevelsParams::from_array(env.levels);
    }
    if chain.contains(Effect::ColourRemapping) {
        let mut get = |slot: usize| {
            let (map, _) = env.colour_remap[slot];
            (map > -1).then(|| remappers.get(map)).flatten()
        };
        let (a, b, c) = (get(0), get(1), get(2));
        params.remap = ColourRemapParams::set(
            a,
            env.colour_remap[0].1,
            b,
            env.colour_remap[1].1,
            c,
            env.colour_remap[2].1,
        );
    }
}

/// Moved to `rs910_scene::env` (lane E-A1): the client core's environment
/// fade interpolates without the GPU crate.
pub use crate::env::interpolate;
#[cfg(test)]
use crate::env::sort_descending;

const SHADER: &str = r#"
struct Uniforms { geom: vec4<f32>, params: vec4<f32>, extra: vec4<f32>, ranges: vec4<f32>, misc: vec4<f32> }
@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var bloom_tex: texture_2d<f32>;
@group(0) @binding(2) var filter_sampler: sampler;
@group(0) @binding(3) var<uniform> u: Uniforms;
@group(0) @binding(4) var remap_1: texture_3d<f32>;
@group(0) @binding(5) var remap_2: texture_3d<f32>;
@group(0) @binding(6) var remap_3: texture_3d<f32>;
struct Varying { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
// Full-screen triangle from the pass geometry. GL samples the negated v
// through GL_REPEAT from the bottom-left origin; wgpu's top-left origin
// gives the same texel with v = +w.
@vertex fn vertex(@builtin(vertex_index) id: u32) -> Varying {
    let x = f32((id << 1u) & 2u);
    let y = f32(id & 2u);
    var v: Varying;
    v.position = vec4<f32>(x * 2.0 * u.geom.x - 1.0, 1.0 - y * 2.0 * u.geom.y, 0.0, 1.0);
    v.uv = vec2<f32>(x * u.geom.z, y * u.geom.w);
    return v;
}
const LUM: vec3<f32> = vec3<f32>(0.212599993, 0.715200007, 0.0722000003);
// Bloom bright pass.
@fragment fn brightpass(v: Varying) -> @location(0) vec4<f32> {
    let c = textureSample(scene_tex, filter_sampler, v.uv - u.extra.xy);
    return c * select(0.0, 1.0, dot(LUM, c.xyz) >= u.params.x);
}
// Bloom blur (one axis per pass).
@fragment fn blur(v: Varying) -> @location(0) vec4<f32> {
    let s = u.misc.zw;
    let t = v.uv - u.extra.xy;
    var c = textureSample(scene_tex, filter_sampler, t) * 0.0913962647;
    c += textureSample(scene_tex, filter_sampler, t - s) * 0.0885843039;
    c += textureSample(scene_tex, filter_sampler, t + s) * 0.0885843039;
    c += textureSample(scene_tex, filter_sampler, t - 2.0 * s) * 0.0806569234;
    c += textureSample(scene_tex, filter_sampler, t + 2.0 * s) * 0.0806569234;
    c += textureSample(scene_tex, filter_sampler, t - 3.0 * s) * 0.0689895153;
    c += textureSample(scene_tex, filter_sampler, t + 3.0 * s) * 0.0689895153;
    c += textureSample(scene_tex, filter_sampler, t - 4.0 * s) * 0.0554346368;
    c += textureSample(scene_tex, filter_sampler, t + 4.0 * s) * 0.0554346368;
    c += textureSample(scene_tex, filter_sampler, t - 5.0 * s) * 0.0418442599;
    c += textureSample(scene_tex, filter_sampler, t + 5.0 * s) * 0.0418442599;
    c += textureSample(scene_tex, filter_sampler, t - 6.0 * s) * 0.0296720229;
    c += textureSample(scene_tex, filter_sampler, t + 6.0 * s) * 0.0296720229;
    c += textureSample(scene_tex, filter_sampler, t - 7.0 * s) * 0.0197658278;
    c += textureSample(scene_tex, filter_sampler, t + 7.0 * s) * 0.0197658278;
    c += textureSample(scene_tex, filter_sampler, t - 8.0 * s) * 0.0123691391;
    c += textureSample(scene_tex, filter_sampler, t + 8.0 * s) * 0.0123691391;
    return vec4<f32>(c.xyz, 1.0);
}
// Bloom composite over the scene copy.
@fragment fn composite(v: Varying) -> @location(0) vec4<f32> {
    let bloom = textureSample(bloom_tex, filter_sampler, v.uv * u.extra.zw - u.extra.xy);
    let s = textureSample(scene_tex, filter_sampler, v.uv - u.extra.xy);
    let scene = vec4<f32>(s.xyz, 1.0);
    let pre = 0.99000001 * dot(LUM, scene.xyz) + 0.00999999978;
    let post = (pre * (1.0 + pre / u.params.z)) / (pre + 1.0);
    return clamp(scene * (post / pre) + bloom * u.params.y, vec4<f32>(0.0), vec4<f32>(1.0));
}
// Levels.
@fragment fn levels(v: Varying) -> @location(0) vec4<f32> {
    let r = u.ranges;
    let c = clamp(textureSample(scene_tex, filter_sampler, v.uv - u.extra.xy).xyz, vec3<f32>(0.0), vec3<f32>(1.0));
    let x = clamp((c - r.x) / (r.y - r.x), vec3<f32>(0.0), vec3<f32>(1.0));
    let g = clamp(pow(x, vec3<f32>(u.misc.x)), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(vec3<f32>(r.z) + g * (vec3<f32>(r.w) - vec3<f32>(r.z)), 1.0);
}
// Colour remapping; `count` is the number of remapper samples.
@fragment fn remap(v: Varying) -> @location(0) vec4<f32> {
    let c = clamp(textureSample(scene_tex, filter_sampler, v.uv - u.extra.xy).xyz, vec3<f32>(0.0), vec3<f32>(1.0));
    var out = c * u.params.x;
    let coord = vec3<f32>(0.03125) + c * vec3<f32>(0.9375);
    let s1 = textureSample(remap_1, filter_sampler, coord).xyz;
    let s2 = textureSample(remap_2, filter_sampler, coord).xyz;
    let s3 = textureSample(remap_3, filter_sampler, coord).xyz;
    let count = u.misc.y;
    if (count > 0.5) { out += s1 * u.params.y; }
    if (count > 1.5) { out += s2 * u.params.z; }
    if (count > 2.5) { out += s3 * u.params.w; }
    return vec4<f32>(out, 1.0);
}
// One mip-chain step for the capture.
@fragment fn downsample(v: Varying) -> @location(0) vec4<f32> {
    return textureSampleLevel(scene_tex, filter_sampler, v.uv, 0.0);
}
// Scene viewport copy into the black capture.
@fragment fn copy(v: Varying) -> @location(0) vec4<f32> {
    return textureLoad(scene_tex, vec2<i32>(v.position.xy), 0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    /// Region/texcoord extents ([`pass_geometry`]).
    pub geom: [f32; 4],
    /// Bloom parameters; colour remapping weightings.
    pub params: [f32; 4],
    /// Pixel offset and bloom scale.
    pub extra: [f32; 4],
    /// Levels ranges.
    pub ranges: [f32; 4],
    /// Levels gamma, remap sample count, bloom sample size.
    pub misc: [f32; 4],
}

/// The uniforms for `step`.
pub fn step_uniforms(step: &Step, params: &Params, fbo: [u32; 2], surface: [u32; 2]) -> Uniforms {
    let (w, h) = (fbo[0] as f32, fbo[1] as f32);
    let mut u = Uniforms {
        geom: pass_geometry(step.effect, step.pass, fbo, surface, step.last),
        ..Default::default()
    };
    match step.effect {
        Effect::Bloom => {
            let b = &params.bloom;
            u.params = [b.threshold, b.intensity, b.white_point_sq, 0.0];
            u.extra = [0.0, 0.0, BLOOM_SIZE as f32 / w, BLOOM_SIZE as f32 / h];
            u.misc = match step.pass {
                1 => [0.0, 0.0, b.sample_scale / w, 0.0],
                2 => [0.0, 0.0, 0.0, b.sample_scale / h],
                _ => [0.0; 4],
            };
        }
        Effect::Levels => {
            let l = &params.levels;
            u.ranges = [l.input_min, l.input_max, l.output_min, l.output_max];
            u.misc = [l.gamma, 0.0, 0.0, 0.0];
        }
        Effect::ColourRemapping => {
            let r = &params.remap;
            u.params = [r.base, r.weights[0], r.weights[1], r.weights[2]];
            u.misc = [0.0, r.count as f32, 0.0, 0.0];
        }
    }
    u
}

struct Targets {
    size: [u32; 2],
    format: wgpu::TextureFormat,
    /// The capture (mipmapped), then the ping-pong pair.
    #[allow(dead_code)]
    textures: [wgpu::Texture; 3],
    /// Full views (all mips) for sampling.
    views: [wgpu::TextureView; 3],
    /// Level-0 attachments.
    attachments: [wgpu::TextureView; 3],
    /// Single-level views of the capture's mip chain.
    levels: Vec<wgpu::TextureView>,
}

/// One draw position's uniform buffer and bind group (programme Phase 6:
/// kept across frames instead of created per draw). The group is rebuilt
/// when the views it binds change; the buffer holds the last uniforms
/// written to it.
struct DrawSlot {
    uniforms: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// Scene, bloom and the three LUT views the group binds.
    views: [wgpu::TextureView; 5],
    written: Option<Uniforms>,
}

/// The scene copy a capture starts from: `scene` is a `size` texture of
/// `format` (the resolved scene target), copied inside `scene_clip`.
#[derive(Clone, Copy)]
pub struct Capture<'a> {
    pub scene: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: [u32; 2],
    pub scene_clip: [u32; 4],
}

/// Where the chain's last pass writes: the screen view and its format, and
/// the physical `[x, y, w, h]` bounds inside it.
#[derive(Clone, Copy)]
pub struct Screen<'a> {
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub bounds: [u32; 4],
}

/// One full-screen draw: the pipeline entry, its target and the views it
/// samples. `present` selects the slot family (see `PostProcessor::slots`).
#[derive(Clone, Copy)]
struct PassDraw<'a> {
    present: bool,
    entry: &'static str,
    format: wgpu::TextureFormat,
    target: &'a wgpu::TextureView,
    clear: bool,
    viewport: Option<[u32; 4]>,
    scissor: Option<[u32; 4]>,
    uniforms: Uniforms,
    scene: &'a wgpu::TextureView,
    bloom: &'a wgpu::TextureView,
    luts: [&'a wgpu::TextureView; 3],
}

/// The GPU half of the post-process chain, owned by the renderer.
pub struct PostProcessor {
    pub chain: Chain,
    pub params: Params,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    pipelines: HashMap<(&'static str, wgpu::TextureFormat), wgpu::RenderPipeline>,
    sampler: wgpu::Sampler,
    dummy_lut: wgpu::TextureView,
    /// Placeholder for an unused bloom texture binding.
    dummy_2d: wgpu::TextureView,
    luts: HashMap<i32, wgpu::TextureView>,
    targets: Option<Targets>,
    /// Draw slots by `(present, draw index)`: [`PostProcessor::encode`]'s
    /// draws and [`PostProcessor::present`]'s, each numbered from 0 per
    /// call (the renderer calls one of them once per frame).
    slots: std::cell::RefCell<HashMap<(bool, usize), DrawSlot>>,
    next_slot: std::cell::Cell<usize>,
}

impl PostProcessor {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post-process"),
            entries: &[
                texture(0, wgpu::TextureViewDimension::D2),
                texture(1, wgpu::TextureViewDimension::D2),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(4, wgpu::TextureViewDimension::D3),
                texture(5, wgpu::TextureViewDimension::D3),
                texture(6, wgpu::TextureViewDimension::D3),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post-process"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bloom, levels and colour remapping"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        // GL_LINEAR, and LINEAR_MIPMAP_LINEAR once the capture's mips are
        // built; GL's default GL_REPEAT wrap.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post-process linear repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let dummy = |dimension| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("unbound post-process texture"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let dummy_lut = dummy(wgpu::TextureDimension::D3);
        let dummy_2d = dummy(wgpu::TextureDimension::D2);
        Self {
            chain: Chain::with_filters(),
            params: Params::default(),
            layout,
            pipeline_layout,
            shader,
            pipelines: HashMap::new(),
            sampler,
            dummy_lut,
            dummy_2d,
            luts: HashMap::new(),
            targets: None,
            slots: Default::default(),
            next_slot: std::cell::Cell::new(0),
        }
    }

    /// The gate for the frame owner: whether a capture is needed.
    pub fn capture(&self) -> bool {
        self.chain.capture(&self.params)
    }

    fn ensure_pipeline(
        &mut self,
        device: &wgpu::Device,
        entry: &'static str,
        format: wgpu::TextureFormat,
    ) {
        let (layout, shader) = (&self.pipeline_layout, &self.shader);
        self.pipelines.entry((entry, format)).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vertex"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        });
    }

    /// (Re)create the capture and ping-pong textures on a size or format
    /// change.
    fn ensure_targets(
        &mut self,
        device: &wgpu::Device,
        size: [u32; 2],
        format: wgpu::TextureFormat,
    ) {
        if self
            .targets
            .as_ref()
            .is_some_and(|t| t.size == size && t.format == format)
        {
            return;
        }
        let mips = 32 - size[0].max(size[1]).max(1).leading_zeros();
        let make = |label, mips| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let textures = [
            make("post-process capture", mips),
            make("post-process ping", 1),
            make("post-process pong", 1),
        ];
        let views = [0, 1, 2].map(|i| textures[i].create_view(&Default::default()));
        let attachments = [0, 1, 2].map(|i| {
            textures[i].create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: 0,
                mip_level_count: Some(1),
                ..Default::default()
            })
        });
        let levels = (0..mips)
            .map(|level| {
                textures[0].create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        self.targets = Some(Targets {
            size,
            format,
            textures,
            views,
            attachments,
            levels,
        });
    }

    /// The RGBA 16^3 volume of a remapper, uploaded once per remapper.
    fn ensure_lut(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, remapper: &Remapper) {
        self.luts.entry(remapper.id).or_insert_with(|| {
            let texture = device.create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("colour remapper"),
                    size: wgpu::Extent3d {
                        width: LUT_SIZE as u32,
                        height: LUT_SIZE as u32,
                        depth_or_array_layers: LUT_SIZE as u32,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &lut_texels(&remapper.argb),
            );
            texture.create_view(&Default::default())
        });
    }

    fn draw(
        &self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        encoder: &mut wgpu::CommandEncoder,
        pass: &PassDraw<'_>,
    ) {
        let PassDraw {
            present,
            entry,
            format,
            target,
            clear,
            viewport,
            scissor,
            uniforms,
            scene,
            bloom,
            luts,
        } = *pass;
        let pipeline = &self.pipelines[&(entry, format)];
        let index = self.next_slot.get();
        self.next_slot.set(index + 1);
        let views = [
            scene.clone(),
            bloom.clone(),
            luts[0].clone(),
            luts[1].clone(),
            luts[2].clone(),
        ];
        let mut slots = self.slots.borrow_mut();
        let slot = match slots.entry((present, index)) {
            std::collections::hash_map::Entry::Occupied(e) if e.get().views == views => {
                e.into_mut()
            }
            e => {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("post-process pass uniforms"),
                    size: std::mem::size_of::<Uniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let group = self.bind_group(device, &buffer, scene, bloom, luts);
                let slot = DrawSlot {
                    uniforms: buffer,
                    group,
                    views,
                    written: None,
                };
                match e {
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        e.insert(slot);
                        e.into_mut()
                    }
                    std::collections::hash_map::Entry::Vacant(e) => e.insert(slot),
                }
            }
        };
        if slot
            .written
            .is_none_or(|w| bytemuck::bytes_of(&w) != bytemuck::bytes_of(&uniforms))
        {
            queue.write_buffer(&slot.uniforms, 0, bytemuck::bytes_of(&uniforms));
            slot.written = Some(uniforms);
        }
        let binding = &slot.group;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(entry),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some([x, y, w, h]) = viewport {
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
        }
        if let Some([x, y, w, h]) = scissor {
            if w == 0 || h == 0 {
                // Nothing inside the bounds; a clear still applies.
                return;
            }
            pass.set_scissor_rect(x, y, w, h);
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, binding, &[]);
        pass.draw(0..3, 0..1);
    }

    /// A draw's bind group over `uniforms`.
    fn bind_group(
        &self,
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
        scene: &wgpu::TextureView,
        bloom: &wgpu::TextureView,
        luts: [&wgpu::TextureView; 3],
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post-process pass"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(bloom),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(luts[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(luts[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(luts[2]),
                },
            ],
        })
    }

    /// One capture: `capture.scene` (a `capture.size` texture of
    /// `capture.format`, the resolved scene target) is copied inside
    /// `capture.scene_clip` into the black capture, the live chain runs, and
    /// the final pass writes `screen` inside its `bounds` (physical
    /// `[x, y, w, h]`). Returns false when nothing was live (the caller then
    /// presents the scene unprocessed).
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        encoder: &mut wgpu::CommandEncoder,
        capture: &Capture<'_>,
        screen: &Screen<'_>,
    ) -> bool {
        let Capture {
            scene,
            format,
            size,
            scene_clip,
        } = *capture;
        let Screen {
            view: screen,
            format: screen_format,
            bounds,
        } = *screen;
        let live = self.chain.live(&self.params);
        if live.is_empty() || size[0] == 0 || size[1] == 0 {
            return false;
        }
        self.next_slot.set(0);
        self.ensure_targets(device, size, format);
        // The counted remappers bind to units 1..3.
        let remap = self.params.remap.clone();
        let counted = if live.contains(&Effect::ColourRemapping) {
            remap.count.clamp(0, 3) as usize
        } else {
            0
        };
        for r in remap.remappers[..counted].iter().flatten() {
            self.ensure_lut(device, queue.queue(), r);
        }
        let steps = schedule(&live);
        for entry in ["copy", "downsample"] {
            self.ensure_pipeline(device, entry, format);
        }
        for step in &steps {
            let f = if step.output == PassOutput::Screen {
                screen_format
            } else {
                format
            };
            self.ensure_pipeline(device, step_entry(step), f);
        }
        let this = &*self;
        let t = this.targets.as_ref().unwrap();
        let mut luts = [&this.dummy_lut; 3];
        for (slot, r) in remap.remappers[..counted].iter().enumerate() {
            if let Some(r) = r {
                luts[slot] = &this.luts[&r.id];
            }
        }
        let full = Uniforms {
            geom: [1.0, 1.0, 1.0, 1.0],
            ..Default::default()
        };
        // The capture is cleared black, then the scene viewport is copied
        // into it.
        this.draw(
            device,
            queue,
            encoder,
            &PassDraw {
                present: false,
                entry: "copy",
                format,
                target: &t.attachments[0],
                clear: true,
                viewport: None,
                scissor: Some(clamp_rect(scene_clip, size)),
                uniforms: full,
                scene,
                bloom: &this.dummy_2d,
                luts,
            },
        );
        // Only the bloom bright pass minifies the capture; every other pass
        // samples level 0.
        if live.first() == Some(&Effect::Bloom) {
            for level in 1..t.levels.len() {
                this.draw(
                    device,
                    queue,
                    encoder,
                    &PassDraw {
                        present: false,
                        entry: "downsample",
                        format,
                        target: &t.levels[level],
                        clear: false,
                        viewport: None,
                        scissor: None,
                        uniforms: full,
                        scene: &t.levels[level - 1],
                        bloom: &this.dummy_2d,
                        luts,
                    },
                );
            }
        }
        let clip = clamp_rect(bounds, size);
        for step in &steps {
            let u = step_uniforms(step, &this.params, size, size);
            // The composite samples the scene copy as its scene and the
            // previous pass's output as its bloom texture.
            let (scene_view, bloom_view) = if step.effect == Effect::Bloom && step.pass == 3 {
                (&t.views[step.scene], &t.views[step.input])
            } else {
                (&t.views[step.input], &this.dummy_2d)
            };
            let pass = match step.output {
                PassOutput::Texture(i) => PassDraw {
                    present: false,
                    entry: step_entry(step),
                    format,
                    target: &t.attachments[i],
                    clear: false,
                    viewport: None,
                    scissor: None,
                    uniforms: u,
                    scene: scene_view,
                    bloom: bloom_view,
                    luts,
                },
                PassOutput::Screen => PassDraw {
                    present: false,
                    entry: step_entry(step),
                    format: screen_format,
                    target: screen,
                    clear: false,
                    viewport: Some([0, 0, size[0], size[1]]),
                    scissor: Some(clip),
                    uniforms: u,
                    scene: scene_view,
                    bloom: bloom_view,
                    luts,
                },
            };
            this.draw(device, queue, encoder, &pass);
        }
        true
    }
}

impl PostProcessor {
    /// Copy `scene` inside the screen's bounds onto it without effects (the
    /// scene drawn straight to the screen when no capture is open).
    pub fn present(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        size: [u32; 2],
        screen: &Screen<'_>,
    ) {
        let Screen {
            view: screen,
            format: screen_format,
            bounds: clip,
        } = *screen;
        let clip = clamp_rect(clip, size);
        if clip[2] == 0 || clip[3] == 0 {
            return;
        }
        self.ensure_pipeline(device, "copy", screen_format);
        let full = Uniforms {
            geom: [1.0, 1.0, 1.0, 1.0],
            ..Default::default()
        };
        self.next_slot.set(0);
        self.draw(
            device,
            queue,
            encoder,
            &PassDraw {
                present: true,
                entry: "copy",
                format: screen_format,
                target: screen,
                clear: false,
                viewport: Some([0, 0, size[0], size[1]]),
                scissor: Some(clip),
                uniforms: full,
                scene,
                bloom: &self.dummy_2d,
                luts: [&self.dummy_lut; 3],
            },
        );
    }
}

fn step_entry(step: &Step) -> &'static str {
    match (step.effect, step.pass) {
        (Effect::Bloom, 0) => "brightpass",
        (Effect::Bloom, 1 | 2) => "blur",
        (Effect::Bloom, _) => "composite",
        (Effect::Levels, _) => "levels",
        (Effect::ColourRemapping, _) => "remap",
    }
}

fn clamp_rect(rect: [u32; 4], size: [u32; 2]) -> [u32; 4] {
    let x0 = rect[0].min(size[0]);
    let y0 = rect[1].min(size[1]);
    let x1 = rect[0].saturating_add(rect[2]).min(size[0]);
    let y1 = rect[1].saturating_add(rect[3]).min(size[1]);
    [x0, y0, x1 - x0, y1 - y0]
}

#[cfg(test)]
#[path = "postprocess_tests.rs"]
mod tests;
