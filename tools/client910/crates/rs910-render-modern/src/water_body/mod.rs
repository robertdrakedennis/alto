//! Modern water (renderer plan M7): the water surfaces, their per-vertex data
//! and the maths of the reflection, CPU only. The GPU half (targets,
//! pipelines, the refraction copy, the reflection pass and the water draws)
//! is `crate::frame::water`; the WGSL is [`WATER_WGSL`].
//!
//! # What the modern client draws
//!
//! - **Two sources of water.** A water patch is built per map file 8 record
//!   (an oriented box at the record's height, with a flow and a type), and
//!   one from the terrain's water layer (map file 5's water tiles) with a
//!   water type from the square's list. A patch is a flat grid whose
//!   per-vertex depth is the patch height minus the terrain height (bilinear
//!   terrain height), so the terrain hides it where it rises above the water
//!   and the shader fades it in with depth.
//! - **This renderer draws the terrain layer.** Its terrain is the classic
//!   floor, whose water tiles are exactly map file 5's water tiles (the A5
//!   census: every classic-only overlay tile is a modern water tile) and
//!   whose heights there are the water *surface*: the tests prove, tile for
//!   tile, that a water tile's second height byte is the classic land height
//!   (the surface) and its first is the underwater land (file 4) height (the
//!   bed). At the default water detail no bed is built, so a file-8 patch
//!   drawn as the modern client draws it (a flat box at the record's height,
//!   over the bed) would have nothing under it; its height unit is unproven
//!   (25 over the Lumbridge river, whose surface is classic height 0). The
//!   water surface is therefore the classic floor's water batches (materials
//!   with a water effect, 2/4/8/9), at the classic heights, and file 8
//!   supplies what the terrain lacks: each vertex's **flow** (the record whose
//!   box holds the vertex: the two bytes are divided by 50; in world axes,
//!   proven below by the Lumbridge river running south in its records) and
//!   **water type**.
//! - **Depth** per vertex: the classic floor's own water depth when it has
//!   one (water detail 2), else the ground of map file 5 under the vertex
//!   minus the surface: a water tile's bed lies `32 h` below its own surface
//!   (M10's rule, `crate::terrain::tile_height_up`; lane Q-FIN: M7 put it at
//!   `32 h` below classic height 0, right only where the surface is 0), a
//!   land tile's ground is its own height.
//!
//! # Shading
//!
//! The WGSL follows the modern client's water program. Variant: common water,
//! normal maps, reflection, refraction, global environment mapping, specular
//! lighting, direct sun lighting, sun shadows and distance fog, with the
//! extinction of the foam variant (no foam). The older two-phase flow map is
//! not used; where the two differ the modern program is followed.
//!
//! - Mesh ([`water_mesh`]): every water triangle has three vertices of its
//!   own, each carrying all three corners' flows (the three patch-flow slots)
//!   and its corner index, so each slot's flow-oriented UVs are affine inside
//!   the triangle and the fragment blends the slots by the interpolated
//!   corner mask: no seams at triangle edges.
//! - Normals: a macro normal (the maps at a tenth of the UV drifting by
//!   `(0.1, -0.13) * 0.25`) plus per slot a detail normal whose UVs run along
//!   the flow and are pushed by the macro normal; each map adds its XY and
//!   slope weighted by `clamp(|flow| * 6 + still strength)`. The slot's flow
//!   noise (no 910 source) is a hash of the slot's flow, constant over a
//!   patch: it moves the UVs by `noise * speed * t`, and M7's per-vertex hash
//!   stretched them by tens of repeats per tile as the clock ran, the
//!   stripes and seams of the M7 frames.
//! - Fresnel `clamp(Schlick(F0 0.28), 0, 0.6)`; opacity `q * fresnel` with
//!   the soft edge's `q = clamp(depth * (0.004 + visibility share))`.
//! - Reflection: the planar image (full viewport resolution) at the pixel
//!   pushed by `normal.xz * min(depth, 32)`, composited over the sky: the
//!   reflection pass clears to transparent black and its alpha-blended draws
//!   leave the reflected scene premultiplied, so an uncovered texel is the
//!   sky, as the modern fallback makes it. The sky (a stand-in for the
//!   environment map) is a gradient from the frame's clear colour at the
//!   horizon to a deeper zenith: the classic environment cubes M7 sampled are
//!   material maps (a lava cave at Draynor, which painted the sea red; a
//!   jungle at Lumbridge), not the sky of the place. Tinted by `mix(albedo,
//!   1, fresnel)`, weighted by the contribution and strength.
//!   The reflection pass supplies the planar camera: mirrored about the water
//!   height near the camera, clipped by an oblique near plane. The image is
//!   the mirror image, tested to 0.1 pixel against the mirrored point and
//!   against the terrain's occlusion; over the Lumbridge river, whose
//!   surface lies 400 to nearly a thousand fine units below its banks (up to
//!   two tile widths), the reflected rays of a steep camera clear the bank, so the
//!   houses on it reflect near the camera: physically so, not a
//!   misplacement.
//! - Sun specular: half vector, the sun's colour squared, its elevation,
//!   `pow(dot(n, h), power / 4)` and the modern intensity law, under the cascade
//!   lookup offset by the normal.
//! - The water body (refraction with extinction): the light under the
//!   surface (the sun's diffuse, the ambient and the point lights) on the
//!   opaque water colour (the classic water colour under the type's tint,
//!   [`TypeLook`]) and on the bed; per channel the bed's light crosses
//!   `exp(-path / depth_rgb)` of the path through the water (red first).
//!   Where every sample behind the pixel shows geometry, the scene copy is
//!   the bed (the refraction, the path running to it); the classic floor
//!   builds no bed at the default water detail, so elsewhere the bed is the
//!   nearest bank's colour ([`beds`], this port's addition) and the depth
//!   is at most the bank's slope times the distance to the shore (the 910
//!   depths do not reach 0 at the shoreline): the bank runs on under the
//!   surface and the shallows meet it without an edge. The result is
//!   `mix(body, reflection + specular, q * fresnel)`, the distance fog on
//!   the reflected part as the modern in/out scattering does (alpha pulled
//!   to 1) and on the synthesised body.
//!
//! 910 data vs the modern client's uniforms: the WATERTYPE opcodes name two
//! normal-map materials (bound as maps 0 and 1; the modern client has three);
//! ops 2/4 (the maps' repeat in tiles) and op 6 (the type's colour) are read
//! as inferred ([`TypeLook`]); nothing is proven for the flow speed,
//! still-water strength, sample weights and distortions, BRDF parameters,
//! reflection contribution/strength or extinction: those are [`LOOK`]'s
//! chosen values. Foam, caustics and the modern shadowed body light are lane
//! Q-FX's ([`crate::water_body::effects`], hooked into `fs_water` by
//! markers). Not drawn: emissive, light scattering, the vertex waves.

use std::collections::HashMap;

use rs910_config::nxt::map_terrain::{decode_terrain, TERRAIN_SIDE};
use rs910_config::nxt::map_water::{decode_water, NxtWaterPatch};
use rs910_config::nxt::{map_group, MAP_ARCHIVE, TERRAIN_FILE, WATER_FILE};

use crate::floor::FloorGeometry;
use crate::texture::MaterialStore;

/// The classic material effects that are water (2 water, 4/8/9 sea;
/// `Material::effect`).
pub const WATER_EFFECTS: [u8; 4] = [2, 4, 8, 9];

/// Fine units per classic height step (`(h * 8) << 2`).
pub const HEIGHT_STEP: f32 = 32.0;

/// Depth used where the bed is unknown (instanced regions, upper levels).
pub const DEFAULT_DEPTH: f32 = 512.0;

/// The soft edge's law (without extinction): the surface's opacity per fine
/// unit of water depth, `clamp(depth * 0.004)`.
pub const OPACITY_PER_UNIT: f32 = 0.004;

/// The water fresnel: scalar Schlick with F0 0.28, clamped to 0.6.
pub const FRESNEL_F0: f32 = 0.28;
pub const FRESNEL_MAX: f32 = 0.6;

/// The water clock wraps here (seconds): the modern clock grows without
/// bound; the flow-oriented UVs advance continuously, so a wrap is
/// a jump in the pattern, once an hour.
pub const TIME_WRAP_SECONDS: i64 = 3600;

/// The water clock (seconds, wrapped) at logic-clock time `millis`
/// (from the injected clock: fixed-clock frames repeat).
#[must_use]
pub fn water_seconds(millis: i64) -> f32 {
    millis.rem_euclid(TIME_WRAP_SECONDS * 1000) as f32 / 1000.0
}

/// The frame's water constants: the modern client's per-type values whose
/// 910 sources are unknown (`rs910_config::nxt::water_type` proves none of
/// them), so they are chosen values; each field says what it stands for or
/// that it is this port's addition (the per-type part: [`TypeLook`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The flow speed of all maps.
    pub flow_speed: f32,
    /// The flow noise's scale (the maps' texture scales are
    /// [`TypeLook::texture_scales`]).
    pub flow_noise_scale: f32,
    /// The normal strength of still water.
    pub still_water_normal_strength: f32,
    /// The detail maps' (weight, UV distortion) pairs.
    pub detail: [[f32; 2]; 2],
    /// The macro maps' (weight, UV distortion) pairs.
    pub macro_: [[f32; 2]; 2],
    /// The normal BRDF parameters: fresnel offset, fog offset, specular power
    /// (times 4), specular intensity.
    pub brdf: [f32; 4],
    /// The reflection map's contribution and the reflection strength.
    pub reflection: [f32; 2],
    /// The planar image's push by the normal (`normal.xz * min(depth,
    /// 32)` pixels), as a factor.
    pub distortion: f32,
    /// The extinction depths per channel in fine units: the path over which
    /// each channel of the light under the surface falls to `1 / e`.
    pub extinction_depths: [f32; 3],
    /// The soft edge's visibility share, `1 / (visibility / 0.002)`
    /// per fine unit.
    pub visibility_share: f32,
    /// The longest path through the water the extinction counts.
    pub max_path: f32,
    /// The opaque water colour's brightness against the classic water colour
    /// under the type's tint: the deep water is darker than the classic
    /// surface colour (the modern opaque colour is its own value).
    pub deep_brightness: f32,
    /// The bed colour's brightness against the bank's ([`beds`]): measured
    /// so the shallows meet the bank without a step (the river view: the
    /// last land pixel `(55, 57, 55)`, the first water pixel `(55, 60,
    /// 57)`; the forward pass lights the bank brighter than the vertex
    /// colour times the texture mean predicts).
    pub bed_brightness: f32,
    /// The bank's slope: at most this much depth per fine unit from the
    /// shoreline ([`beds`]).
    pub bank_slope: f32,
}

/// See [`Look`] (the choices: the plan's M7 and Q-WATER2 sections).
pub const LOOK: Look = Look {
    flow_speed: 0.25,
    flow_noise_scale: 0.1,
    still_water_normal_strength: 0.35,
    detail: [[1.0, 0.0], [0.7, 0.5]],
    macro_: [[1.0, 0.0], [0.7, 0.5]],
    brdf: [0.0, 0.0, 256.0, 0.3],
    reflection: [1.0, 0.6],
    distortion: 1.0,
    extinction_depths: [160.0, 380.0, 560.0],
    visibility_share: 0.000_67,
    max_path: 2048.0,
    deep_brightness: 0.5,
    bed_brightness: 2.0,
    bank_slope: 0.35,
};

/// The normal maps' repeat in tiles where the type gives none (type 0's).
pub const DEFAULT_MAP_TILES: f32 = 6.0;

/// The per-type part of the look, from the WATERTYPE fields this port reads
/// (both inferred, not proven by the modern client's uniform code):
///
/// - ops 2 and 4 (`u16 / 256`): the normal maps' repeat in tiles, maps 1
///   and 2 (6 for the river and pond types 0/1, 12 for the sea type 12,
///   the sea's larger waves): the texture scales are
///   `1 / (tiles * 512)`;
/// - op 6 (RGB, the one colour the type keeps for itself): the tint of the
///   opaque water colour (pale cyan, rgb `b7dbda`, for the river and the
///   pond; sea blue, rgb `38738f`, for the sea), display-referred.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeLook {
    pub texture_scales: [f32; 2],
    pub tint: [f32; 3],
}

impl TypeLook {
    #[must_use]
    pub fn of(t: Option<&rs910_config::nxt::water_type::NxtWaterType>) -> Self {
        let tiles = |v: Option<f32>| v.filter(|&v| v > 0.0).unwrap_or(DEFAULT_MAP_TILES);
        let rgb = t.and_then(|t| t.op6_rgb).unwrap_or(0x00b7_dbda);
        let channel = |shift: u32| (((rgb >> shift) & 0xff) as f32 / 255.0).powf(2.2);
        Self {
            texture_scales: [
                1.0 / (tiles(t.and_then(|t| t.op2)) * 512.0),
                1.0 / (tiles(t.and_then(|t| t.op4)) * 512.0),
            ],
            tint: [channel(16), channel(8), channel(0)],
        }
    }
}

/// The flow-oriented UV's advance along the flow:
/// `|flow * speed * t|` with `t = seconds * 0.5`.
#[must_use]
pub fn flow_advance(flow: [f32; 2], speed: f32, seconds: f32) -> f32 {
    (flow[0] * flow[0] + flow[1] * flow[1]).sqrt() * speed * seconds * 0.5
}

/// `WaterFragment`'s fresnel for the cosine between the surface normal
/// and the direction to the eye.
#[must_use]
pub fn fresnel(cos: f32) -> f32 {
    (FRESNEL_F0 + (1.0 - FRESNEL_F0) * (1.0 - cos.clamp(0.0, 1.0)).powi(5)).clamp(0.0, FRESNEL_MAX)
}

/// `CLIENT910_MODERN_WATER=debug5` (verification): the reflection pass writes
/// each fragment's camera-local position and the water shows the planar
/// image at its own pixel (no normal offset), its coverage in alpha.
pub const DEBUG_REFLECTION_POSITIONS: u32 = 5;

/// Whether material `id` is drawn as water.
#[must_use]
pub fn is_water_material(materials: &MaterialStore, id: i32) -> bool {
    u32::try_from(id)
        .ok()
        .and_then(|id| materials.get(id))
        .is_some_and(|m| WATER_EFFECTS.contains(&m.effect))
}

/// The debug and verification switch `CLIENT910_MODERN_WATER`: `off` draws
/// water as the M1 floor batch (the frame before M7, byte for byte), `env`
/// skips the planar reflection (the sky gradient only), `debug<N>` shows
/// one term (1 fresnel over a flat normal, 2 the flow displacement, 3 the
/// reflected colour, 4 the water body, 5 [`DEBUG_REFLECTION_POSITIONS`], 6
/// the shading depth, 7 the tangent normal, 8 the normal maps' mip levels).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    On,
    Off,
    EnvOnly,
    Debug(u32),
}

/// The mode, read once.
#[must_use]
pub fn mode() -> Mode {
    static MODE: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();
    *MODE.get_or_init(
        || match std::env::var("CLIENT910_MODERN_WATER").as_deref() {
            Ok("off") => Mode::Off,
            Ok("env") => Mode::EnvOnly,
            Ok(v) if v.starts_with("debug") => v[5..].parse().map_or(Mode::On, Mode::Debug),
            _ => Mode::On,
        },
    )
}

/// The sign of a record's rotation about the vertical in the scene's
/// (classic) X/Z axes. The modern client builds the quaternion `(axis sin(t
/// pi), cos(t pi))` about its Y-up axis; the classic axes keep X and Z, so the sense in X/Z is
/// what the coverage test picks (`patch_rotation_sense_covers_the_river`:
/// the sense that puts the rotated boxes on the Lumbridge river's water
/// tiles).
pub const ROTATION_SENSE: f32 = -1.0;

/// One file-8 record in scene-local fine units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patch {
    pub centre: [f32; 2],
    pub half: [f32; 2],
    /// Rotation about the vertical: cos and sin of the angle (the record's
    /// turns, [`ROTATION_SENSE`]).
    pub cos_sin: [f32; 2],
    /// `flow / 50`, world X and Z.
    pub flow: [f32; 2],
    pub water_type: u16,
}

impl Patch {
    fn from_record(r: &NxtWaterPatch, origin: [f32; 2]) -> Self {
        // Only the vertical component of the axis turns the box in X/Z;
        // records with a zero axis (a "no rotation" of 1.0 turns) stay
        // unrotated.
        let axis_y = r.rotation_axis[1];
        let angle = if axis_y.abs() > 0.5 {
            r.rotation_turns * std::f32::consts::TAU * axis_y.signum() * ROTATION_SENSE
        } else {
            0.0
        };
        Self {
            centre: [
                origin[0] + f32::from(r.tile_x) * 512.0 + 256.0,
                origin[1] + f32::from(r.tile_z) * 512.0 + 256.0,
            ],
            half: [f32::from(r.size_x) * 256.0, f32::from(r.size_z) * 256.0],
            cos_sin: [angle.cos(), angle.sin()],
            flow: r.flow_scaled(),
            water_type: r.water_type,
        }
    }

    /// Whether scene-local `(x, z)` is inside the box.
    #[must_use]
    pub fn contains(&self, x: f32, z: f32) -> bool {
        let (dx, dz) = (x - self.centre[0], z - self.centre[1]);
        let [c, s] = self.cos_sin;
        let lx = dx * c + dz * s;
        let lz = -dx * s + dz * c;
        lx.abs() <= self.half[0] && lz.abs() <= self.half[1]
    }
}

/// One tile's map-file-5 water data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileWater {
    pub water: bool,
    /// The first height byte: at a water tile the classic underwater (bed)
    /// height, the bed's depth below the surface in `32`-unit steps; at a
    /// land tile its height code.
    pub bed: u8,
    /// The water tile's second height byte: the classic land (surface) height.
    pub surface: Option<u8>,
}

impl TileWater {
    /// The classic y (down) of the ground at this tile's south-west vertex on
    /// level 0: M10's `crate::terrain::tile_height_up` over nothing (a water
    /// tile's bed `32 bed` below its surface code's height, a land tile its
    /// own height code's), negated.
    #[must_use]
    pub fn ground_y(&self) -> f32 {
        let up = match self.surface {
            Some(surface) => height_offset(surface) - HEIGHT_STEP * f32::from(self.bed),
            None => height_offset(self.bed),
        };
        -up
    }
}

/// The offset of height code `c` over the level below
/// (y up): `32 c`, `c == 1` none, `c == 0` 960. The same law as
/// `crate::terrain::height_offset` (M10, proven over the pack), kept here so the
/// water module does not depend on the terrain's (which reads the water's
/// material test).
fn height_offset(code: u8) -> f32 {
    match code {
        0 => 960.0,
        1 => 0.0,
        c => HEIGHT_STEP * f32::from(c),
    }
}

/// The NXT water data of one installed scene (level 0 of map file 5, every
/// file-8 record of the squares under the scene).
#[derive(Clone, Debug, Default)]
pub struct WaterMap {
    /// The scene's base tile (`SceneSnapshot::floor_base`).
    pub base: [i32; 2],
    /// Vertices per side (`tiles + 1`).
    pub dims: [usize; 2],
    /// Level-0 tiles `x * dims[1] + z` over the vertex grid (the tile whose
    /// south-west corner is the vertex).
    pub tiles: Vec<TileWater>,
    pub patches: Vec<Patch>,
    /// The most frequent record type under the scene (0 without records):
    /// the type of water outside every record.
    pub default_type: u16,
    /// Map squares decoded (0: no pack or no map files).
    pub squares: usize,
}

impl WaterMap {
    /// Decode the squares under a `tiles_x` x `tiles_z` scene at `base`.
    pub fn load(pack: &crate::cache::Pack, base: [i32; 2], tiles: [usize; 2]) -> Self {
        let dims = [tiles[0] + 1, tiles[1] + 1];
        let mut map = Self {
            base,
            dims,
            tiles: vec![TileWater::default(); dims[0] * dims[1]],
            ..Self::default()
        };
        let lo = [base[0].div_euclid(64), base[1].div_euclid(64)];
        let hi = [
            (base[0] + dims[0] as i32).div_euclid(64),
            (base[1] + dims[1] as i32).div_euclid(64),
        ];
        let mut types: HashMap<u16, usize> = HashMap::new();
        for sx in lo[0]..=hi[0] {
            for sz in lo[1]..=hi[1] {
                let (Ok(ux), Ok(uz)) = (u32::try_from(sx), u32::try_from(sz)) else {
                    continue;
                };
                if ux >= 128 || uz >= 256 {
                    continue;
                }
                let group = map_group(ux, uz);
                let Ok(files) = pack.read_group(MAP_ARCHIVE, group) else {
                    continue;
                };
                map.squares += 1;
                let origin = [
                    ((sx * 64 - base[0]) * 512) as f32,
                    ((sz * 64 - base[1]) * 512) as f32,
                ];
                if let Some(Ok(records)) = files.get(&WATER_FILE).map(|b| decode_water(group, b)) {
                    for r in &records {
                        *types.entry(r.water_type).or_default() += 1;
                        map.patches.push(Patch::from_record(r, origin));
                    }
                }
                let Some(Ok(levels)) = files.get(&TERRAIN_FILE).map(|b| decode_terrain(group, b))
                else {
                    continue;
                };
                let Some(level0) = levels.iter().find(|l| l.level == 0) else {
                    continue;
                };
                for lx in 0..64 {
                    for lz in 0..64 {
                        let (x, z) = (sx * 64 + lx - base[0], sz * 64 + lz - base[1]);
                        if x < 0 || z < 0 || x as usize >= dims[0] || z as usize >= dims[1] {
                            continue;
                        }
                        let t = &level0.tiles[(lx as usize + 1) * TERRAIN_SIDE + lz as usize + 1];
                        map.tiles[x as usize * dims[1] + z as usize] = TileWater {
                            water: t.is_water(),
                            bed: t.height,
                            surface: t.water_height,
                        };
                    }
                }
            }
        }
        map.default_type = types
            .into_iter()
            .max_by_key(|&(t, n)| (n, std::cmp::Reverse(t)))
            .map_or(0, |(t, _)| t);
        map
    }

    fn tile(&self, x: i32, z: i32) -> Option<&TileWater> {
        if x < 0 || z < 0 || x as usize >= self.dims[0] || z as usize >= self.dims[1] {
            return None;
        }
        self.tiles.get(x as usize * self.dims[1] + z as usize)
    }

    /// Whether scene tile `(x, z)` is an NXT water tile.
    #[must_use]
    pub fn is_water_tile(&self, x: i32, z: i32) -> bool {
        self.tile(x, z).is_some_and(|t| t.water)
    }

    /// The bed's classic y (down positive) at scene-local fine `(x, z)`: the
    /// bilinear ground of the four surrounding vertices
    /// ([`TileWater::ground_y`]: a water vertex's bed `32 h` below its
    /// surface).
    #[must_use]
    pub fn bed_y(&self, x: f32, z: f32) -> Option<f32> {
        let (fx, fz) = (x / 512.0, z / 512.0);
        let (x0, z0) = (fx.floor() as i32, fz.floor() as i32);
        let (tx, tz) = (fx - x0 as f32, fz - z0 as f32);
        let h = |dx: i32, dz: i32| self.tile(x0 + dx, z0 + dz).map(TileWater::ground_y);
        let (a, b, c, d) = (h(0, 0)?, h(1, 0)?, h(0, 1)?, h(1, 1)?);
        let south = a + (b - a) * tx;
        let north = c + (d - c) * tx;
        Some(south + (north - south) * tz)
    }

    /// The flow and water type at scene-local `(x, z)`: the containing
    /// record nearest by centre, else no flow and the default type.
    #[must_use]
    pub fn flow_at(&self, x: f32, z: f32) -> ([f32; 2], u16) {
        let mut best: Option<(f32, &Patch)> = None;
        for p in self.patches.iter().filter(|p| p.contains(x, z)) {
            let d = (x - p.centre[0]).powi(2) + (z - p.centre[1]).powi(2);
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, p));
            }
        }
        best.map_or(([0.0; 2], self.default_type), |(_, p)| {
            (p.flow, p.water_type)
        })
    }
}

/// The per-vertex water attributes of one floor level (vertex stream 3 of
/// the water pipeline): depth below the surface (fine units), flow X and
/// Z, and the type in `w` (negative: the depth is the default guess).
#[must_use]
pub fn vertex_attributes(g: &FloorGeometry, level: usize, map: Option<&WaterMap>) -> Vec<[f32; 4]> {
    let stride = g.stride_floats;
    (0..g.vertex_count)
        .map(|i| {
            let f = &g.stream0[i * stride..(i + 1) * stride];
            let (x, y, z) = (f[0], f[1], f[2]);
            let (flow, water_type) = map.map_or(([0.0; 2], 0), |m| m.flow_at(x, z));
            // The classic floor's own depth (water detail 2), else the bed of
            // map file 5 under level 0.
            let depth = if g.has_depth {
                Some(f[5].max(0.0))
            } else if level == 0 {
                map.and_then(|m| m.bed_y(x, z))
                    .map(|bed| (bed - y).max(0.0))
            } else {
                None
            };
            let w = f32::from(water_type);
            match depth {
                Some(d) => [d, flow[0], flow[1], w],
                None => [DEFAULT_DEPTH, flow[0], flow[1], -1.0 - w],
            }
        })
        .collect()
}

/// The bed under each floor vertex (this port's addition: at the default
/// water detail no bed is built, and the modern shallow water shows the bed):
/// the land batches' colour where land meets the vertex (the shoreline:
/// the vertex colour times the batch texture's mean `texture_mean`, linear
/// light, averaged over the land triangles there), carried from the shoreline across the water triangles to the
/// nearest water vertices, and the distance (fine units, X/Z) to that
/// shoreline along the water triangles' edges (a shortest-path walk).
/// `None`: no land reaches the vertex's water.
#[must_use]
pub fn beds(
    g: &FloorGeometry,
    water: &[bool],
    mut texture_mean: impl FnMut(i32) -> [f32; 3],
) -> Vec<Option<Bed>> {
    let n = g.vertex_count;
    let stride = g.stride_floats;
    let xz = |v: usize| [g.stream0[v * stride], g.stream0[v * stride + 2]];
    // The classic floor does not share vertices between batches' tiles, so
    // the land meets the water by position.
    let key = |v: usize| {
        let [x, z] = xz(v);
        (x.round() as i32, z.round() as i32)
    };
    let mut land: HashMap<(i32, i32), [f32; 4]> = HashMap::new();
    let mut water_tris: Vec<[usize; 3]> = Vec::new();
    for (i, tris) in g.tile_tris.iter().enumerate() {
        let Some(tris) = tris else { continue };
        for (b, batch) in g.batches.iter().enumerate() {
            let owned = batch.tri_mask.get(i).copied().unwrap_or(0);
            for (t, tri) in tris.chunks_exact(3).enumerate() {
                if owned & (1_u32 << (t % 32)) == 0 {
                    continue;
                }
                let tri = [
                    usize::from(tri[0]),
                    usize::from(tri[1]),
                    usize::from(tri[2]),
                ];
                if tri.iter().any(|&v| v >= n) {
                    continue;
                }
                if water.get(b).copied().unwrap_or(false) {
                    water_tris.push(tri);
                    continue;
                }
                let mean = texture_mean(batch.material);
                for v in tri {
                    let Some(&c) = batch.colours.get(v) else {
                        continue;
                    };
                    let c = c as u32;
                    let s = land.entry(key(v)).or_default();
                    let lin =
                        |x: u32| crate::post::tonemap::display_to_linear((x & 0xff) as f32 / 255.0);
                    s[0] += lin(c) * mean[0];
                    s[1] += lin(c >> 8) * mean[1];
                    s[2] += lin(c >> 16) * mean[2];
                    s[3] += 1.0;
                }
            }
        }
    }
    let mut out: Vec<Option<Bed>> = vec![None; n];
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
    for tri in &water_tris {
        for a in 0..3 {
            for b in 0..3 {
                if a != b {
                    next[tri[a]].push(tri[b]);
                }
            }
            if out[tri[a]].is_none() {
                out[tri[a]] = land.get(&key(tri[a])).map(|s| Bed {
                    colour: [s[0] / s[3], s[1] / s[3], s[2] / s[3]],
                    distance: 0.0,
                });
            }
        }
    }
    // Dijkstra from every shoreline vertex (distances in whole fine units,
    // ties by vertex, so the walk is deterministic).
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(u32, usize)>> = (0..n)
        .filter(|&v| out[v].is_some())
        .map(|v| std::cmp::Reverse((0, v)))
        .collect();
    while let Some(std::cmp::Reverse((d, v))) = heap.pop() {
        let Some(here) = out[v] else { continue };
        if (here.distance as u32) < d {
            continue;
        }
        let p = xz(v);
        for &w in &next[v] {
            let q = xz(w);
            let step = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2))
                .sqrt()
                .round() as u32;
            let nd = d + step;
            if out[w].is_none_or(|b| nd < b.distance as u32) {
                out[w] = Some(Bed {
                    colour: here.colour,
                    distance: nd as f32,
                });
                heap.push(std::cmp::Reverse((nd, w)));
            }
        }
    }
    out
}

/// One floor vertex's bed ([`beds`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bed {
    /// The nearest bank's colour (linear).
    pub colour: [f32; 3],
    /// The distance to that bank's shoreline (fine units).
    pub distance: f32,
}

/// A level's water mesh (the modern vertex layout): every
/// selected water triangle with three vertices of its own, each carrying
/// the flows of all three corners (the three patch-flow slots) and which
/// corner it is. The fragment blends the three slots' normals by the
/// interpolated one-hot corner mask, so the flow-oriented UVs are affine within each
/// triangle and the blend is continuous across shared edges; per-vertex
/// flows interpolated into one flow-oriented UV (M7) turn the UV frame
/// inside the triangle and seam at the edges.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaterMesh {
    /// The floor vertex of each mesh vertex.
    pub sources: Vec<u32>,
    /// Per mesh vertex: depth, slot 0 flow (x, z), corner; slot 1 and 2
    /// flows; the bed colour and shore distance ([`beds`]; red `-1`:
    /// none).
    pub attrs: Vec<[f32; 12]>,
    /// `(key, first, count)` per batch, in input order.
    pub ranges: Vec<(usize, u32, u32)>,
}

/// Build the water mesh of `batches` (`(key, triangle list of floor
/// vertices)`) over the floor's per-vertex attributes
/// ([`vertex_attributes`]).
#[must_use]
pub fn water_mesh(
    batches: &[(usize, Vec<u16>)],
    attrs: &[[f32; 4]],
    beds: &[Option<Bed>],
) -> WaterMesh {
    let mut mesh = WaterMesh::default();
    let at = |v: u16| {
        attrs
            .get(usize::from(v))
            .copied()
            .unwrap_or([DEFAULT_DEPTH, 0.0, 0.0, 0.0])
    };
    for (key, indices) in batches {
        let first = mesh.sources.len() as u32;
        for tri in indices.chunks_exact(3) {
            let [a, b, c] = [at(tri[0]), at(tri[1]), at(tri[2])];
            for (corner, &v) in tri.iter().enumerate() {
                mesh.sources.push(u32::from(v));
                // No bed: a negative red (the shader's own) and no shore.
                let bed = beds.get(usize::from(v)).copied().flatten().unwrap_or(Bed {
                    colour: [-1.0, 0.0, 0.0],
                    distance: f32::MAX,
                });
                mesh.attrs.push([
                    at(v)[0],
                    a[1],
                    a[2],
                    corner as f32,
                    b[1],
                    b[2],
                    c[1],
                    c[2],
                    bed.colour[0],
                    bed.colour[1],
                    bed.colour[2],
                    bed.distance.min(1.0e6),
                ]);
            }
        }
        mesh.ranges
            .push((*key, first, mesh.sources.len() as u32 - first));
    }
    mesh
}

/// How far the map agrees with a floor's water: the classic water tiles (owned
/// by a water batch) that are modern water tiles, of all classic water tiles.
#[must_use]
pub fn agreement(g: &FloorGeometry, water_batches: &[usize], map: &WaterMap) -> (usize, usize) {
    let (mut hits, mut total) = (0, 0);
    for z in 0..g.tiles_z {
        for x in 0..g.tiles_x {
            let owned = water_batches.iter().any(|&b| {
                g.batches[b]
                    .tri_mask
                    .get(g.tiles_x * z + x)
                    .is_some_and(|&m| m != 0)
            });
            if owned {
                total += 1;
                hits += usize::from(map.is_water_tile(x as i32, z as i32));
            }
        }
    }
    (hits, total)
}

/// Mirror about the horizontal plane `y = h` (camera-local classic units).
#[must_use]
pub fn mirror(h: f32) -> glam::Mat4 {
    glam::Mat4::from_cols(
        glam::Vec4::X,
        -glam::Vec4::Y,
        glam::Vec4::Z,
        glam::Vec4::new(0.0, 2.0 * h, 0.0, 1.0),
    )
}

/// `projection` with its near plane replaced by the view-space `plane`
/// (Lengyel's oblique near plane for a `[0, 1]` depth range): points with
/// `plane . p > 0` keep a depth in `[0, 1]`, points behind it clip. `plane`
/// must have the eye on its negative side.
#[must_use]
pub fn oblique(projection: glam::Mat4, plane: glam::Vec4) -> glam::Mat4 {
    let inv = projection.inverse();
    // The far corner on the plane's positive side, chosen by the plane's
    // signs in clip space (the classic projection flips y).
    let in_clip = inv.transpose() * plane;
    let q = inv * glam::Vec4::new(in_clip.x.signum(), in_clip.y.signum(), 1.0, 1.0);
    let c = plane * (1.0 / plane.dot(q));
    let mut rows = projection.transpose();
    rows.z_axis = c;
    rows.transpose()
}

/// The reflection camera of a frame: the view about `y = h` (camera-local)
/// and its oblique projection keeping what lies above the plane (classic y
/// down: `y < h`), `bias` units of slack. `view` maps camera-local to view
/// space, `view_proj` camera-local to clip (wgpu depth).
#[must_use]
pub fn reflection_view_proj(
    view: glam::Mat4,
    view_proj: glam::Mat4,
    h: f32,
    bias: f32,
) -> (glam::Mat4, glam::Mat4) {
    let projection = view_proj * view.inverse();
    let reflected_view = view * mirror(h);
    // Above the water: -y + h > 0, in the reflected view's space.
    let plane_world = glam::Vec4::new(0.0, -1.0, 0.0, h + bias);
    let plane_view = reflected_view.inverse().transpose() * plane_world;
    let p = oblique(projection, plane_view);
    (reflected_view, p * reflected_view)
}

/// The reflection plane of a frame: the surface height (camera-local y) of
/// the water vertex nearest the camera target in X/Z, from `(x, y, z)`
/// camera-local water vertices (the modern client reflects about the water
/// near the camera).
#[must_use]
pub fn reflection_height(vertices: impl IntoIterator<Item = [f32; 3]>) -> Option<f32> {
    let mut best: Option<(f32, f32)> = None;
    for [x, y, z] in vertices {
        let d = x * x + z * z;
        if best.is_none_or(|(b, _)| d < b) {
            best = Some((d, y));
        }
    }
    best.map(|(_, y)| y)
}

#[cfg(test)]
mod tests;

pub mod caustics;
pub mod effects;
