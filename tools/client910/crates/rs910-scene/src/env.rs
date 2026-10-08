//! The environment trailer of a LAND stream and the per-chunk environment
//! map: the trailer reader, the environment (sun, fog, bloom, levels, colour
//! remapping, skybox), static point lights and light types, and the
//! map/sun/fog bookkeeping.
//!
//! The trailer follows the 4x64x64 tile bodies in the same `Packet`:
//! 8 skipped bytes, then opcodes until the end of
//! the stream — `0` environment, `1` static point lights, `2` bloom, `3`
//! colour remapping, `128` skybox, `129` camera height offsets, `130`
//! force-ground-level; anything else is a decode error.
//!
//! What this module does not model: the fade between environments and
//! overrides (owned by `app.rs`, `fade_environment`), the environment
//! sampler object (only its id is kept; colour remapping objects are
//! `postprocess::RemapperCache`). The per-tile light references built for
//! entity lighting are in `model_lights.rs`. The skybox object behind
//! [`SkyboxRef`] is owned by `crate::skybox::SkyboxOwner`.

use crate::floor::SunLighting;
use crate::landscape_packet::Packet;
use crate::protocol910::terrain::RegionCopy;

/// Config group holding the light types.
pub const LIGHTTYPE_GROUP: u32 = 31;
/// Environment defaults.
pub const DEFAULT_SUN_COLOUR: i32 = 0xFF_FFFF;
pub const DEFAULT_SUN_AMBIENT: f32 = 1.152_343_8;
pub const DEFAULT_SUN_DIFFUSE: f32 = 0.699_218_75;
pub const DEFAULT_SUN_SHADOW: f32 = 1.2;
pub const DEFAULT_SUN_DIR: [f32; 3] = [-50.0, -60.0, -50.0];
pub const DEFAULT_FOG_COLOUR: i32 = 13_156_520;
/// Maximum number of lights the GPU path supports.
pub const GLX_MAX_LIGHTS: i32 = 6;

pub use rs910_protocol::server_prot::SkyboxRef;

/// One environment: sun, fog, bloom, levels, colour remapping and skybox.
#[derive(Clone, Debug, PartialEq)]
pub struct Environment {
    /// Sun colour (RGB24).
    pub sun_colour: i32,
    /// Sun ambient intensity.
    pub sun_ambient: f32,
    /// Sun diffuse intensity.
    pub sun_diffuse: f32,
    /// Sun shadow intensity.
    pub sun_shadow: f32,
    /// Sun direction.
    pub sun_dir: [f32; 3],
    /// Fog colour (RGB24).
    pub fog_colour: i32,
    /// Fog depth.
    pub fog_depth: i32,
    /// Material id of the environment sampler, or `-1` for the static
    /// default.
    pub sampler: i32,
    /// Bloom white point squared, intensity and threshold.
    pub bloom: [f32; 3],
    /// The skybox (`None` = no skybox).
    pub skybox: Option<SkyboxRef>,
    /// Levels: gamma, input min, input max, output min, output max.
    pub levels: [f32; 5],
    /// Colour remapping map and weight per slot.
    pub colour_remap: [(i32, f32); 3],
}

impl Environment {
    /// Equality of every field except the sun direction; the skybox by
    /// identity (the cache key here).
    #[must_use]
    pub fn equal_ignoring_sun_direction(&self, other: &Self) -> bool {
        self.sun_colour == other.sun_colour
            && self.sun_ambient == other.sun_ambient
            && self.sun_diffuse == other.sun_diffuse
            && self.sun_shadow == other.sun_shadow
            && self.bloom == other.bloom
            && self.fog_colour == other.fog_colour
            && self.fog_depth == other.fog_depth
            && self.sampler == other.sampler
            && self.skybox == other.skybox
            && self.levels == other.levels
            && self.colour_remap == other.colour_remap
    }
}

impl Default for Environment {
    /// The default environment.
    fn default() -> Self {
        Self {
            sun_colour: DEFAULT_SUN_COLOUR,
            sun_ambient: DEFAULT_SUN_AMBIENT,
            sun_diffuse: DEFAULT_SUN_DIFFUSE,
            sun_shadow: DEFAULT_SUN_SHADOW,
            sun_dir: DEFAULT_SUN_DIR,
            fog_colour: DEFAULT_FOG_COLOUR,
            fog_depth: 0,
            sampler: -1,
            bloom: [1.0, 0.25, 1.0],
            skybox: None,
            levels: [1.0, 0.0, 1.0, 0.0, 1.0],
            colour_remap: [(-1, 0.0); 3],
        }
    }
}

impl Environment {
    /// Decode the base environment record. `sun_enabled` is the
    /// lighting-detail preference being 1 with a GPU path that supports
    /// lights; when false the sun fields are consumed and reset to the
    /// defaults.
    pub fn decode(&mut self, p: &mut Packet<'_>, sun_enabled: bool) -> anyhow::Result<()> {
        let bits = p.g1()?;
        if sun_enabled {
            self.sun_colour = if bits & 1 == 0 {
                DEFAULT_SUN_COLOUR
            } else {
                p.g4s()?
            };
            self.sun_ambient = if bits & 2 == 0 {
                DEFAULT_SUN_AMBIENT
            } else {
                p.g2()? as f32 / 256.0
            };
            self.sun_diffuse = if bits & 4 == 0 {
                DEFAULT_SUN_DIFFUSE
            } else {
                p.g2()? as f32 / 256.0
            };
            self.sun_shadow = if bits & 8 == 0 {
                DEFAULT_SUN_SHADOW
            } else {
                p.g2()? as f32 / 256.0
            };
        } else {
            if bits & 1 != 0 {
                p.g4s()?;
            }
            if bits & 2 != 0 {
                p.g2()?;
            }
            if bits & 4 != 0 {
                p.g2()?;
            }
            if bits & 8 != 0 {
                p.g2()?;
            }
            self.sun_colour = DEFAULT_SUN_COLOUR;
            self.sun_shadow = DEFAULT_SUN_SHADOW;
            self.sun_diffuse = DEFAULT_SUN_DIFFUSE;
            self.sun_ambient = DEFAULT_SUN_AMBIENT;
        }
        self.sun_dir = if bits & 16 == 0 {
            DEFAULT_SUN_DIR
        } else {
            [p.g2s()? as f32, p.g2s()? as f32, p.g2s()? as f32]
        };
        self.fog_colour = if bits & 32 == 0 {
            DEFAULT_FOG_COLOUR
        } else {
            p.g4s()?
        };
        self.fog_depth = if bits & 64 == 0 { 0 } else { p.g2()? };
        self.sampler = if bits & 128 == 0 { -1 } else { p.g2()? };
        Ok(())
    }

    /// Decode the bloom terms.
    pub fn decode_bloom(&mut self, p: &mut Packet<'_>) -> anyhow::Result<()> {
        self.bloom = [p.gfloat()?, p.gfloat()?, p.gfloat()?];
        Ok(())
    }

    /// Decode the colour remapping into slot 0.
    pub fn decode_colour_remapping(&mut self, p: &mut Packet<'_>) -> anyhow::Result<()> {
        self.colour_remap[0] = (p.g2()?, p.gfloat()?);
        Ok(())
    }

    /// Decode the skybox reference.
    pub fn decode_skybox(&mut self, p: &mut Packet<'_>) -> anyhow::Result<()> {
        let kind = p.g2()?;
        let a = p.g2s()?;
        let b = p.g2s()?;
        let c = p.g2s()?;
        let yaw_offset = p.g2()?;
        self.skybox = Some(SkyboxRef {
            kind,
            a,
            b,
            c,
            yaw_offset,
        });
        Ok(())
    }

    /// The sun state pushed to the toolkit: ambient is
    /// `(brightness * 0.1 + 0.7 + anti_macro) * sun_ambient`, the direction
    /// is truncated to integers and shifted left 2. Note the manager's own
    /// sun direction is the direction actually pushed; it is only refreshed at
    /// the end of the rebuild, see [`EnvState::sun_direction`].
    #[must_use]
    pub fn sun_lighting(
        &self,
        sun_dir: [f32; 3],
        brightness_pref: i32,
        anti_macro: f32,
    ) -> SunLighting {
        let ambient = (brightness_pref as f32 * 0.1 + 0.7 + anti_macro) * self.sun_ambient;
        SunLighting::from_set_sun(
            ambient,
            self.sun_diffuse,
            self.sun_shadow,
            ((sun_dir[0] as i32) << 2) as f32,
            ((sun_dir[1] as i32) << 2) as f32,
            ((sun_dir[2] as i32) << 2) as f32,
        )
    }

    /// The fog arguments `(colour, depth, 0)` for the toolkit, depth `-1`
    /// when the fog preference is off.
    #[must_use]
    pub fn fog_args(&self, fog_pref: bool) -> (i32, i32, i32) {
        let depth = (self.fog_depth + 256) << 2;
        (self.fog_colour, if fog_pref { depth } else { -1 }, 0)
    }
}

/// The fog state handed to the toolkit: colour (RGB24), depth (`-1` = off)
/// and `end_inset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FogState {
    pub colour: i32,
    pub depth: i32,
    pub end_inset: i32,
}

/// The `DistanceFogPlane/DistanceFogColour` shader uniforms, over scene
/// units in the same space as the view matrix they were derived from. Zero
/// when fog is off.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct FogPlanes {
    pub distance_plane: [f32; 4],
    pub distance_colour: [f32; 3],
    /// Fog start and end for diagnostics; `None` when fog is off.
    pub range: Option<(f32, f32)>,
}

impl FogState {
    /// The fog uniforms: `fogEnd = far - end_inset`,
    /// `fogStart = max(fogEnd - depth, near_min)`, plane `(0,0,1,-fogStart)`
    /// pushed through the transposed view matrix,
    /// scaled by `1 / (fogEnd - fogStart)`. `far` is the projection far
    /// plane, `near_min` is `-m14 / m10`, `view` the row-major 4x4 view
    /// entries.
    #[must_use]
    pub fn planes(&self, far: f32, near_min: f32, view: &[f32; 16]) -> FogPlanes {
        if self.depth <= 0 {
            return FogPlanes::default();
        }
        let fog_end = far - self.end_inset as f32;
        let mut fog_start = fog_end - self.depth as f32;
        if fog_start < near_min {
            fog_start = near_min;
        }
        // Row-vector product with the transposed view:
        // out[c] = sum_r v[r] * viewT[r*4 + c] = sum_r v[r] * view[c*4 + r].
        let v = [0.0_f32, 0.0, 1.0, -fog_start];
        let mut plane = [0.0_f32; 4];
        for c in 0..4 {
            // Summed in the fixed order w, z, x, y so the float rounding is reproducible.
            let t = |r: usize| view[c * 4 + r];
            plane[c] = t(3) * v[3] + t(2) * v[2] + t(0) * v[0] + t(1) * v[1];
        }
        let scale = 1.0 / (fog_end - fog_start);
        for p in &mut plane {
            *p *= scale;
        }
        FogPlanes {
            distance_plane: plane,
            distance_colour: [
                ((self.colour >> 16) & 0xFF) as f32 / 255.0,
                ((self.colour >> 8) & 0xFF) as f32 / 255.0,
                (self.colour & 0xFF) as f32 / 255.0,
            ],
            range: Some((fog_start, fog_end)),
        }
    }
}

/// The sun colour split into 0..1 red, green and blue.
#[must_use]
pub fn sun_rgb(colour: i32) -> [f32; 3] {
    [
        (colour & 0xFF_0000) as f32 / 1.671_168E7,
        (colour & 0xFF00) as f32 / 65280.0,
        (colour & 0xFF) as f32 / 255.0,
    ]
}

/// Interpolate the post-process fields (bloom, levels, colour remapping) of
/// `out` between `a` and `b` at `t`.
pub fn interpolate(out: &mut Environment, a: &Environment, b: &Environment, t: f32) {
    // Field order: white point squared, intensity, threshold.
    for i in 0..3 {
        out.bloom[i] = (b.bloom[i] - a.bloom[i]) * t + a.bloom[i];
    }
    for i in 0..5 {
        out.levels[i] = (b.levels[i] - a.levels[i]) * t + a.levels[i];
    }
    let a_sum = a.colour_remap[1].1 + a.colour_remap[0].1 + a.colour_remap[2].1;
    let b_sum = b.colour_remap[0].1 + b.colour_remap[1].1 + b.colour_remap[2].1;
    let sum = (b_sum - a_sum) * t + a_sum;
    let a_empty = a.colour_remap.iter().all(|(m, _)| *m == -1);
    let b_empty = b.colour_remap.iter().all(|(m, _)| *m == -1);
    if sum == 0.0 {
        out.colour_remap = [(-1, 0.0); 3];
    } else if a_empty {
        for i in 0..3 {
            let map = b.colour_remap[i].0;
            out.colour_remap[i] = (
                map,
                if map == -1 {
                    0.0
                } else {
                    b.colour_remap[i].1 * t
                },
            );
        }
    } else if b_empty {
        for i in 0..3 {
            let map = a.colour_remap[i].0;
            out.colour_remap[i] = (
                map,
                if map == -1 {
                    0.0
                } else {
                    (1.0 - t) * a.colour_remap[i].1
                },
            );
        }
    } else {
        let inv = 1.0 - t;
        let mut count = 0usize;
        let mut maps = [-1i32; 6];
        let mut weights = [0.0f32; 6];
        for i in 0..3 {
            if a.colour_remap[i].0 > -1 {
                maps[count] = a.colour_remap[i].0;
                weights[count] = a.colour_remap[i].1 * inv;
                count += 1;
            }
        }
        let from_a = count;
        for i in 0..3 {
            if b.colour_remap[i].0 > -1 {
                let w = b.colour_remap[i].1 * t;
                for j in 0..from_a {
                    if b.colour_remap[i].0 == maps[j] {
                        weights[j] += w;
                        break;
                    }
                    if from_a - 1 == j {
                        maps[count] = b.colour_remap[i].0;
                        weights[count] = w;
                        count += 1;
                    }
                }
            }
        }
        if count > 3 {
            let total: f32 = weights[..count].iter().sum();
            sort_descending(&mut weights, &mut maps, 0, count as i32 - 1);
            let kept: f32 = weights[..3].iter().sum();
            let scale = total / kept;
            for w in &mut weights[..3] {
                *w *= scale;
            }
        }
        for i in 0..3 {
            out.colour_remap[i] = (maps[i], weights[i]);
        }
    }
}

/// Quicksort of `weights` (descending) carrying `ids`.
pub fn sort_descending(weights: &mut [f32], ids: &mut [i32], lo: i32, hi: i32) {
    if lo >= hi {
        return;
    }
    let mid = ((lo + hi) / 2) as usize;
    let (lo_u, hi_u) = (lo as usize, hi as usize);
    let pivot = weights[mid];
    weights.swap(mid, hi_u);
    let pivot_id = ids[mid];
    ids.swap(mid, hi_u);
    let mut store = lo_u;
    for i in lo_u..hi_u {
        if weights[i] > pivot {
            weights.swap(i, store);
            ids.swap(i, store);
            store += 1;
        }
    }
    weights[hi_u] = weights[store];
    weights[store] = pivot;
    ids[hi_u] = ids[store];
    ids[store] = pivot_id;
    sort_descending(weights, ids, lo, store as i32 - 1);
    sort_descending(weights, ids, store as i32 + 1, hi);
}

/// Duration in milliseconds of the fade between chunk environments.
pub const CHUNK_FADE_MS: u32 = 5047;

/// The sun terms of an environment frame: the direction the manager holds,
/// the brightness preference (default 3) and the anti-macro brightness bias
/// (random per rebuild in the client; 0 elsewhere).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunSettings {
    pub direction: [f32; 3],
    pub brightness_pref: i32,
    pub anti_macro: f32,
}

/// The projection terms the fog planes need: the far plane, the minimum near
/// distance and the row-major view matrix.
#[derive(Clone, Copy, Debug)]
pub struct FogReference<'a> {
    pub far: f32,
    pub near_min: f32,
    pub view: &'a [f32; 16],
}

/// Everything one frame's shaders take from the current environment (the
/// app owns the cross-fade between environments, `app.rs`
/// `fade_environment`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvFrame {
    /// White point squared, intensity, threshold from the current environment.
    pub bloom: [f32; 3],
    /// Levels: gamma, input min/max, output min/max.
    pub levels: [f32; 5],
    /// Colour remapping map and weight per slot.
    pub colour_remap: [(i32, f32); 3],
    pub sampler: i32,
    pub sun: SunLighting,
    pub sun_rgb: [f32; 3],
    pub fog: FogPlanes,
    /// The clear colour (the fog colour) as 0..1 RGB.
    pub clear: [f32; 3],
}

impl EnvFrame {
    /// The sun and fog state for `env`, the manager's `sun_direction`,
    /// the brightness preference (default 3), `anti_macro` (the brightness
    /// bias, random per rebuild in the client; pass 0), the fog preference,
    /// and the projection far / near-min plus the row-major view matrix
    /// (scene units).
    #[must_use]
    pub fn build(
        env: &Environment,
        sun: SunSettings,
        fog_pref: bool,
        reference: FogReference<'_>,
    ) -> Self {
        let SunSettings {
            direction: sun_direction,
            brightness_pref,
            anti_macro,
        } = sun;
        let FogReference {
            far,
            near_min,
            view,
        } = reference;
        let (colour, depth, end_inset) = env.fog_args(fog_pref);
        let fog = FogState {
            colour,
            depth,
            end_inset,
        }
        .planes(far, near_min, view);
        Self {
            bloom: env.bloom,
            levels: env.levels,
            colour_remap: env.colour_remap,
            sampler: env.sampler,
            sun: env.sun_lighting(sun_direction, brightness_pref, anti_macro),
            sun_rgb: sun_rgb(env.sun_colour),
            fog,
            clear: [
                ((env.fog_colour >> 16) & 0xFF) as f32 / 255.0,
                ((env.fog_colour >> 8) & 0xFF) as f32 / 255.0,
                (env.fog_colour & 0xFF) as f32 / 255.0,
            ],
        }
    }

    /// The default environment with fog on, for callers without a map.
    #[must_use]
    pub fn default_for(far: f32, near_min: f32, view: &[f32; 16]) -> Self {
        Self::build(
            &Environment::default(),
            SunSettings {
                direction: DEFAULT_SUN_DIR,
                brightness_pref: 3,
                anti_macro: 0.0,
            },
            true,
            FogReference {
                far,
                near_min,
                view,
            },
        )
    }
}

/// What decoding a landscape's environment trailer needs besides the
/// packet: the light types, how many lights the toolkit supports and
/// whether the sun properties are read.
#[derive(Clone, Copy)]
pub struct EnvironmentSettings<'a> {
    pub light_types: &'a LightTypeStore,
    pub max_lights: i32,
    pub sun_enabled: bool,
}

/// A light type: the flicker wave shape, its speed, its amplitude and its
/// base offset. The light intensity is `(amplitude * wave(phase) >> 11) +
/// offset` over 2048, see `light_animation.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightType {
    pub wave: i32,
    pub speed: i32,
    pub amplitude: i32,
    pub offset: i32,
}

impl Default for LightType {
    fn default() -> Self {
        Self {
            wave: 0,
            speed: 2048,
            amplitude: 2048,
            offset: 0,
        }
    }
}

impl LightType {
    /// Decode a light type record.
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let mut p = Packet::new(bytes);
        let mut t = Self::default();
        loop {
            let op = p.g1()?;
            match op {
                0 => return Ok(t),
                1 => t.wave = p.g1()?,
                2 => t.speed = p.g2()?,
                3 => t.amplitude = p.g2()?,
                4 => t.offset = p.g2s()?,
                _ => anyhow::bail!("light type: unknown opcode {op}"),
            }
        }
    }
}

/// The light types of config archive group [`LIGHTTYPE_GROUP`].
#[derive(Clone, Debug, Default)]
pub struct LightTypeStore {
    entries: std::collections::BTreeMap<u32, LightType>,
}

impl LightTypeStore {
    pub fn load(pack: &crate::cache::Pack) -> anyhow::Result<Self> {
        rs910_core::profile::scope!("load light types");
        let files = pack.read_group(crate::flo::FLO_ARCHIVE, LIGHTTYPE_GROUP)?;
        let mut entries = std::collections::BTreeMap::new();
        for (id, bytes) in files {
            entries.insert(
                id,
                LightType::decode(&bytes).map_err(|e| anyhow::anyhow!("light type {id}: {e:#}"))?,
            );
        }
        Ok(Self { entries })
    }

    /// The light type for `id`; a missing id yields the defaults (an empty
    /// type).
    #[must_use]
    pub fn get(&self, id: i32) -> LightType {
        u32::try_from(id)
            .ok()
            .and_then(|id| self.entries.get(&id).copied())
            .unwrap_or_default()
    }
}

/// A static point light and its placed position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticLight {
    /// Scene level the light sits on (`& 0x7`).
    pub level: u8,
    /// Bit `0x8` — also lights the levels above.
    pub above: bool,
    /// Bit `0x10` — also lights the levels below.
    pub below: bool,
    /// Position after placement (scene units; y is the terrain height minus
    /// the wire height).
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Radius: `(span_radius << 9) + 256`.
    pub radius: i32,
    /// Colour (RGB24), looked up in the HSL palette by a 16-bit index.
    pub colour: i32,
    /// Flicker kind (`31` = from a `LightType`).
    pub flicker: u8,
    /// `(g1 & 0xE0) << 3`.
    pub phase: i32,
    /// Flicker wave shape, base offset, amplitude and speed (see [`LightType`]).
    pub wave: i32,
    pub offset: i32,
    pub amplitude: i32,
    pub speed: i32,
    /// Light group (`g2s`, `-1` none).
    pub group: i32,
    /// Span runs: `span_radius * 2 + 1` packed `(start << 8 | length)`.
    pub span_runs: Vec<u16>,
}

impl StaticLight {
    /// Decode a light record with scene size 9 and shift 2, plus the light
    /// type lookup when the flicker kind is 31.
    pub fn decode(p: &mut Packet<'_>, light_types: &LightTypeStore) -> anyhow::Result<Self> {
        let scene_size = 9;
        let shift = 2;
        let mut level = p.g1()?;
        let above = level & 0x8 != 0;
        let below = level & 0x10 != 0;
        level &= 0x7;
        let x = p.g2()? << shift;
        let z = p.g2()? << shift;
        let y = p.g2()? << shift;
        let span_radius = p.g1()?;
        let count = (span_radius * 2 + 1) as usize;
        let mut span_runs = Vec::with_capacity(count);
        for _ in 0..count {
            let raw = p.g2()? as u16;
            let mut start = i32::from(raw >> 8);
            let mut len = i32::from(raw & 0xFF);
            if start >= count as i32 {
                start = count as i32 - 1;
            }
            if len > count as i32 - start {
                len = count as i32 - start;
            }
            span_runs.push(((start << 8) | len) as u16);
        }
        let radius = (span_radius << scene_size) + ((1 << scene_size) >> 1);
        // The GPU path always has the HSL palette.
        let colour = crate::colour::hsl_tables().rgb[(p.g2()? & 0xFFFF) as usize];
        let flicker_bits = p.g1()?;
        let flicker = (flicker_bits & 0x1F) as u8;
        let phase = (flicker_bits & 0xE0) << 3;
        let mut light = Self {
            level: level as u8,
            above,
            below,
            x,
            y,
            z,
            radius,
            colour,
            flicker,
            phase,
            wave: 0,
            offset: 0,
            amplitude: 2048,
            speed: 2048,
            group: -1,
            span_runs,
        };
        if flicker != 31 {
            light.apply_flicker_preset();
        }
        light.group = p.g2s()?;
        if flicker == 31 {
            let t = light_types.get(p.g2()?);
            // Flicker shape from the light type.
            light.wave = t.wave;
            light.offset = t.offset;
            light.amplitude = t.amplitude;
            light.speed = t.speed;
        }
        Ok(light)
    }

    /// The flicker presets by kind.
    fn apply_flicker_preset(&mut self) {
        let (a, b, c, d) = match self.flicker {
            2 => (1, 0, 2048, 2048),
            3 => (1, 0, 2048, 4096),
            4 => (4, 0, 2048, 2048),
            5 => (4, 0, 2048, 8192),
            6 => (3, 1280, 768, 2048),
            7 => (3, 1280, 768, 4096),
            8 => (3, 1024, 1024, 2048),
            9 => (3, 1024, 1024, 4096),
            10 => (3, 1536, 512, 2048),
            11 => (3, 1536, 512, 4096),
            12 => (2, 0, 2048, 2048),
            13 => (2, 0, 2048, 8192),
            14 => (1, 1280, 768, 2048),
            15 => (1, 1536, 512, 4096),
            16 => (1, 1792, 256, 8192),
            _ => (0, 0, 2048, 2048),
        };
        self.wave = a;
        self.offset = b;
        self.amplitude = c;
        self.speed = d;
    }
}

/// The per-chunk environment map plus the loader's per-window trailer
/// state (camera height offsets, force-ground flags) and the placed static
/// lights, in placement order.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvState {
    /// Map dimensions in chunks (`sizeX >> 3`, `sizeZ >> 3`).
    pub grid_x: usize,
    pub grid_z: usize,
    /// The environment per chunk, `x * grid_z + z`; `None` = never set (falls
    /// back to the default environment).
    pub map: Vec<Option<Environment>>,
    /// The placed static lights.
    pub lights: Vec<StaticLight>,
    /// Camera height offsets `[4][maxTileX + 1][maxTileZ + 1]`
    /// (`None` until opcode 129 is seen; each level allocated on demand).
    pub camera_height: Option<[Option<Vec<i8>>; 4]>,
    /// Force-ground-level flags, `x * grid_z + z`.
    pub force_ground: Vec<bool>,
    /// The sun direction the manager holds (default `(-50,-60,-50)`),
    /// refreshed from the map at the end of the rebuild.
    pub sun_direction: [f32; 3],
    /// The yaw offset of the 2D skybox path, a global
    /// that the LAST decoded skybox trailer wins.
    pub skybox_yaw_offset: i32,
    max_tile_x: usize,
    max_tile_z: usize,
}

impl EnvState {
    /// An empty map for a `max_tile_x x max_tile_z` window.
    #[must_use]
    pub fn new(max_tile_x: usize, max_tile_z: usize) -> Self {
        let grid_x = max_tile_x >> 3;
        let grid_z = max_tile_z >> 3;
        Self {
            grid_x,
            grid_z,
            map: vec![None; grid_x * grid_z],
            lights: Vec::new(),
            camera_height: None,
            force_ground: vec![false; grid_x * grid_z],
            sun_direction: DEFAULT_SUN_DIR,
            skybox_yaw_offset: 0,
            max_tile_x,
            max_tile_z,
        }
    }

    /// The environment of a chunk, or the default environment.
    #[must_use]
    pub fn environment(&self, chunk_x: usize, chunk_z: usize) -> Environment {
        self.map
            .get(chunk_x * self.grid_z + chunk_z)
            .and_then(|e| e.clone())
            .unwrap_or_default()
    }

    /// The environment the game targets: the chunk of the
    /// player's route waypoint (`>> 3`), falling back to the window centre
    /// (`sizeX >> 4, sizeZ >> 4`) when out of range.
    #[must_use]
    pub fn target_environment(&self, tile_x: i32, tile_z: i32) -> Environment {
        let mut cx = tile_x >> 3;
        let mut cz = tile_z >> 3;
        if cx < 0 || cx >= self.grid_x as i32 || cz < 0 || cz >= self.grid_z as i32 {
            cx = (self.max_tile_x >> 4) as i32;
            cz = (self.max_tile_z >> 4) as i32;
        }
        self.environment(cx as usize, cz as usize)
    }

    /// Adopt the centre chunk's sun direction if that chunk has an
    /// environment (called at the end of the rebuild).
    pub fn adopt_centre_sun_direction(&mut self) {
        if let Some(Some(env)) = self
            .map
            .get((self.max_tile_x >> 4) * self.grid_z + (self.max_tile_z >> 4))
        {
            self.sun_direction = env.sun_dir;
        }
    }

    /// Read the trailer for the square whose south-west tile is `(tx, tz)`
    /// in window coordinates. `level_heightmap[level][x * (maxTileZ + 1) + z]`
    /// is the loader's height grid; `max_lights` is the number of lights the
    /// toolkit supports; `sun_enabled` gates the sun fields.
    pub fn read_normal_environment(
        &mut self,
        p: &mut Packet<'_>,
        [tx, tz]: [i32; 2],
        level_heightmap: &[Vec<i32>],
        settings: EnvironmentSettings<'_>,
        underwater: bool,
    ) -> anyhow::Result<()> {
        let EnvironmentSettings {
            light_types,
            max_lights,
            sun_enabled,
        } = settings;
        if underwater {
            return Ok(());
        }
        p.pos += 8;
        let max_x = self.max_tile_x as i32;
        let max_z = self.max_tile_z as i32;
        let cam_seen = false;
        let mut env: Option<Environment> = None;
        let mut force_ground = false;
        while !p.is_empty() {
            let op = p.g1()?;
            match op {
                0 => {
                    let e = env.get_or_insert_with(Environment::default);
                    e.decode(p, sun_enabled)?;
                }
                1 => {
                    let count = p.g1()?;
                    for _ in 0..count {
                        let mut light = StaticLight::decode(p, light_types)?;
                        if max_lights > 0 {
                            let x = light.x + (tx << 9);
                            let z = light.z + (tz << 9);
                            let cx = x >> 9;
                            let cz = z >> 9;
                            if cx >= 0 && cz >= 0 && cx < max_x && cz < max_z {
                                let h = level_heightmap[light.level as usize]
                                    [cx as usize * (self.max_tile_z + 1) + cz as usize];
                                light.x = x;
                                light.y = h - light.y;
                                light.z = z;
                                // Capped at 65253 lights.
                                if self.lights.len() < 65253 {
                                    self.lights.push(light);
                                }
                            }
                        }
                    }
                }
                2 => env
                    .get_or_insert_with(Environment::default)
                    .decode_bloom(p)?,
                3 => env
                    .get_or_insert_with(Environment::default)
                    .decode_colour_remapping(p)?,
                128 => {
                    let e = env.get_or_insert_with(Environment::default);
                    e.decode_skybox(p)?;
                    self.skybox_yaw_offset =
                        e.skybox.map_or(self.skybox_yaw_offset, |s| s.yaw_offset);
                }
                129 => {
                    let stride = self.max_tile_z + 1;
                    let cells = (self.max_tile_x + 1) * stride;
                    let cam = self.camera_height.get_or_insert([None, None, None, None]);
                    for level in 0..4 {
                        let mode = p.g1b()?;
                        if mode == 0 && cam[level].is_some() {
                            let (x0, x1) = clamp_span(tx, max_x);
                            let (z0, z1) = clamp_span(tz, max_z);
                            let grid = cam[level].as_mut().expect("checked");
                            for x in x0..x1 {
                                for z in z0..z1 {
                                    grid[x as usize * stride + z as usize] = 0;
                                }
                            }
                        } else if mode == 1 {
                            let grid = cam[level].get_or_insert_with(|| vec![0; cells]);
                            for ox in (0..64).step_by(4) {
                                for oz in (0..64).step_by(4) {
                                    let v = p.g1b()?;
                                    for x in tx + ox..tx + ox + 4 {
                                        for z in tz + oz..tz + oz + 4 {
                                            if x >= 0 && x < max_x && z >= 0 && z < max_z {
                                                grid[x as usize * stride + z as usize] = v;
                                            }
                                        }
                                    }
                                }
                            }
                        } else if mode == 2 {
                            if cam[level].is_none() {
                                cam[level] = Some(vec![0; cells]);
                            }
                            if level > 0 {
                                let (x0, x1) = clamp_span(tx, max_x);
                                let (z0, z1) = clamp_span(tz, max_z);
                                let (lower, upper) = cam.split_at_mut(level);
                                let src = lower[level - 1].as_ref();
                                let dst = upper[0].as_mut().expect("allocated above");
                                for x in x0..x1 {
                                    for z in z0..z1 {
                                        let i = x as usize * stride + z as usize;
                                        // The level below is read unconditionally;
                                        // a missing one reads as 0.
                                        dst[i] = src.map_or(0, |s| s[i]);
                                    }
                                }
                            }
                        }
                    }
                }
                130 => force_ground = true,
                _ => anyhow::bail!("environment trailer: unknown opcode {op} at {}", p.pos - 1),
            }
        }
        for i in 0..8 {
            for j in 0..8 {
                let cx = (tx >> 3) + i;
                let cz = (tz >> 3) + j;
                if cx >= 0 && cx < (max_x >> 3) && cz >= 0 && cz < (max_z >> 3) {
                    let idx = cx as usize * self.grid_z + cz as usize;
                    if let Some(e) = &env {
                        self.map[idx] = Some(e.clone());
                    }
                    self.force_ground[idx] = force_ground;
                }
            }
        }
        if !cam_seen {
            if let Some(cam) = self.camera_height.as_mut() {
                let stride = self.max_tile_z + 1;
                for grid in cam.iter_mut().flatten() {
                    for i in 0..16 {
                        for j in 0..16 {
                            let x = (tx >> 2) + i;
                            let z = (tz >> 2) + j;
                            // Hard-coded 26 (= 104 / 4) bounds.
                            if (0..26).contains(&x) && (0..26).contains(&z) {
                                grid[x as usize * stride + z as usize] = 0;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Read the trailer of a region-template copy.
    /// Region templates reuse a source 64x64 LAND trailer for one rotated 8x8
    /// destination chunk; environment profiles, static lights and camera
    /// offsets therefore need the same source-level/chunk transform as the
    /// landscape reader instead of falling back to defaults.
    pub fn read_region_environment(
        &mut self,
        p: &mut Packet<'_>,
        copy: RegionCopy,
        level_heightmap: &[Vec<i32>],
        settings: EnvironmentSettings<'_>,
        underwater: bool,
    ) -> anyhow::Result<()> {
        let RegionCopy {
            level: destination_level,
            tile_x: destination_x,
            tile_z: destination_z,
            src_level: source_level,
            src_chunk_x: source_chunk_x,
            src_chunk_z: source_chunk_z,
            rotation,
        } = copy;
        let EnvironmentSettings {
            light_types,
            max_lights,
            sun_enabled,
        } = settings;
        if underwater {
            return Ok(());
        }
        p.pos += 8;
        let max_x = self.max_tile_x as i32;
        let max_z = self.max_tile_z as i32;
        let source_x = (source_chunk_x & 7) * 8;
        let source_z = (source_chunk_z & 7) * 8;
        let mut cam_seen = false;
        let mut env: Option<Environment> = None;
        let mut force_ground = false;
        while !p.is_empty() {
            let op = p.g1()?;
            match op {
                0 => env
                    .get_or_insert_with(Environment::default)
                    .decode(p, sun_enabled)?,
                1 => {
                    let count = p.g1()?;
                    for _ in 0..count {
                        let mut light = StaticLight::decode(p, light_types)?;
                        if max_lights <= 0
                            || light.level as usize != source_level
                            || (light.x >> 9) < source_x
                            || (light.x >> 9) >= source_x + 8
                            || (light.z >> 9) < source_z
                            || (light.z >> 9) >= source_z + 8
                        {
                            continue;
                        }
                        let rotate_x = match rotation & 3 {
                            0 => light.x & 0xFFF,
                            1 => light.z & 0xFFF,
                            2 => 4095 - (light.x & 0xFFF),
                            _ => 4095 - (light.z & 0xFFF),
                        };
                        let rotate_z = match rotation & 3 {
                            0 => light.z & 0xFFF,
                            1 => 4095 - (light.x & 0xFFF),
                            2 => 4095 - (light.z & 0xFFF),
                            _ => light.x & 0xFFF,
                        };
                        let x = (destination_x << 9) + rotate_x;
                        let z = (destination_z << 9) + rotate_z;
                        let cx = x >> 9;
                        let cz = z >> 9;
                        if cx < 0 || cz < 0 || cx >= max_x || cz >= max_z {
                            continue;
                        }
                        let height = level_heightmap[destination_level]
                            [cx as usize * (self.max_tile_z + 1) + cz as usize];
                        light.level = destination_level as u8;
                        light.x = x;
                        light.y = height - light.y;
                        light.z = z;
                        if self.lights.len() < 65253 {
                            self.lights.push(light);
                        }
                    }
                }
                2 => env
                    .get_or_insert_with(Environment::default)
                    .decode_bloom(p)?,
                3 => env
                    .get_or_insert_with(Environment::default)
                    .decode_colour_remapping(p)?,
                128 => {
                    let e = env.get_or_insert_with(Environment::default);
                    e.decode_skybox(p)?;
                    self.skybox_yaw_offset =
                        e.skybox.map_or(self.skybox_yaw_offset, |s| s.yaw_offset);
                }
                129 => {
                    let stride = self.max_tile_z + 1;
                    let cells = (self.max_tile_x + 1) * stride;
                    let cam = self.camera_height.get_or_insert([None, None, None, None]);
                    for source_plane in 0..4 {
                        let mode = p.g1b()?;
                        if mode == 0 && cam[destination_level].is_some() {
                            if source_plane <= source_level {
                                let (x0, x1) = clamp_span(destination_x, max_x);
                                let (z0, z1) = clamp_span(destination_z, max_z);
                                let grid = cam[destination_level].as_mut().expect("checked");
                                for x in x0..x1 {
                                    for z in z0..z1 {
                                        grid[x as usize * stride + z as usize] = 0;
                                    }
                                }
                            }
                        } else if mode == 1 {
                            let grid = cam[destination_level].get_or_insert_with(|| vec![0; cells]);
                            for ox in (0..64).step_by(4) {
                                for oz in (0..64).step_by(4) {
                                    let value = p.g1b()?;
                                    if source_plane > source_level
                                        || ox < source_x
                                        || ox >= source_x + 8
                                        || oz < source_z
                                        || oz >= source_z + 8
                                    {
                                        continue;
                                    }
                                    for local_x in ox..ox + 4 {
                                        for local_z in oz..oz + 4 {
                                            if local_x < source_x
                                                || local_x >= source_x + 8
                                                || local_z < source_z
                                                || local_z >= source_z + 8
                                            {
                                                continue;
                                            }
                                            let rx = local_x - source_x;
                                            let rz = local_z - source_z;
                                            let (dx, dz) = match rotation & 3 {
                                                0 => (rx, rz),
                                                1 => (rz, 7 - rx),
                                                2 => (7 - rx, 7 - rz),
                                                _ => (7 - rz, rx),
                                            };
                                            let x = destination_x + dx;
                                            let z = destination_z + dz;
                                            if x >= 0 && x < max_x && z >= 0 && z < max_z {
                                                grid[x as usize * stride + z as usize] = value;
                                                cam_seen = true;
                                            }
                                        }
                                    }
                                }
                            }
                        } else if mode == 2 {
                            if cam[destination_level].is_none() {
                                cam[destination_level] = Some(vec![0; cells]);
                            }
                            if source_plane > 0 && source_plane <= source_level {
                                let (lower, upper) = cam.split_at_mut(destination_level);
                                let src = lower.last().and_then(Option::as_ref);
                                let dst = upper[0].as_mut().expect("allocated above");
                                if let Some(src) = src {
                                    let (x0, x1) = clamp_span(destination_x, max_x);
                                    let (z0, z1) = clamp_span(destination_z, max_z);
                                    for x in x0..x1 {
                                        for z in z0..z1 {
                                            let i = x as usize * stride + z as usize;
                                            dst[i] = src[i];
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                130 => force_ground = true,
                _ => anyhow::bail!(
                    "region environment trailer: unknown opcode {op} at {}",
                    p.pos - 1
                ),
            }
        }
        let cx = destination_x >> 3;
        let cz = destination_z >> 3;
        if cx >= 0 && cz >= 0 && cx < (max_x >> 3) && cz < (max_z >> 3) {
            let index = cx as usize * self.grid_z + cz as usize;
            if let Some(environment) = env {
                self.map[index] = Some(environment);
            }
            self.force_ground[index] = force_ground;
        }
        if !cam_seen {
            if let Some(cam) = self.camera_height.as_mut() {
                let stride = self.max_tile_z + 1;
                let (x0, x1) = clamp_span(destination_x, max_x);
                let (z0, z1) = clamp_span(destination_z, max_z);
                if let Some(grid) = cam[destination_level].as_mut() {
                    for x in x0..x1 {
                        for z in z0..z1 {
                            grid[x as usize * stride + z as usize] = 0;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Serialise for byte comparison with the committed scene goldens
    /// (big-endian):
    /// `"ENV1", i32 gridX, i32 gridZ, per cell (x outer): u8 present,
    /// [i32 sunColour, f32 ambient, f32 diffuse, f32 shadow, f32[3] dir,
    /// i32 fogColour, i32 fogDepth, i32 sampler, f32[3] bloom, u8 hasSkybox,
    /// [i32 kind, i32 a, i32 b, i32 c, i32 skybox_yaw_offset], (i32, f32)[3] remap,
    /// f32[5] levels]; i32 lightCount, per light: u8 level, u8 flags
    /// (above | below << 1), i32 x, y, z, radius, colour, u8 flicker, i32
    /// phase, wave, offset, amplitude, speed, group, i32 spanCount, u16[] spans;
    /// u8 hasCameraHeights, per level u8 present [i8[(maxX+1)*(maxZ+1)]];
    /// u8[gridX*gridZ] forceGround; f32[3] sunDirection`.
    #[must_use]
    pub fn to_dump(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(b"ENV1");
        o.extend_from_slice(&(self.grid_x as i32).to_be_bytes());
        o.extend_from_slice(&(self.grid_z as i32).to_be_bytes());
        let f = |o: &mut Vec<u8>, v: f32| o.extend_from_slice(&v.to_bits().to_be_bytes());
        let i = |o: &mut Vec<u8>, v: i32| o.extend_from_slice(&v.to_be_bytes());
        for cell in &self.map {
            match cell {
                None => o.push(0),
                Some(e) => {
                    o.push(1);
                    i(&mut o, e.sun_colour);
                    f(&mut o, e.sun_ambient);
                    f(&mut o, e.sun_diffuse);
                    f(&mut o, e.sun_shadow);
                    for v in e.sun_dir {
                        f(&mut o, v);
                    }
                    i(&mut o, e.fog_colour);
                    i(&mut o, e.fog_depth);
                    i(&mut o, e.sampler);
                    for v in e.bloom {
                        f(&mut o, v);
                    }
                    match e.skybox {
                        None => o.push(0),
                        Some(s) => {
                            o.push(1);
                            for v in [s.kind, s.a, s.b, s.c, s.yaw_offset] {
                                i(&mut o, v);
                            }
                        }
                    }
                    for (m, w) in e.colour_remap {
                        i(&mut o, m);
                        f(&mut o, w);
                    }
                    for v in e.levels {
                        f(&mut o, v);
                    }
                }
            }
        }
        i(&mut o, self.lights.len() as i32);
        for l in &self.lights {
            o.push(l.level);
            o.push(u8::from(l.above) | (u8::from(l.below) << 1));
            for v in [l.x, l.y, l.z, l.radius, l.colour] {
                i(&mut o, v);
            }
            o.push(l.flicker);
            for v in [l.phase, l.wave, l.offset, l.amplitude, l.speed, l.group] {
                i(&mut o, v);
            }
            i(&mut o, l.span_runs.len() as i32);
            for s in &l.span_runs {
                o.extend_from_slice(&s.to_be_bytes());
            }
        }
        match &self.camera_height {
            None => o.push(0),
            Some(levels) => {
                o.push(1);
                for level in levels {
                    match level {
                        None => o.push(0),
                        Some(grid) => {
                            o.push(1);
                            o.extend(grid.iter().map(|v| *v as u8));
                        }
                    }
                }
            }
        }
        o.extend(self.force_ground.iter().map(|&b| u8::from(b)));
        for v in self.sun_direction {
            f(&mut o, v);
        }
        o
    }
}

/// The `[start, start + 64)` span clamped to `[0, max]`, each end clamped
/// independently.
fn clamp_span(start: i32, max: i32) -> (i32, i32) {
    let mut a = start;
    let mut b = start + 64;
    if start < 0 {
        a = 0;
    } else if start >= max {
        a = max;
    }
    if b < 0 {
        b = 0;
    } else if b >= max {
        b = max;
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_environment_values() {
        let e = Environment::default();
        assert_eq!(e.sun_colour, 0xFF_FFFF);
        // 13156520 = 0xC8C0A8 (200, 192, 168).
        assert_eq!(e.fog_colour, 0xC8_C0A8);
        assert_eq!(e.sun_dir, [-50.0, -60.0, -50.0]);
        assert_eq!(e.fog_args(true), (0xC8_C0A8, 1024, 0));
        assert_eq!(e.fog_args(false).1, -1);
    }

    #[test]
    fn region_environment_applies_destination_chunk_trailer() {
        // Eight landscape bytes are skipped before opcode 130, matching the
        // region trailer layout. The force-ground flag must land in the
        // destination chunk, even though the source chunk is reused directly.
        let mut bytes = vec![0; 8];
        bytes.push(130);
        let mut env = EnvState::new(16, 16);
        let heights = vec![vec![0; 17 * 17]; 4];
        env.read_region_environment(
            &mut Packet::new(&bytes),
            RegionCopy {
                level: 1,
                tile_x: 8,
                tile_z: 0,
                src_level: 0,
                src_chunk_x: 0,
                src_chunk_z: 0,
                rotation: 0,
            },
            &heights,
            EnvironmentSettings {
                light_types: &LightTypeStore::default(),
                max_lights: 0,
                sun_enabled: false,
            },
            false,
        )
        .unwrap();
        assert!(env.force_ground[env.grid_z]);
    }

    #[test]
    fn decode_reads_every_property_bit() {
        // bits 0xFF: colour, ambient, diffuse, shadow, dir, fog colour, depth, sampler
        let bytes = [
            0xFF, 0x00, 0x12, 0x34, 0x56, 0x01, 0x00, 0x00, 0x80, 0x01, 0x33, 0xFF, 0xCE, 0xFF,
            0xC4, 0xFF, 0xCE, 0x00, 0x11, 0x22, 0x33, 0x00, 0x40, 0x00, 0x07,
        ];
        let mut p = Packet::new(&bytes);
        let mut e = Environment::default();
        e.decode(&mut p, true).unwrap();
        assert_eq!(e.sun_colour, 0x123456);
        assert_eq!(e.sun_ambient, 1.0);
        assert_eq!(e.sun_diffuse, 0.5);
        assert_eq!(e.sun_shadow, 307.0 / 256.0);
        assert_eq!(e.sun_dir, [-50.0, -60.0, -50.0]);
        assert_eq!(e.fog_colour, 0x112233);
        assert_eq!(e.fog_depth, 64);
        assert_eq!(e.sampler, 7);
        assert!(p.is_empty());
        // Sun disabled: the same bytes are consumed but the sun is default.
        let mut p = Packet::new(&bytes);
        let mut e = Environment::default();
        e.decode(&mut p, false).unwrap();
        assert_eq!(e.sun_colour, 0xFF_FFFF);
        assert_eq!(e.fog_depth, 64);
        assert!(p.is_empty());
    }

    #[test]
    fn flicker_presets_set_the_light_wave() {
        let mut l = StaticLight {
            level: 0,
            above: false,
            below: false,
            x: 0,
            y: 0,
            z: 0,
            radius: 0,
            colour: 0,
            flicker: 6,
            phase: 0,
            wave: 0,
            offset: 0,
            amplitude: 0,
            speed: 0,
            group: -1,
            span_runs: Vec::new(),
        };
        l.apply_flicker_preset();
        assert_eq!(
            (l.wave, l.offset, l.amplitude, l.speed),
            (3, 1280, 768, 2048)
        );
        l.flicker = 99;
        l.apply_flicker_preset();
        assert_eq!((l.wave, l.offset, l.amplitude, l.speed), (0, 0, 2048, 2048));
    }

    #[test]
    fn fog_planes_follow_set_fog_parameters() {
        // Identity view: plane = (0, 0, 1, -fogStart) / (fogEnd - fogStart).
        let ident = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let fog = FogState {
            colour: 0xC8C8E8,
            depth: 1024,
            end_inset: 0,
        }
        .planes(14844.0, 200.0, &ident);
        let (start, end) = fog.range.unwrap();
        assert_eq!((start, end), (13820.0, 14844.0));
        assert_eq!(
            fog.distance_plane,
            [0.0, 0.0, 1.0 / 1024.0, -13820.0 / 1024.0]
        );
        assert_eq!(
            fog.distance_colour,
            [200.0 / 255.0, 200.0 / 255.0, 232.0 / 255.0]
        );
        // Depth larger than the far plane clamps the start at near_min.
        let fog = FogState {
            colour: 0,
            depth: 100000,
            end_inset: 0,
        }
        .planes(14844.0, 200.0, &ident);
        assert_eq!(fog.range.unwrap().0, 200.0);
        // Off.
        assert_eq!(
            FogState {
                colour: 0,
                depth: -1,
                end_inset: 0
            }
            .planes(1.0, 0.0, &ident),
            FogPlanes::default()
        );
        // A translated view: the plane's w picks up the eye's z.
        let mut view = ident;
        view[14] = -5000.0; // translation z (row 3)
        let fog = FogState {
            colour: 0,
            depth: 1024,
            end_inset: 0,
        }
        .planes(14844.0, 200.0, &view);
        assert!((fog.distance_plane[3] - (-13820.0 - 5000.0) / 1024.0).abs() < 1e-3);
    }

    #[test]
    fn sun_rgb_matches_set_sun_split() {
        assert_eq!(sun_rgb(0xFFFFFF), [1.0, 1.0, 1.0]);
        assert_eq!(sun_rgb(0x800000), [0x80 as f32 / 255.0, 0.0, 0.0]);
    }
}
