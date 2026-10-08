//! Floor (terrain) model build on the CPU side. Output is the exact vertex
//! streams a renderer uploads; no GPU here.
//!
//! # Units
//! Everything is in client units: 512 fine units per tile, heights are the
//! LAND ints (negative = up), colours are HSL16 shorts until the LUT step.
//! Nothing is rescaled or re-signed. The renderer applies its own view
//! transform at upload time.
//!
//! # What is here
//! - [`FloorHeights`]: the height grid `(tilesX+1) x (tilesZ+1)` with its
//!   bilinear fine-height sample.
//! - [`FloorBuilder`]: the floor builder, from construction (normals and
//!   min/max) through the level shade map, the per-tile vertex capture and
//!   batch lookup, to finalisation and the vertex writer.
//! - [`FloorBatch`]: one `(material, scale, water fog)` draw batch with its
//!   own colour stream, triangle ownership mask and index emission.
//! - [`SunLighting`]: the sun state the CPU lambert bake reads when the floor
//!   carries no normals (`(flagsB & 0x7) == 0`).
//!
//! # Deliberate deviations (all named)
//! 1. The client's hash tables are replaced by plain maps. Batch lookup keys
//!    on the same 64-bit `(offset, scale, colour, materialScale, material)`
//!    word and equality on `(material, scale, fog)`; batch order before the
//!    final node-id sort reproduces the 128-bucket walk (bucket = key & 127,
//!    insertion order).
//! 2. The vertex-dedupe table is a `HashMap` with first-insert-wins, which is
//!    what the client's table returns (bucket walk from the head; `put`
//!    appends at the tail).
//! 3. A vertex created with `rgb == -1` has no owner batch in the client and
//!    a later dedupe hit on it with `rgb != -1` would fail there. Here the
//!    owner is simply set instead.
//! 4. Colour bytes are stored as `R | G << 8 | B << 16 | A << 24` (the GL
//!    byte order). The DX-only R/B swap is not reproduced.
//! 5. Missing materials are a hard error at batch creation instead of a
//!    null dereference at finalise.

use std::collections::HashMap;

use crate::colour;
use crate::hardshadow::FloorHardShadows;
use crate::texture::MaterialStore;

/// Per-batch water fog parameters, compared structurally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaterFogData {
    /// 24-bit RGB.
    pub colour: i32,
    pub scale: i32,
    pub offset: i32,
    /// Never set by the map loader; part of the equality and of the dump.
    pub reserved: i32,
    pub extra_a: i32,
    pub extra_b: i32,
    pub extra_c: i32,
}

/// The sun state consumed by the floor bake: normalised direction, ambient,
/// `diffuse * 0.5` and `shadow * 0.5`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunLighting {
    /// Normalised sun direction (wire space, +Y down).
    pub dir: [f32; 3],
    /// `setSunAmbientIntensity` value.
    pub ambient: f32,
    /// `diffuseIntensity * 0.5`.
    pub diffuse_half: f32,
    /// `shadowIntensity * 0.5`.
    pub shadow_half: f32,
    /// `(int)(dx * 256.0F / dy)`, the
    /// hard-shadow x displacement per 256 units of height.
    pub shadow_offset_x: i32,
    /// `(int)(dz * 256.0F / dy)`.
    pub shadow_offset_z: i32,
}

impl SunLighting {
    /// The sun set with a diffuse and shadow intensity, a direction and an
    /// ambient intensity, in single-precision float (the direction is
    /// normalised through a `double` sqrt narrowed to float).
    #[must_use]
    pub fn from_set_sun(
        ambient: f32,
        diffuse: f32,
        shadow: f32,
        dx: f32,
        dy: f32,
        dz: f32,
    ) -> Self {
        let inv = (1.0_f64 / f64::from(dz * dz + dx * dx + dy * dy).sqrt()) as f32;
        Self {
            dir: [dx * inv, dy * inv, dz * inv],
            ambient,
            diffuse_half: diffuse * 0.5,
            shadow_half: shadow * 0.5,
            shadow_offset_x: (dx * 256.0_f32 / dy) as i32,
            shadow_offset_z: (dz * 256.0_f32 / dy) as i32,
        }
    }

    /// The default environment's sun: ambient
    /// `(brightness * 0.1 + 0.7 + antiMacro) * 1.1523438`, sun
    /// `(0xFFFFFF, 0.69921875, 1.2, -50 << 2, -60 << 2, -50 << 2)`.
    /// `anti_macro` is the per-rebuild random jitter (`-0.05 + random / 10`);
    /// pass `0.0` for deterministic builds.
    #[must_use]
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the default environment; exercised by tests only")
    )]
    pub fn environment_default(brightness_pref: i32, anti_macro: f32) -> Self {
        let ambient = (brightness_pref as f32 * 0.1 + 0.7 + anti_macro) * 1.152_343_8;
        Self::from_set_sun(ambient, 0.699_218_75, 1.2, -200.0, -240.0, -200.0)
    }
}

// ---------------------------------------------------------------------------
// FloorHeights: the height grid
// ---------------------------------------------------------------------------

/// Tile grid dimensions, tile size and the vertex
/// height grid.
#[derive(Clone, Debug)]
pub struct FloorHeights {
    /// Tiles along X.
    pub tiles_x: usize,
    /// Tiles along Z.
    pub tiles_z: usize,
    /// Fine units per tile (512).
    pub tile_size: i32,
    /// `log2(tile_size)` (9).
    pub shift: i32,
    /// The height grid, `(tiles_x + 1) * (tiles_z + 1)`, row-major by x.
    heights: Vec<i32>,
}

impl FloorHeights {
    /// A height grid over `tiles_x` x `tiles_z` tiles of `tile_size`.
    /// `heights` is the `[tilesX + 1][tilesZ + 1]` grid flattened by x.
    #[must_use]
    pub fn new(tiles_x: usize, tiles_z: usize, mut tile_size: i32, heights: Vec<i32>) -> Self {
        assert_eq!(
            heights.len(),
            (tiles_x + 1) * (tiles_z + 1),
            "height grid size"
        );
        let mut shift = 0;
        while tile_size > 1 {
            shift += 1;
            tile_size >>= 1;
        }
        Self {
            tiles_x,
            tiles_z,
            tile_size: 1 << shift,
            shift,
            heights,
        }
    }

    #[inline]
    fn h(&self, x: usize, z: usize) -> i32 {
        self.heights[x * (self.tiles_z + 1) + z]
    }

    /// `getFineHeight(fineX, fineZ)`: bilinear
    /// height in fine units; 0 outside the tile grid.
    #[must_use]
    pub fn get_fine_height(&self, fx: i32, fz: i32) -> i32 {
        let tx = fx >> self.shift;
        let tz = fz >> self.shift;
        if tx < 0 || tz < 0 || tx > self.tiles_x as i32 - 1 || tz > self.tiles_z as i32 - 1 {
            return 0;
        }
        let (tx, tz) = (tx as usize, tz as usize);
        let sx = fx & (self.tile_size - 1);
        let sz = fz & (self.tile_size - 1);
        let a = ((self.tile_size - sx) * self.h(tx, tz) + self.h(tx + 1, tz) * sx) >> self.shift;
        let b = ((self.tile_size - sx) * self.h(tx, tz + 1) + self.h(tx + 1, tz + 1) * sx)
            >> self.shift;
        ((self.tile_size - sz) * a + sz * b) >> self.shift
    }

    /// [`Self::get_fine_height`]
    /// with the tile coordinates clamped into the grid instead of returning 0.
    #[must_use]
    pub fn get_fine_height_clamped(&self, fx: i32, fz: i32) -> i32 {
        let tx = fx >> self.shift;
        let tz = fz >> self.shift;
        let x0 = (tx.max(0) as usize).min(self.tiles_x - 1);
        let z0 = (tz.max(0) as usize).min(self.tiles_z - 1);
        let x1 = (x0 + 1).min(self.tiles_x - 1);
        let z1 = (z0 + 1).min(self.tiles_z - 1);
        let sx = fx & (self.tile_size - 1);
        let sz = fz & (self.tile_size - 1);
        let a = ((self.tile_size - sx) * self.h(x0, z0) + self.h(x1, z0) * sx) >> self.shift;
        let b = ((self.tile_size - sx) * self.h(x0, z1) + self.h(x1, z1) * sx) >> self.shift;
        ((self.tile_size - sz) * a + sz * b) >> self.shift
    }

    /// `getTileHeight(x, z)`: vertex height.
    #[must_use]
    pub fn get_tile_height(&self, x: usize, z: usize) -> i32 {
        self.h(x, z)
    }

    /// [`Self::get_tile_height`] for a vertex that may lie off the grid: the
    /// vertex grid is `0..=tiles` on each axis, and `None` is outside it.
    #[must_use]
    pub fn tile_height_checked(&self, x: i32, z: i32) -> Option<i32> {
        let x = usize::try_from(x).ok().filter(|&x| x <= self.tiles_x)?;
        let z = usize::try_from(z).ok().filter(|&z| z <= self.tiles_z)?;
        Some(self.h(x, z))
    }

    /// Vertex height with the
    /// coordinates clamped to `0..tiles - 1`.
    #[must_use]
    pub fn get_tile_height_clamped(&self, x: i32, z: i32) -> i32 {
        let cx = (x.max(0) as usize).min(self.tiles_x - 1);
        let cz = (z.max(0) as usize).min(self.tiles_z - 1);
        self.h(cx, cz)
    }
}

// ---------------------------------------------------------------------------
// One material batch
// ---------------------------------------------------------------------------

/// The material terms the batch colour write reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MaterialTint {
    /// The material's grey blend (`& 0xFF`).
    grey_blend: i32,
    /// The material's brightness boost (`& 0xFF`).
    brightness_boost: i32,
    /// The material's effect id.
    effect: i32,
}

/// One `(material, material scale, water fog)` batch.
#[derive(Clone, Debug)]
pub struct FloorBatch {
    /// Material id, `-1` for the white texture.
    pub material: i32,
    /// Material scale (UV divisor at draw time).
    pub scale: f32,
    pub water_fog: WaterFogData,
    /// The 64-bit batch key.
    pub node_id: i64,
    /// Per-vertex colour bytes as `R | G << 8 | B << 16 | A << 24`
    /// (GL byte order). Alpha is `0` until the vertex is marked as owned by
    /// this batch (`0xFF`).
    pub colours: Vec<i32>,
    /// Bit `t` set when this batch owns
    /// triangle `t` of that tile.
    pub tri_mask: Vec<u32>,
    /// Ownership increments (only its `> 0` matters).
    pub tri_count: i32,
    tint: Option<MaterialTint>,
}

impl FloorBatch {
    /// Mark `vertex` as drawn by this batch (alpha 0xFF).
    fn mark_vertex(&mut self, vertex: usize) {
        self.colours[vertex] |= 0xFF00_0000_u32 as i32;
    }

    /// Claims triangle `tri` of tile `(x, z)`.
    fn own_triangle(&mut self, tiles_x: usize, x: usize, z: usize, tri: u32) {
        self.tri_mask[tiles_x * z + x] |= 1_u32.wrapping_shl(tri);
        self.tri_count += 1;
    }

    /// Writes the colour of `vertex`. `rgb` is the LUT colour packed `R | G << 8 | B << 16`;
    /// the channel math below is symmetric in R/B so the packing is irrelevant
    /// until the final byte write.
    fn write_colour(&mut self, vertex: usize, mut rgb: i32, shade: i32, lambert: f32) {
        if let Some(tint) = self.tint {
            let grey = tint.grey_blend;
            if grey != 0 && tint.effect != 4 {
                let target = if shade < 0 {
                    0
                } else if shade > 127 {
                    16_777_215
                } else {
                    shade * 131_586
                };
                if grey == 256 {
                    rgb = target;
                } else {
                    let inv = 256 - grey;
                    let low = (rgb & 0xFF00FF)
                        .wrapping_mul(inv)
                        .wrapping_add((target & 0xFF00FF).wrapping_mul(grey))
                        & 0xFF00_FF00_u32 as i32;
                    let mid = (rgb & 0xFF00)
                        .wrapping_mul(inv)
                        .wrapping_add((target & 0xFF00).wrapping_mul(grey))
                        & 0xFF0000;
                    rgb = low.wrapping_add(mid) >> 8;
                }
            }
            let boost = tint.brightness_boost;
            if boost != 0 {
                let boost = boost + 256;
                let r = (((rgb >> 16) & 0xFF) * boost).min(65535);
                let g = (((rgb >> 8) & 0xFF) * boost).min(65535);
                let b = ((rgb & 0xFF) * boost).min(65535);
                rgb = (b >> 8) + ((r & 0xFF00) << 8) + (g & 0xFF00);
            }
        }
        if lambert != 1.0 {
            let r = clamp_channel(((rgb >> 16) & 0xFF) as f32 * lambert);
            let g = clamp_channel(((rgb >> 8) & 0xFF) as f32 * lambert);
            let b = clamp_channel((rgb & 0xFF) as f32 * lambert);
            rgb = r << 16 | g << 8 | b;
        }
        // Byte 0 = rgb & 0xFF, byte 1 = rgb >> 8, byte 2 = rgb >> 16.
        let alpha = self.colours[vertex] & (0xFF00_0000_u32 as i32);
        self.colours[vertex] = alpha | (rgb & 0xFF_FFFF);
    }

    /// emit this batch's owned triangles of the visible tiles as a 16-bit
    /// index list. Returns `(indices, minVertex, maxVertex)`; the index count
    /// is `indices.len()`. Tiles are
    /// `tilesX * z + x` ids, in the order the caller lists them.
    #[must_use]
    pub fn build_indices(
        &self,
        geometry: &FloorGeometry,
        visible_tiles: &[usize],
    ) -> (Vec<u16>, i32, i32) {
        let mut out = Vec::new();
        let mut min = 32767_i32;
        let mut max = -32768_i32;
        for &tile in visible_tiles {
            let mask = self.tri_mask[tile];
            let Some(tris) = geometry.tile_tris[tile].as_ref() else {
                continue;
            };
            if mask == 0 {
                continue;
            }
            let mut bit = 0_u32;
            let mut i = 0;
            while i < tris.len() {
                let owned = mask & 1_u32.wrapping_shl(bit) != 0;
                bit += 1;
                if !owned {
                    i += 3;
                    continue;
                }
                for _ in 0..3 {
                    let v = i32::from(tris[i]);
                    i += 1;
                    if v > max {
                        max = v;
                    }
                    if v < min {
                        min = v;
                    }
                    out.push(v as u16);
                }
            }
        }
        (out, min, max)
    }
}

fn clamp_channel(v: f32) -> i32 {
    let v = v as i32;
    v.clamp(0, 255)
}

// ---------------------------------------------------------------------------
// Floor build state
// ---------------------------------------------------------------------------

/// The per-tile capture of the vertices of a tile.
#[derive(Clone, Debug)]
struct TileVerts {
    /// Vertex x within the tile (0..=512).
    xs: Vec<i32>,
    /// Vertex z within the tile.
    zs: Vec<i32>,
    /// HSL16 colour per vertex (`-1` = none).
    rgb: Vec<i32>,
    /// Alternate (average) colour per vertex, or `None`
    /// when the map loader passed no vertex-colour array (the colour is used
    /// instead).
    alt: Option<Vec<i32>>,
    /// Water depth per vertex.
    depth: Option<Vec<i32>>,
    /// Vertical offset per vertex.
    offset: Option<Vec<i32>>,
    /// Batch index per vertex.
    batches: Vec<usize>,
}

/// The renderer-independent construction calls of one floor — the create
/// arguments, the level shade map and every tile add in call order — retained
/// so another renderer can build its own floor from the same inputs (the
/// modern renderer reads them).
#[derive(Clone, Debug, Default)]
pub struct FloorCalls {
    #[allow(dead_code, reason = "recorded call argument; no reader yet")]
    pub flags_a: i32,
    pub flags_b: i32,
    pub tiles_x: usize,
    pub tiles_z: usize,
    /// `[tilesX + 1][tilesZ + 1]` heights, flattened by x.
    pub heights: Vec<i32>,
    /// The normal-source grid.
    pub normal_source: Vec<i32>,
    pub tile_size: i32,
    pub calls: Vec<FloorCall>,
    /// The sun when the floor was created: the default environment with the
    /// new map's sun direction, pushed before the floors are built.
    pub sun: Option<SunLighting>,
}

/// One recorded floor-construction call.
#[derive(Clone, Debug)]
pub enum FloorCall {
    /// `setLevelShadeMap(x, z, value)`.
    ShadeMap(i32, i32, i32),
    /// `addTileUnblended(x, z, pointX, pointOffset, pointZ, pointDepth, triA,
    /// triB, triC, triRgb, triAlt, triMaterial, triScale, waterFog,
    /// hardShadow)`.
    Unblended {
        x: i32,
        z: i32,
        point_x: Vec<i32>,
        point_offset: Option<Vec<i32>>,
        point_z: Vec<i32>,
        point_depth: Option<Vec<i32>>,
        tri: [Vec<i32>; 3],
        tri_rgb: Vec<i32>,
        tri_alt: Option<Vec<i32>>,
        tri_material: Vec<i32>,
        tri_scale: Vec<i32>,
        water_fog: WaterFogData,
        hard_shadow: bool,
    },
    /// A blended tile: the flat per-vertex form.
    Blended {
        x: i32,
        z: i32,
        xs: Vec<i32>,
        offsets: Option<Vec<i32>>,
        zs: Vec<i32>,
        depths: Option<Vec<i32>>,
        rgb: Vec<i32>,
        alt: Option<Vec<i32>>,
        materials: Vec<i32>,
        scales: Vec<i32>,
        water_fog: WaterFogData,
        hard_shadow: bool,
    },
}

/// The state the tile writer carries across tiles: the batch scratch list, the
/// vertex dedupe table, the owner batch of every vertex and the lighting.
struct TileWriter<'a> {
    scratch: Vec<usize>,
    dedupe: HashMap<i64, u16>,
    owner: Vec<Option<usize>>,
    sun: &'a SunLighting,
    tables: &'a colour::HslTables,
}

/// One tile as a flat triangle list: 3 entries per triangle, every vector the
/// same length (the offsets and depths are optional).
#[derive(Clone, Debug)]
pub struct FlatTile {
    /// Vertex x and z within the tile.
    pub xs: Vec<i32>,
    pub zs: Vec<i32>,
    /// Vertical offset per vertex, when the tile has them.
    pub offsets: Option<Vec<i32>>,
    /// Water depth per vertex, when the tile has water.
    pub depths: Option<Vec<i32>>,
    /// HSL16 colour per vertex (`-1` = none) and the alternate (average)
    /// colour, when there is one.
    pub rgb: Vec<i32>,
    pub alt: Option<Vec<i32>>,
    /// Material id and material scale per vertex.
    pub material_ids: Vec<i32>,
    pub material_scales: Vec<i32>,
    pub water_fog: WaterFogData,
    /// The tile casts a hard shadow onto lower levels.
    pub hard_shadow: bool,
}

/// One tile as indexed triangles over shared points: each triangle has a
/// colour, material and scale; each point a position (and optional offset and
/// depth).
#[derive(Clone, Copy, Debug)]
pub struct IndexedTile<'a> {
    pub point_x: &'a [i32],
    pub point_offset: Option<&'a [i32]>,
    pub point_z: &'a [i32],
    pub point_depth: Option<&'a [i32]>,
    /// The three corner point indices of each triangle.
    pub tri_a: &'a [i32],
    pub tri_b: &'a [i32],
    pub tri_c: &'a [i32],
    pub tri_rgb: &'a [i32],
    pub tri_alt: Option<&'a [i32]>,
    pub tri_material: &'a [i32],
    pub tri_scale: &'a [i32],
    pub water_fog: WaterFogData,
    pub hard_shadow: bool,
}

/// The floor between construction and finalisation.
#[derive(Clone, Debug)]
pub struct FloorBuilder {
    /// The height grid.
    pub heights: FloorHeights,
    /// Flags A: `0x1` scenery shadows, `0x2` keep tile arrays.
    #[allow(dead_code, reason = "no reader yet")]
    pub flags_a: i32,
    /// Flags B: `0x7` normals, `0x8` water detail, `0x10` hard
    /// shadows, `0x20` textures off).
    pub flags_b: i32,
    /// `shift - 2` (quarter-tile lattice shift).
    lattice_shift: i32,
    /// `1 << lattice_shift`.
    lattice: i32,
    /// Min/max height, widened by 1 after the scan.
    min_y: f32,
    max_y: f32,
    /// Per-vertex normal, `(tiles_x+1)*(tiles_z+1)`.
    normals: Vec<[f32; 3]>,
    /// Shade map, `(tiles_x+1)*(tiles_z+1)`, bytes.
    shade: Vec<u8>,
    /// Tile captures, `tiles_x * tiles_z`, index `x * tiles_z + z`.
    tiles: Vec<Option<TileVerts>>,
    /// Per-tile flags (bit `0x1` = hard shadow), index `x * tiles_z + z`.
    hard_shadow: Vec<u8>,
    /// The hard-shadow mask upper levels stamp into, present
    /// only when `(flagsB & 0x10) != 0`.
    hard_shadows: Option<FloorHardShadows>,
    /// Batches in creation order.
    batches: Vec<FloorBatch>,
    /// Lookup: key -> batch indices with that key.
    batch_lookup: HashMap<i64, Vec<usize>>,
    /// Total captured vertices (pre-dedupe).
    total_verts: usize,
    /// Most vertices captured by one tile.
    max_tile_verts: usize,
    /// Some tile carried water depth.
    has_depth: bool,
    /// Built for `Scene.underwaterLevelHeightMaps`:
    /// drawn in `Scene.draw`'s underwater pass.
    pub underwater: bool,
    /// The construction calls, for toolkits that build their own floor.
    calls: FloorCalls,
}

impl FloorBuilder {
    /// Starts a floor build from the toolkit, the two flag sets, the tile counts,
    /// the heights, the normal source and the tile size. `heights`
    /// and `normal_source` are `[tilesX + 1][tilesZ + 1]` grids flattened by
    /// x (`normal_source` is the underwater grid for the seabed floor,
    /// otherwise the same array).
    #[must_use]
    pub fn new(
        flags_a: i32,
        flags_b: i32,
        tiles_x: usize,
        tiles_z: usize,
        heights: Vec<i32>,
        normal_source: &[i32],
        tile_size: i32,
    ) -> Self {
        let calls = FloorCalls {
            flags_a,
            flags_b,
            tiles_x,
            tiles_z,
            heights: heights.clone(),
            normal_source: normal_source.to_vec(),
            tile_size,
            calls: Vec::new(),
            sun: None,
        };
        let heights = FloorHeights::new(tiles_x, tiles_z, tile_size, heights);
        let stride = tiles_z + 1;
        assert_eq!(
            normal_source.len(),
            (tiles_x + 1) * stride,
            "normal source size"
        );
        let mut min_y = f32::MAX;
        let mut max_y = -3.402_823_5E38_f32;
        for z in 0..=tiles_z {
            for x in 0..=tiles_x {
                let h = heights.h(x, z);
                if (h as f32) < min_y {
                    min_y = h as f32;
                }
                if (h as f32) > max_y {
                    max_y = h as f32;
                }
            }
        }
        let normals = grid_normals(normal_source, tiles_x, tiles_z, heights.tile_size);
        min_y -= 1.0;
        max_y += 1.0;
        // The hard-shadow mask.
        let hard_shadows = if (flags_b & 0x10) != 0 {
            Some(FloorHardShadows::new(
                tiles_x,
                tiles_z,
                heights.tile_size,
                heights.shift,
            ))
        } else {
            None
        };
        Self {
            heights,
            flags_a,
            flags_b,
            lattice_shift: 0,
            lattice: 0,
            min_y,
            max_y,
            normals,
            shade: vec![0; (tiles_x + 1) * stride],
            tiles: vec![None; tiles_x * tiles_z],
            hard_shadow: vec![0; tiles_x * tiles_z],
            hard_shadows,
            batches: Vec::new(),
            batch_lookup: HashMap::new(),
            total_verts: 0,
            max_tile_verts: 0,
            has_depth: false,
            underwater: false,
            calls,
        }
        .with_lattice()
    }

    fn with_lattice(mut self) -> Self {
        self.lattice_shift = self.heights.shift - 2;
        self.lattice = 1 << self.lattice_shift;
        self
    }

    /// Does the tile cast a hard shadow onto lower levels?
    #[must_use]
    pub fn tile_casts_hard_shadow(&self, x: usize, z: usize) -> bool {
        (self.hard_shadow[x * self.heights.tiles_z + z] & 0x1) != 0
    }

    /// The tile's vertex
    /// x / z within the tile, 3 per triangle; `None` for an empty tile.
    #[must_use]
    pub fn tile_vertex_xz(&self, x: usize, z: usize) -> Option<(&[i32], &[i32])> {
        self.tiles[x * self.heights.tiles_z + z]
            .as_ref()
            .map(|t| (t.xs.as_slice(), t.zs.as_slice()))
    }

    /// The hard-shadow mask, if this floor has one.
    #[allow(dead_code)]
    #[must_use]
    pub fn hard_shadows(&self) -> Option<&FloorHardShadows> {
        self.hard_shadows.as_ref()
    }

    /// Mutable [`Self::hard_shadows`].
    pub fn hard_shadows_mut(&mut self) -> Option<&mut FloorHardShadows> {
        self.hard_shadows.as_mut()
    }

    /// `setLevelShadeMap(x, z, value)`: max-keep
    /// stamp into the vertex shade map, coordinates clamped into the grid.
    pub fn set_level_shade_map(&mut self, x: i32, z: i32, value: i32) {
        self.calls.calls.push(FloorCall::ShadeMap(x, z, value));
        let stride = self.heights.tiles_z + 1;
        let cx = (x.max(0) as usize).min(self.heights.tiles_x);
        let cz = (z.max(0) as usize).min(self.heights.tiles_z);
        let slot = &mut self.shade[cx * stride + cz];
        if i32::from(*slot) < value {
            *slot = value as u8;
        }
    }

    /// Captures one tile's flat triangle list (3 entries per triangle) and
    /// resolves each vertex's batch.
    pub fn add_tile(
        &mut self,
        materials: &MaterialStore,
        x: usize,
        z: usize,
        tile: FlatTile,
    ) -> anyhow::Result<()> {
        self.calls.calls.push(FloorCall::Blended {
            x: x as i32,
            z: z as i32,
            xs: tile.xs.clone(),
            offsets: tile.offsets.clone(),
            zs: tile.zs.clone(),
            depths: tile.depths.clone(),
            rgb: tile.rgb.clone(),
            alt: tile.alt.clone(),
            materials: tile.material_ids.clone(),
            scales: tile.material_scales.clone(),
            water_fog: tile.water_fog,
            hard_shadow: tile.hard_shadow,
        });
        self.add_tile_captured(materials, x, z, tile)
    }

    fn add_tile_captured(
        &mut self,
        materials: &MaterialStore,
        x: usize,
        z: usize,
        tile: FlatTile,
    ) -> anyhow::Result<()> {
        let FlatTile {
            xs,
            offsets,
            zs,
            depths,
            rgb,
            alt,
            material_ids,
            material_scales,
            water_fog,
            hard_shadow,
        } = tile;
        let n = rgb.len();
        assert!(
            xs.len() == n && zs.len() == n && material_ids.len() == n && material_scales.len() == n
        );
        if depths.is_some() {
            self.has_depth = true;
        }
        let mut batches = Vec::with_capacity(n);
        for i in 0..n {
            let mut material = material_ids[i];
            let mut scale = material_scales[i];
            if (self.flags_b & 0x20) != 0 && material != -1 {
                let m = materials.get(material as u32).ok_or_else(|| {
                    anyhow::anyhow!("floor tile {x},{z}: material {material} missing")
                })?;
                if m.high_detail {
                    scale = 128;
                    material = -1;
                }
            }
            let key = (i64::from(water_fog.offset) << 48)
                | (i64::from(water_fog.scale) << 42)
                | (i64::from(water_fog.colour) << 28)
                | i64::from(scale << 14)
                | i64::from(material);
            let found = self.batch_lookup.get(&key).and_then(|cands| {
                cands.iter().copied().find(|&b| {
                    let batch = &self.batches[b];
                    batch.material == material
                        && scale as f32 == batch.scale
                        && batch.water_fog == water_fog
                })
            });
            let idx = match found {
                Some(idx) => idx,
                None => {
                    let tint = if material == -1 {
                        None
                    } else {
                        let m = materials.get(material as u32).ok_or_else(|| {
                            anyhow::anyhow!("floor tile {x},{z}: material {material} missing")
                        })?;
                        Some(MaterialTint {
                            grey_blend: i32::from(m.grey_blend),
                            brightness_boost: i32::from(m.brightness_boost),
                            effect: i32::from(m.effect),
                        })
                    };
                    let idx = self.batches.len();
                    self.batches.push(FloorBatch {
                        material,
                        scale: scale as f32,
                        water_fog,
                        node_id: key,
                        colours: Vec::new(),
                        tri_mask: vec![0; self.heights.tiles_x * self.heights.tiles_z],
                        tri_count: 0,
                        tint,
                    });
                    self.batch_lookup.entry(key).or_default().push(idx);
                    idx
                }
            };
            batches.push(idx);
        }
        let slot = x * self.heights.tiles_z + z;
        if hard_shadow {
            self.hard_shadow[slot] |= 0x1;
        }
        if n > self.max_tile_verts {
            self.max_tile_verts = n;
        }
        self.total_verts += n;
        self.tiles[slot] = Some(TileVerts {
            xs,
            zs,
            rgb,
            alt,
            depth: depths,
            offset: offsets,
            batches,
        });
        Ok(())
    }

    /// Expands an indexed per-triangle tile into the flat form and captures
    /// it like [`Self::add_tile`].
    pub fn add_tile_unblended(
        &mut self,
        materials: &MaterialStore,
        x: usize,
        z: usize,
        tile: IndexedTile<'_>,
    ) -> anyhow::Result<()> {
        let IndexedTile {
            point_x,
            point_offset,
            point_z,
            point_depth,
            tri_a,
            tri_b,
            tri_c,
            tri_rgb,
            tri_alt,
            tri_material,
            tri_scale,
            water_fog,
            hard_shadow,
        } = tile;
        self.calls.calls.push(FloorCall::Unblended {
            x: x as i32,
            z: z as i32,
            point_x: point_x.to_vec(),
            point_offset: point_offset.map(<[i32]>::to_vec),
            point_z: point_z.to_vec(),
            point_depth: point_depth.map(<[i32]>::to_vec),
            tri: [tri_a.to_vec(), tri_b.to_vec(), tri_c.to_vec()],
            tri_rgb: tri_rgb.to_vec(),
            tri_alt: tri_alt.map(<[i32]>::to_vec),
            tri_material: tri_material.to_vec(),
            tri_scale: tri_scale.to_vec(),
            water_fog,
            hard_shadow,
        });
        let tris = tri_rgb.len();
        let mut xs = Vec::with_capacity(tris * 3);
        let mut zs = Vec::with_capacity(tris * 3);
        let mut rgb = Vec::with_capacity(tris * 3);
        let mut alt = Vec::with_capacity(tris * 3);
        let mut mats = Vec::with_capacity(tris * 3);
        let mut scales = Vec::with_capacity(tris * 3);
        let mut offsets = point_offset.map(|_| Vec::with_capacity(tris * 3));
        let mut depths = point_depth.map(|_| Vec::with_capacity(tris * 3));
        for t in 0..tris {
            for corner in [tri_a[t], tri_b[t], tri_c[t]] {
                let p = corner as usize;
                xs.push(point_x[p]);
                zs.push(point_z[p]);
                rgb.push(tri_rgb[t]);
                mats.push(tri_material[t]);
                scales.push(tri_scale[t]);
                alt.push(tri_alt.map_or(tri_rgb[t], |a| a[t]));
                if let (Some(out), Some(src)) = (offsets.as_mut(), point_offset) {
                    out.push(src[p]);
                }
                if let (Some(out), Some(src)) = (depths.as_mut(), point_depth) {
                    out.push(src[p]);
                }
            }
        }
        self.add_tile_captured(
            materials,
            x,
            z,
            FlatTile {
                xs,
                offsets,
                zs,
                depths,
                rgb,
                alt: Some(alt),
                material_ids: mats,
                material_scales: scales,
                water_fog,
                hard_shadow,
            },
        )
    }

    /// Finalises the floor and assembles its streams: blur the shade map, write every tile's vertices, elect one
    /// owner batch per triangle, compact + sort the batches.
    pub fn finalise(mut self, sun: &SunLighting) -> anyhow::Result<FloorGeometry> {
        let tiles_x = self.heights.tiles_x;
        let tiles_z = self.heights.tiles_z;
        let stride = tiles_z + 1;
        let has_normals = (self.flags_b & 0x7) != 0;
        let water_detail = (self.flags_b & 0x8) != 0;
        let underwater = self.underwater;
        let mut stride_floats = 5;
        if self.has_depth {
            stride_floats += 1;
        }
        if has_normals {
            stride_floats += 3;
        }
        if self.total_verts == 0 {
            return Ok(FloorGeometry {
                tiles_x,
                tiles_z,
                has_depth: self.has_depth,
                has_normals,
                water_detail,
                underwater,
                stride_floats,
                vertex_count: 0,
                index_count: 0,
                stream0: Vec::new(),
                base_colours: Vec::new(),
                tile_tris: vec![None; tiles_x * tiles_z],
                batches: Vec::new(),
                hard_shadow: self.hard_shadow,
                // An empty floor releases the shadow mask before any dynamic
                // scenery stamps it.
                hard_shadows: None,
                min_y: self.min_y,
                max_y: self.max_y,
                heights: self.heights,
                calls: Some(std::sync::Arc::new(self.calls)),
            });
        }

        // Blurred shade map (signed-byte arithmetic).
        let mut blurred = vec![0_i8; (tiles_x + 1) * stride];
        for x in 1..tiles_x {
            for z in 1..tiles_z {
                let s = |xx: usize, zz: usize| i32::from(self.shade[xx * stride + zz] as i8);
                blurred[x * stride + z] = ((s(x, z) >> 1)
                    + (s(x, z + 1) >> 3)
                    + (s(x, z - 1) >> 2)
                    + (s(x - 1, z) >> 2)
                    + (s(x + 1, z) >> 3)) as i8;
            }
        }

        // Batch colour buffers (alpha 0).
        for batch in &mut self.batches {
            batch.colours = vec![0; self.total_verts];
        }

        let mut geometry = FloorGeometry {
            tiles_x,
            tiles_z,
            has_depth: self.has_depth,
            has_normals,
            water_detail,
            underwater,
            stride_floats,
            vertex_count: 0,
            index_count: 0,
            stream0: Vec::with_capacity(stride_floats * self.total_verts),
            base_colours: Vec::with_capacity(self.total_verts),
            tile_tris: vec![None; tiles_x * tiles_z],
            batches: Vec::new(),
            hard_shadow: Vec::new(),
            hard_shadows: None,
            min_y: self.min_y,
            max_y: self.max_y,
            heights: FloorHeights::new(1, 1, 512, vec![0; 4]),
            calls: None,
        };
        let mut writer = TileWriter {
            scratch: Vec::with_capacity(self.max_tile_verts),
            dedupe: HashMap::new(),
            owner: vec![None; self.total_verts],
            sun,
            tables: colour::hsl_tables(),
        };

        // X outer, z inner.
        for x in 0..tiles_x {
            for z in 0..tiles_z {
                self.write_tile(x, z, &blurred, &mut writer, &mut geometry)?;
            }
        }
        let owner = writer.owner;

        // Owner batches mark their vertices drawn.
        for (v, o) in owner.iter().enumerate().take(geometry.vertex_count) {
            if let Some(b) = o {
                self.batches[*b].mark_vertex(v);
            }
        }

        // Per triangle, elect the lowest-nodeId owner (x outer).
        for x in 0..tiles_x {
            for z in 0..tiles_z {
                let Some(tris) = geometry.tile_tris[tiles_x * z + x].as_ref() else {
                    continue;
                };
                let mut tri = 0_u32;
                let mut i = 0;
                while i < tris.len() {
                    let v0 = usize::from(tris[i]);
                    let v1 = usize::from(tris[i + 1]);
                    let v2 = usize::from(tris[i + 2]);
                    i += 3;
                    let b0 = owner[v0];
                    let b1 = owner[v1];
                    let b2 = owner[v2];
                    let mut elected: Option<usize> = None;
                    if let Some(b) = b0 {
                        self.batches[b].own_triangle(tiles_x, x, z, tri);
                        elected = Some(b);
                    }
                    if let Some(b) = b1 {
                        self.batches[b].own_triangle(tiles_x, x, z, tri);
                        if elected.is_none_or(|e| self.batches[b].node_id < self.batches[e].node_id)
                        {
                            elected = Some(b);
                        }
                    }
                    if let Some(b) = b2 {
                        self.batches[b].own_triangle(tiles_x, x, z, tri);
                        if elected.is_none_or(|e| self.batches[b].node_id < self.batches[e].node_id)
                        {
                            elected = Some(b);
                        }
                    }
                    if let Some(e) = elected {
                        if b0.is_some() {
                            self.batches[e].mark_vertex(v0);
                        }
                        if b1.is_some() {
                            self.batches[e].mark_vertex(v1);
                        }
                        if b2.is_some() {
                            self.batches[e].mark_vertex(v2);
                        }
                        self.batches[e].own_triangle(tiles_x, x, z, tri);
                    }
                    tri += 1;
                }
            }
        }

        // 319 order (HashTable.toArray: bucket = key & 127, insertion
        // order within a bucket), then :397-411 compaction + nodeId sort.
        let mut order: Vec<usize> = (0..self.batches.len()).collect();
        order.sort_by_key(|&i| (self.batches[i].node_id & 127) as u32);
        let kept: Vec<usize> = order
            .into_iter()
            .filter(|&i| self.batches[i].tri_count > 0)
            .collect();
        let mut keys: Vec<i64> = kept.iter().map(|&i| self.batches[i].node_id).collect();
        let mut items = kept;
        jittered_sort(&mut keys, &mut items);
        let vertex_count = geometry.vertex_count;
        let mut batches = Vec::with_capacity(items.len());
        let mut taken: Vec<Option<FloorBatch>> = self.batches.drain(..).map(Some).collect();
        for i in items {
            let mut b = taken[i].take().expect("batch taken twice");
            b.colours.truncate(vertex_count);
            batches.push(b);
        }
        geometry.batches = batches;
        geometry.hard_shadow = self.hard_shadow;
        geometry.hard_shadows = self.hard_shadows;
        geometry.heights = self.heights;
        geometry.calls = Some(std::sync::Arc::new(std::mem::take(&mut self.calls)));
        Ok(geometry)
    }

    /// Writes the vertices of tile `(x, z)`: normals, colours, the dedupe
    /// table and the owner batch of every vertex.
    fn write_tile(
        &mut self,
        x: usize,
        z: usize,
        blurred: &[i8],
        writer: &mut TileWriter<'_>,
        geometry: &mut FloorGeometry,
    ) -> anyhow::Result<()> {
        let scratch = &mut writer.scratch;
        let dedupe = &mut writer.dedupe;
        let owner = &mut writer.owner;
        let sun = writer.sun;
        let tables = writer.tables;
        let tiles_x = self.heights.tiles_x;
        let tiles_z = self.heights.tiles_z;
        let stride = tiles_z + 1;
        let Some(tile) = self.tiles[x * tiles_z + z].take() else {
            return Ok(());
        };
        let shift = self.heights.shift;
        let ts = self.heights.tile_size;
        let n00 = self.normals[x * stride + z];
        let n01 = self.normals[x * stride + z + 1];
        let n11 = self.normals[(x + 1) * stride + z + 1];
        let n10 = self.normals[(x + 1) * stride + z];
        let s00 = i32::from(blurred[x * stride + z] as u8);
        let s01 = i32::from(blurred[x * stride + z + 1] as u8);
        let s11 = i32::from(blurred[(x + 1) * stride + z + 1] as u8);
        let s10 = i32::from(blurred[(x + 1) * stride + z] as u8);
        let alt = tile.alt.as_deref().unwrap_or(&tile.rgb);

        // Distinct batches of this tile, first-seen order.
        scratch.clear();
        for &b in &tile.batches {
            if !scratch.contains(&b) {
                scratch.push(b);
            }
        }

        let n = tile.rgb.len();
        let mut tris = vec![0_u16; n];
        let bake_lambert = (self.flags_b & 0x7) == 0;
        for i in 0..n {
            let wx = ((x as i32) << shift) + tile.xs[i];
            let wz = ((z as i32) << shift) + tile.zs[i];
            let qx = wx >> self.lattice_shift;
            let qz = wz >> self.lattice_shift;
            let rgb = tile.rgb[i];
            let alt_c = alt[i];
            let off = tile.offset.as_ref().map_or(0, |o| o[i]);
            let key = (i64::from(alt_c) << 48)
                | (i64::from(rgb) << 32)
                | i64::from(qx << 16)
                | i64::from(qz);
            let lx = tile.xs[i];
            let lz = tile.zs[i];
            let normal = tile_point_normal([n00, n01, n11, n10], lx, lz, ts);
            let shade = if lx == 0 && lz == 0 {
                74 - s00
            } else if lx == 0 && ts == lz {
                74 - s01
            } else if ts == lx && ts == lz {
                74 - s11
            } else if ts == lx && lz == 0 {
                74 - s10
            } else {
                let sa = (((s10 - s00) * lx) >> shift) + s00;
                let sb = (((s11 - s01) * lx) >> shift) + s01;
                74 - ((((sb - sa) * lz) >> shift) + sa)
            };
            let mut lit = 0_i32;
            let mut lambert = 1.0_f32;
            if rgb != -1 {
                let lum = clamp_lum(((rgb & 0x7F) * shade) >> 7);
                lit = lut(tables, (rgb & 0xFF80) | lum)?;
                if bake_lambert {
                    let ndotl =
                        sun.dir[2] * normal[2] + sun.dir[0] * normal[0] + sun.dir[1] * normal[1];
                    lambert = sun.ambient
                        + ndotl
                            * (if ndotl > 0.0 {
                                sun.diffuse_half
                            } else {
                                sun.shadow_half
                            });
                }
            }
            let on_lattice = (wx & (self.lattice - 1)) == 0 && (wz & (self.lattice - 1)) == 0;
            let existing = if on_lattice {
                dedupe.get(&key).copied()
            } else {
                None
            };
            let vertex: usize;
            match existing {
                None => {
                    // New vertex.
                    let base = if rgb == alt_c {
                        lit
                    } else {
                        let lum = clamp_lum(((alt_c & 0x7F) * shade) >> 7);
                        let mut c = lut(tables, (alt_c & 0xFF80) | lum)?;
                        if bake_lambert {
                            // The client reuses the rgb lambert as the dot
                            // product here — reproduced verbatim.
                            let _ndotl = sun.dir[2] * normal[2]
                                + sun.dir[0] * normal[0]
                                + sun.dir[1] * normal[1];
                            let f = sun.ambient
                                + lambert
                                    * (if lambert > 0.0 {
                                        sun.diffuse_half
                                    } else {
                                        sun.shadow_half
                                    });
                            let r = clamp_channel(((c >> 16) & 0xFF) as f32 * f);
                            let g = clamp_channel(((c >> 8) & 0xFF) as f32 * f);
                            let b = clamp_channel((c & 0xFF) as f32 * f);
                            c = r << 16 | g << 8 | b;
                        }
                        c
                    };
                    geometry.stream0.push(wx as f32);
                    geometry
                        .stream0
                        .push((self.heights.get_fine_height(wx, wz) + off) as f32);
                    geometry.stream0.push(wz as f32);
                    geometry.stream0.push(wx as f32);
                    geometry.stream0.push(wz as f32);
                    if self.has_depth {
                        geometry
                            .stream0
                            .push(tile.depth.as_ref().map_or(0.0, |d| (d[i] - 1) as f32));
                    }
                    if !bake_lambert {
                        geometry.stream0.push(normal[0]);
                        geometry.stream0.push(normal[1]);
                        geometry.stream0.push(normal[2]);
                    }
                    geometry.base_colours.push(base | (0xFF00_0000_u32 as i32));
                    vertex = geometry.vertex_count;
                    geometry.vertex_count += 1;
                    // The client keeps an int vertex counter but stores the
                    // tile/dedupe index as a short, including overflow in large
                    // build areas. Do not reject or widen the index.
                    tris[i] = vertex as u16;
                    if rgb != -1 {
                        owner[vertex] = Some(tile.batches[i]);
                    }
                    dedupe.entry(key).or_insert(vertex as u16);
                }
                Some(v) => {
                    // Shared vertex; the lowest node id batch wins ownership.
                    tris[i] = v;
                    vertex = usize::from(v);
                    if rgb != -1 {
                        match owner[vertex] {
                            // The owner is dereferenced before the node ids are
                            // compared. A wrapped short may alias an unowned
                            // vertex; the client throws a null pointer error.
                            None => {
                                anyhow::bail!("floor batch owner missing at reused index {vertex}")
                            }
                            Some(o)
                                if self.batches[tile.batches[i]].node_id
                                    >= self.batches[o].node_id => {}
                            _ => owner[vertex] = Some(tile.batches[i]),
                        }
                    }
                }
            }
            // Every batch of this tile writes the vertex colour.
            for &b in scratch.iter() {
                self.batches[b].write_colour(vertex, lit, shade, lambert);
            }
            geometry.index_count += 1;
        }
        geometry.tile_tris[tiles_x * z + x] = Some(tris);
        Ok(())
    }
}

fn clamp_lum(lum: i32) -> i32 {
    lum.clamp(2, 126)
}

/// The HSL16 lookup table entry, with the bounds check turned into an error.
fn lut(tables: &colour::HslTables, index: i32) -> anyhow::Result<i32> {
    usize::try_from(index)
        .ok()
        .and_then(|i| tables.bgr.get(i).copied())
        .ok_or_else(|| anyhow::anyhow!("hsl16 colour index {index} out of range"))
}

/// The client's quicksort on parallel `i64` keys + item array, including the
/// `(i & 1)` jitter for pivots other than `i64::MAX`. Reproduced so equal keys
/// land in the same order as the client.
pub fn jittered_sort<T: Copy>(keys: &mut [i64], items: &mut [T]) {
    if keys.is_empty() {
        return;
    }
    jittered_sort_range(keys, items, 0, keys.len() as i32 - 1);
}

fn jittered_sort_range<T: Copy>(keys: &mut [i64], items: &mut [T], lo: i32, hi: i32) {
    if lo >= hi {
        return;
    }
    let mid = (lo + hi) / 2;
    let mut store = lo;
    let pivot = keys[mid as usize];
    keys.swap(mid as usize, hi as usize);
    items.swap(mid as usize, hi as usize);
    let jitter = if pivot == i64::MAX { 0 } else { 1 };
    for i in lo..hi {
        if keys[i as usize] < i64::from(i & jitter).wrapping_add(pivot) {
            keys.swap(i as usize, store as usize);
            items.swap(i as usize, store as usize);
            store += 1;
        }
    }
    keys.swap(hi as usize, store as usize);
    items.swap(hi as usize, store as usize);
    jittered_sort_range(keys, items, lo, store - 1);
    jittered_sort_range(keys, items, store + 1, hi);
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// The per-grid-vertex normals ([`FloorBuilder::new`]) from the normal-source grid
/// (`[tilesX + 1][tilesZ + 1]`, flattened by x) as central differences over
/// `tile_size` (the power-of-two `FloorHeights::tile_size`), normalised
/// through a `double` sqrt narrowed to float. The border vertices keep
/// `(0, 0, 0)`.
#[must_use]
pub fn grid_normals(
    normal_source: &[i32],
    tiles_x: usize,
    tiles_z: usize,
    tile_size: i32,
) -> Vec<[f32; 3]> {
    let stride = tiles_z + 1;
    let ts = tile_size;
    let mut normals = vec![[0.0_f32; 3]; (tiles_x + 1) * stride];
    for z in 0..=tiles_z {
        for x in 0..=tiles_x {
            if x > 0 && z > 0 && x < tiles_x && z < tiles_z {
                let dx = normal_source[(x + 1) * stride + z] - normal_source[(x - 1) * stride + z];
                let dz = normal_source[x * stride + z + 1] - normal_source[x * stride + z - 1];
                let len_sq = dz
                    .wrapping_mul(dz)
                    .wrapping_add(ts.wrapping_mul(4).wrapping_mul(ts))
                    .wrapping_add(dx.wrapping_mul(dx));
                let inv = (1.0_f64 / f64::from(len_sq).sqrt()) as f32;
                normals[x * stride + z] =
                    [dx as f32 * inv, (-ts * 2) as f32 * inv, dz as f32 * inv];
            }
        }
    }
    normals
}

/// The normal a tile point `(lx, lz)` (fine units inside the tile, `0..=ts`)
/// gets from the tile's corner normals `[n00, n01, n11, n10]`
/// (`FloorBuilder::write_tile`): a corner's own normal at the corners, else
/// the bilinear blend along x then z, in the client's float order.
#[must_use]
pub fn tile_point_normal(corners: [[f32; 3]; 4], lx: i32, lz: i32, ts: i32) -> [f32; 3] {
    let [n00, n01, n11, n10] = corners;
    if lx == 0 && lz == 0 {
        n00
    } else if lx == 0 && ts == lz {
        n01
    } else if ts == lx && ts == lz {
        n11
    } else if ts == lx && lz == 0 {
        n10
    } else {
        let fx = lx as f32 / ts as f32;
        let fz = lz as f32 / ts as f32;
        let a = [
            (n10[0] - n00[0]) * fx + n00[0],
            (n10[1] - n00[1]) * fx + n00[1],
            (n10[2] - n00[2]) * fx + n00[2],
        ];
        let b = [
            (n11[0] - n01[0]) * fx + n01[0],
            (n11[1] - n01[1]) * fx + n01[1],
            (n11[2] - n01[2]) * fx + n01[2],
        ];
        [
            (b[0] - a[0]) * fz + a[0],
            (b[1] - a[1]) * fz + a[1],
            (b[2] - a[2]) * fz + a[2],
        ]
    }
}

/// The finished floor: what a renderer uploads, plus the retained heights.
#[derive(Clone, Debug)]
pub struct FloorGeometry {
    /// Tiles along X.
    pub tiles_x: usize,
    /// Tiles along Z.
    pub tiles_z: usize,
    /// Stream 0 carries a water-depth float
    /// (`TEX_COORD_1`).
    pub has_depth: bool,
    /// Stream 0 carries normals (`flags_b & 0x7 != 0`); otherwise the
    /// lambert term was baked into the batch colours.
    pub has_normals: bool,
    /// Water detail (`flags_b & 0x8 != 0`):
    /// water batches take `EnvMappedWater`/`EnvMappedSea`
    pub water_detail: bool,
    /// Floor of the underwater scene.
    pub underwater: bool,
    /// Floats per vertex in [`Self::stream0`]: `pos(3) + uv(2) [+ depth(1)]
    /// [+ normal(3)]`.
    pub stride_floats: usize,
    /// Vertices written after dedupe.
    pub vertex_count: usize,
    /// Total triangle-corner references (3 per triangle).
    pub index_count: usize,
    /// Geometry stream, `stride_floats * vertex_count` floats.
    /// Positions/UVs are fine world units (`x = tileX * 512 + localX`).
    pub stream0: Vec<f32>,
    /// Base (unlit, alt-colour) stream as `R | G << 8 | B << 16
    /// | 0xFF << 24` per vertex, GLX byte order.
    pub base_colours: Vec<i32>,
    /// Per tile, the vertex indices of its
    /// triangles (3 per triangle) or `None` when the tile has no floor.
    pub tile_tris: Vec<Option<Vec<u16>>>,
    /// Draw batches with at least one owned triangle, sorted by
    /// `nodeId`.
    pub batches: Vec<FloorBatch>,
    /// Bit `0x1` = tile casts a hard shadow.
    pub hard_shadow: Vec<u8>,
    /// The hard-shadow mask stamped by the levels above
    /// (`Scene.sweepShadows`), present only with `flagsB & 0x10`.
    pub hard_shadows: Option<FloorHardShadows>,
    /// Lowest height minus 1.
    pub min_y: f32,
    /// Highest height plus 1.
    pub max_y: f32,
    /// The height grid (still needed by fine-height callers:
    /// loc placement, entities, camera).
    pub heights: FloorHeights,
    /// The construction calls (`FloorCalls`) for the other toolkits.
    pub calls: Option<std::sync::Arc<FloorCalls>>,
}

impl FloorGeometry {
    /// The per-grid-vertex normals the floor derives from the normal-source
    /// grid ([`grid_normals`]), whether or not the
    /// build kept them in [`Self::stream0`] (`has_normals`): the normals a
    /// backend with its own lighting needs (renderer plan §3.2). `None`
    /// without the retained construction calls.
    #[must_use]
    pub fn normal_grid(&self) -> Option<Vec<[f32; 3]>> {
        let calls = self.calls.as_ref()?;
        Some(grid_normals(
            &calls.normal_source,
            calls.tiles_x,
            calls.tiles_z,
            self.heights.tile_size,
        ))
    }

    /// The normal of the floor point `(fine_x, fine_z)` (fine units from the
    /// grid origin, like [`Self::stream0`]'s positions): the containing
    /// tile's corner normals from `grid` ([`Self::normal_grid`]) interpolated
    /// as the vertex writer does ([`tile_point_normal`]). A point
    /// on a tile edge is taken in the tile to its lower x/z; `stream0` keeps
    /// the normal of whichever tile emitted the deduplicated vertex first,
    /// which can differ from this in the last bit.
    #[must_use]
    pub fn normal_at(&self, grid: &[[f32; 3]], fine_x: i32, fine_z: i32) -> [f32; 3] {
        let (ts, shift) = (self.heights.tile_size, self.heights.shift);
        let tile = |fine: i32, tiles: usize| {
            let t = if fine <= 0 {
                0
            } else {
                ((fine - 1) >> shift) as usize
            };
            t.min(tiles.saturating_sub(1))
        };
        let x = tile(fine_x, self.tiles_x);
        let z = tile(fine_z, self.tiles_z);
        let stride = self.tiles_z + 1;
        let corners = [
            grid[x * stride + z],
            grid[x * stride + z + 1],
            grid[(x + 1) * stride + z + 1],
            grid[(x + 1) * stride + z],
        ];
        let lx = (fine_x - ((x as i32) << shift)).clamp(0, ts);
        let lz = (fine_z - ((z as i32) << shift)).clamp(0, ts);
        tile_point_normal(corners, lx, lz, ts)
    }

    /// Triangle count across all tiles.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.index_count / 3
    }

    /// Serialise the streams for byte-level comparison with a reference dump:
    /// `u32 vertex_count, u32 stride_floats, u32 flags(has_depth |
    /// has_normals << 1), stream0 as big-endian f32 bits, base_colours as
    /// big-endian i32, u32 batch_count, per batch: i32 material, f32 scale,
    /// 7 x i32 fog, i64 node_id, colours as i32 BE`, then the hard-shadow
    /// mask: `u8 present, [i32 width, i32 height,
    /// u8[width * height] mask]`, then the floor-sweep-only share of it:
    /// `u8 present, [u8[width * height]]`.
    #[must_use]
    pub fn to_dump(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.vertex_count as u32).to_be_bytes());
        out.extend_from_slice(&(self.stride_floats as u32).to_be_bytes());
        let flags = u32::from(self.has_depth) | (u32::from(self.has_normals) << 1);
        out.extend_from_slice(&flags.to_be_bytes());
        for f in &self.stream0 {
            out.extend_from_slice(&f.to_bits().to_be_bytes());
        }
        for c in &self.base_colours {
            out.extend_from_slice(&c.to_be_bytes());
        }
        out.extend_from_slice(&(self.batches.len() as u32).to_be_bytes());
        for b in &self.batches {
            out.extend_from_slice(&b.material.to_be_bytes());
            out.extend_from_slice(&b.scale.to_bits().to_be_bytes());
            for v in [
                b.water_fog.colour,
                b.water_fog.scale,
                b.water_fog.offset,
                b.water_fog.reserved,
                b.water_fog.extra_a,
                b.water_fog.extra_b,
                b.water_fog.extra_c,
            ] {
                out.extend_from_slice(&v.to_be_bytes());
            }
            out.extend_from_slice(&b.node_id.to_be_bytes());
            for c in &b.colours {
                out.extend_from_slice(&c.to_be_bytes());
            }
        }
        match &self.hard_shadows {
            None => out.extend_from_slice(&[0, 0]),
            Some(m) => {
                out.push(1);
                out.extend_from_slice(&m.width.to_be_bytes());
                out.extend_from_slice(&m.height.to_be_bytes());
                out.extend_from_slice(&m.mask);
                // Floor-sweep-only section (the floor-dump layout). The
                // finished mask also carries the loc entity shadows
                // (`hardshadow::apply_shadow`); only `--dump-floor
                // --no-locs` builds, which stamp none, make the two equal.
                out.push(1);
                out.extend_from_slice(&m.mask);
            }
        }
        out
    }
}

/// The floor data the bake reads.
pub trait LightFloor {
    /// Tile counts along X and Z.
    fn tiles_x(&self) -> i32;
    fn tiles_z(&self) -> i32;
    /// Fine units per tile (512) and its log2 (9).
    fn tile_size(&self) -> i32;
    fn shift(&self) -> i32;
    /// The tile height at `(x, z)` — no bounds check.
    fn tile_height(&self, x: i32, z: i32) -> i32;
    /// The fine height at `(fx, fz)`.
    fn fine_height(&self, fx: i32, fz: i32) -> i32;
    /// The tile's vertex x and z within the tile and HSL16 colour (vertex x,
    /// vertex z within the tile, HSL16 colour or `-1`), `None` when the
    /// tile carries no floor.
    fn tile_verts(&self, x: i32, z: i32) -> Option<(&[i32], &[i32], &[i32])>;
}

impl LightFloor for FloorBuilder {
    fn tiles_x(&self) -> i32 {
        self.heights.tiles_x as i32
    }
    fn tiles_z(&self) -> i32 {
        self.heights.tiles_z as i32
    }
    fn tile_size(&self) -> i32 {
        self.heights.tile_size
    }
    fn shift(&self) -> i32 {
        self.heights.shift
    }
    fn tile_height(&self, x: i32, z: i32) -> i32 {
        self.heights.get_tile_height(x as usize, z as usize)
    }
    fn fine_height(&self, fx: i32, fz: i32) -> i32 {
        self.heights.get_fine_height(fx, fz)
    }
    fn tile_verts(&self, x: i32, z: i32) -> Option<(&[i32], &[i32], &[i32])> {
        let t = self
            .tiles
            .get(x as usize * self.heights.tiles_z + z as usize)?
            .as_ref()?;
        Some((&t.xs, &t.zs, &t.rgb))
    }
}

/// The finished floor's hard-shadow mask for the shadow pass
/// (width, height and mask), `None` when the
/// floor was built without hard shadows (`flags_b & 0x10 == 0`).
#[must_use]
pub fn shadow_mask_view(geometry: &FloorGeometry) -> Option<crate::hardshadow::ShadowMaskView<'_>> {
    let s = geometry.hard_shadows.as_ref()?;
    Some(crate::hardshadow::ShadowMaskView {
        width: s.width as usize,
        height: s.height as usize,
        mask: &s.mask,
        texel_shift: crate::hardshadow::SHADOW_TEXEL_SHIFT as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fine_height_bilinear() {
        // 2x2 tiles, heights rise by 512 per tile in x.
        let mut grid = Vec::new();
        for x in 0..3 {
            for _z in 0..3 {
                grid.push(x * 512);
            }
        }
        let f = FloorHeights::new(2, 2, 512, grid);
        assert_eq!(f.shift, 9);
        assert_eq!(f.tile_size, 512);
        assert_eq!(f.get_fine_height(0, 0), 0);
        assert_eq!(f.get_fine_height(256, 100), 256);
        assert_eq!(f.get_fine_height(512, 0), 512);
        assert_eq!(f.get_fine_height(-1, 0), 0);
        assert_eq!(f.get_fine_height(1024, 0), 0);
        // Clamped variant pins both sample columns to the last tile.
        assert_eq!(f.get_fine_height_clamped(1024, 0), 512);
        // Negative fine x: tile -1 clamps to 0, the sub-tile fraction is
        // `-300 & 511 = 212`, so the sample lands 212/512 into tile 0.
        assert_eq!(f.get_fine_height_clamped(-300, 0), 212);
        assert_eq!(f.get_tile_height_clamped(-5, 9), 0);
    }

    /// The sort carries the items with their keys and, through the `(i & 1)`
    /// pivot jitter, leaves equal keys in the client's order, which is not a
    /// stable sort's (the two 5s come out `h, a`). An `i64::MAX` pivot drops
    /// the jitter. Expected orders are the loop run by hand.
    #[test]
    fn jittered_sort_orders_keys() {
        let mut keys = vec![5_i64, 3, 9, 1, 3, 7, 3, 5];
        let mut items: Vec<char> = "abcdefgh".chars().collect();
        jittered_sort(&mut keys, &mut items);
        assert_eq!(keys, vec![1, 3, 3, 3, 5, 5, 7, 9]);
        assert_eq!(items.iter().collect::<String>(), "dbeghafc");

        let max = i64::MAX;
        let mut keys = vec![4, max, 2, max, 4, 1, max];
        let mut items: Vec<char> = "abcdefg".chars().collect();
        jittered_sort(&mut keys, &mut items);
        assert_eq!(keys, vec![1, 2, 4, 4, max, max, max]);
        assert_eq!(items.iter().collect::<String>(), "fceadgb");

        let mut single = vec![1_i64];
        let mut one = vec!['x'];
        jittered_sort(&mut single, &mut one);
        assert_eq!((single, one), (vec![1], vec!['x']));
    }
}
