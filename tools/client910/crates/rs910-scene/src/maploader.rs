//! Landscape decode and the floor (terrain) mesh build.
//!
//! Pipeline (driven by the scene rebuild, see `rebuild.rs`):
//! 1. [`MapLoader::read_normal_landscape`] / [`MapLoader::read_region_landscape`]
//!    per map square → heightmap, underlay/overlay
//!    wire ids, overlay shape/rotation, tile flags.
//! 2. [`MapLoader::build_floors`] → one [`FloorBuilder`] per level.
//! 3. (locs are placed at this point — walls stamp the floor shade map; see
//!    [`MapLoader::stamp_wall_shadows`].)
//! 4. [`MapLoader::build_ground`] → 11x11 underlay blend, then
//!    per tile the blended or unblended tile builder feeding the floor builders, then finalise.
//!
//! Wire ids are the raw LAND smart ints: `0` = absent, config id = `wire - 1`.
//!
//! Deviations:
//! - Missing flo config ids resolve to a default-initialised entry, which is
//!   what the config store hands back for an absent file.
//! - The scene's hard-shadow build runs as [`crate::hardshadow::sweep_shadows`]; the static
//!   lighting build only runs when lights are supplied, since point lights belong to the
//!   scene-graph phase.

mod shape_tables;
pub use shape_tables::{
    BLEND_A, BLEND_B, BLEND_C, BLEND_EDGE_TRI, EDGE_MASK_BLEND, EDGE_MASK_NOBLEND, OVERLAY_POINT,
    OVERLAY_TRIS_BLEND, OVERLAY_TRIS_SIMPLE, OVERLAY_TRIS_SPLIT, SIMPLE_A, SIMPLE_B, SIMPLE_C,
    SPLIT_A, SPLIT_B, SPLIT_C, SPLIT_EDGE_TRI, TILE_POINT_X, TILE_POINT_Z, UNDERLAY_POINT,
    UNDERLAY_TRIS_BLEND, UNDERLAY_TRIS_SIMPLE, UNDERLAY_TRIS_SPLIT,
};
mod floor_types;
#[cfg(test)]
use floor_types::default_flo_tables;
pub use floor_types::{FloTables, Ov, Ul};
mod landscape;
use landscape::rotate_point;
mod ground;
mod tile_mesh;

use crate::floor::{FloorBuilder, FloorGeometry, FloorHeights, WaterFogData};

use crate::texture::MaterialStore;

use crate::tileflags::SceneLevelTileFlags;

pub use rs910_config::landscape_packet::*;

// ---------------------------------------------------------------------------
// Height noise
// ---------------------------------------------------------------------------

// The height noise (perlin, scale, interpolation, smooth noise) lives in `rs910_core::perlin`,
// shared with `map.rs` and `protocol910/terrain.rs`; pinned by client910's
// scene golden test for the perlin noise and colour blending.

/// Lerp two HSL16 shorts field-wise, `t` in `0..=128`.
#[must_use]
pub fn blend_colours(a: i32, b: i32, t: i32) -> i32 {
    if a == b {
        return a;
    }
    let inv = 128 - t;
    let lum = ((a & 0x7F) * inv + (b & 0x7F) * t) >> 7;
    let sat = ((a & 0x380) * inv + (b & 0x380) * t) >> 7;
    let hue = ((a & 0xFC00) * inv + (b & 0xFC00) * t) >> 7;
    (hue & 0xFC00) | (sat & 0x380) | (lum & 0x7F)
}

// ---------------------------------------------------------------------------
// Scene-side state the loader writes
// ---------------------------------------------------------------------------

/// Per-tile water-fog arrays; present
/// only when the scene has an underwater layer.
#[derive(Clone, Debug)]
pub struct WaterFogGrid {
    max_z: usize,
    colour: Vec<i32>,
    scale: Vec<i16>,
    offset: Vec<i8>,
    extra_a: Vec<i8>,
    extra_b: Vec<i8>,
    extra_c: Vec<i8>,
}

impl WaterFogGrid {
    /// Zeroed grid.
    #[must_use]
    pub fn new(max_x: usize, max_z: usize) -> Self {
        let n = max_x * max_z;
        Self {
            max_z,
            colour: vec![0; n],
            scale: vec![0; n],
            offset: vec![0; n],
            extra_a: vec![0; n],
            extra_b: vec![0; n],
            extra_c: vec![0; n],
        }
    }

    /// Store the fog of tile `(x, z)`.
    pub fn set(&mut self, x: usize, z: usize, fog: WaterFogData) {
        let i = x * self.max_z + z;
        self.colour[i] = fog.colour | (0xFF00_0000_u32 as i32);
        self.scale[i] = fog.scale as i16;
        self.offset[i] = fog.offset as i8;
        self.extra_a[i] = fog.extra_a as i8;
        self.extra_b[i] = fog.extra_b as i8;
        self.extra_c[i] = fog.extra_c as i8;
    }

    /// The stored fog fields of a tile bundled as a
    /// [`WaterFogData`], the way the tile builders read them back.
    #[must_use]
    pub fn get(&self, x: usize, z: usize) -> WaterFogData {
        let i = x * self.max_z + z;
        WaterFogData {
            colour: self.colour[i] & 0xFF_FFFF,
            scale: i32::from(self.scale[i] as u16),
            offset: i32::from(self.offset[i] as u8),
            reserved: 0,
            extra_a: i32::from(self.extra_a[i] as u8),
            extra_b: i32::from(self.extra_b[i] as u8),
            extra_c: i32::from(self.extra_c[i] as u8),
        }
    }
}

/// Which tiles the scene has allocated:
/// `marks[level][x * max_z + z]`. Bridges do not exist yet at build time, so
/// the `Tile.level` bump is not modelled here.
#[derive(Clone, Debug)]
pub struct TileMarks {
    max_z: usize,
    marks: Vec<Vec<bool>>,
}

impl TileMarks {
    /// Empty marks for `levels` planes.
    #[must_use]
    pub fn new(levels: usize, max_x: usize, max_z: usize) -> Self {
        Self {
            max_z,
            marks: vec![vec![false; max_x * max_z]; levels],
        }
    }

    /// Allocate `level` and every plane below.
    pub fn create_tile(&mut self, level: usize, x: usize, z: usize) {
        for l in (0..=level).rev() {
            self.marks[l][x * self.max_z + z] = true;
        }
    }

    /// Whether `(level, x, z)` has a tile.
    #[must_use]
    pub fn has(&self, level: usize, x: usize, z: usize) -> bool {
        self.marks[level][x * self.max_z + z]
    }
}

/// The floor-relevant slice of `Scene`: per-level floor builders (normal +
/// underwater sets), finished geometry, water fog and
/// tile marks. `MapLoader` writes into it while it builds.
#[derive(Debug)]
pub struct SceneFloors {
    /// `maxTileX`.
    #[allow(dead_code, reason = "the scene size; nothing reads it yet")]
    pub max_x: usize,
    /// `maxTileZ`.
    #[allow(dead_code, reason = "the scene size; nothing reads it yet")]
    pub max_z: usize,
    /// Normal-set floor builders, before finalise.
    pub normal_builders: Vec<Option<FloorBuilder>>,
    /// Normal-set floor geometry, after finalise.
    pub normal: Vec<Option<FloorGeometry>>,
    /// Underwater-set floor builders, before finalise (1 level or none).
    pub underwater_builders: Vec<Option<FloorBuilder>>,
    /// Underwater-set floor geometry, after finalise.
    pub underwater: Vec<Option<FloorGeometry>>,
    /// `waterFog*` arrays (only with an underwater layer).
    pub water_fog: Option<WaterFogGrid>,
    /// `normalTiles` allocation marks.
    pub normal_tiles: TileMarks,
    /// `underwaterTiles` allocation marks.
    pub underwater_tiles: TileMarks,
    /// Baked static point lights per level.
    pub lights: Vec<Vec<crate::floorlight::BakedLight>>,
}

impl SceneFloors {
    /// An empty floor slice for a scene of the given size; the underwater layer (water fog)
    /// exists only when `has_underwater`.
    #[must_use]
    pub fn new(levels: usize, max_x: usize, max_z: usize, has_underwater: bool) -> Self {
        Self {
            max_x,
            max_z,
            normal_builders: (0..levels).map(|_| None).collect(),
            normal: (0..levels).map(|_| None).collect(),
            underwater_builders: if has_underwater {
                vec![None]
            } else {
                Vec::new()
            },
            underwater: if has_underwater {
                vec![None]
            } else {
                Vec::new()
            },
            water_fog: has_underwater.then(|| WaterFogGrid::new(max_x, max_z)),
            normal_tiles: TileMarks::new(levels, max_x, max_z),
            underwater_tiles: TileMarks::new(1, max_x, max_z),
            lights: (0..levels).map(|_| Vec::new()).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// MapLoader
// ---------------------------------------------------------------------------

/// One landscape tile to decode: its level, the window tile it lands on, the
/// extra shift of its height vertex, the absolute tile (for the noise height
/// fallback), the region rotation and whether the values are only consumed
/// (`skip_store`, for the far edge of a rotated chunk).
#[derive(Clone, Copy, Debug)]
pub struct TileRead {
    pub level: usize,
    pub tile: [i32; 2],
    pub shift: [i32; 2],
    pub absolute: [i32; 2],
    pub rotation: i32,
    pub skip_store: bool,
}

/// `MapLoader` state.
#[derive(Debug)]
pub struct MapLoader<'a> {
    flo: &'a FloTables,
    materials: &'a MaterialStore,
    /// `levels`.
    pub levels: usize,
    /// `maxTileX`.
    pub max_x: usize,
    /// `maxTileZ`.
    pub max_z: usize,
    /// `underwater`.
    pub underwater: bool,
    /// `sceneryShadows` preference.
    pub scenery_shadows: i32,
    /// `isWaterDetail` (`waterDetail == 2`).
    pub is_water_detail: bool,
    /// `isLightingDetail`.
    pub is_lighting_detail: bool,
    /// `isGroundBlending`.
    pub is_ground_blending: bool,
    /// `isTexturing`.
    pub is_texturing: bool,
    /// `isBlending` (`enableBlending`).
    pub is_blending: bool,
    /// `levelHeightmap[level]`: `(max_x + 1) * (max_z + 1)` by x.
    pub level_heightmap: Vec<Vec<i32>>,
    /// `levelOccludemap[level]`: `(max_x + 1) * (max_z + 1)` by x.
    pub level_occludemap: Vec<Vec<u8>>,
    /// `levelTileUnderlayIds[level]`: `max_x * max_z` by x.
    pub level_tile_underlay_ids: Vec<Vec<i16>>,
    /// `levelTileOverlayIds[level]`.
    pub level_tile_overlay_ids: Vec<Vec<i16>>,
    /// `levelTileOverlayShape[level]`.
    pub level_tile_overlay_shape: Vec<Vec<i8>>,
    /// `levelTileOverlayRotation[level]`.
    pub level_tile_overlay_rotation: Vec<Vec<i8>>,
    /// `cameraHeightOffsetMap` (filled by the environment trailer; unused
    /// here, kept for the World port).
    #[allow(
        dead_code,
        reason = "filled by the environment trailer; nothing reads it yet"
    )]
    pub camera_height_offset_map: Vec<Vec<i8>>,
    blend_hue: Vec<i32>,
    blend_saturation: Vec<i32>,
    blend_lightness: Vec<i32>,
    blend_chroma: Vec<i32>,
    blend_magnitude: Vec<i32>,

    // Per-tile scratch.
    /// The point indices of the triangle (or the two of an edge fan) being
    /// emitted.
    triangle_points: [i32; 6],
    slots: BlendSlots,
    shape_id: i32,
    angle: i32,
    vertex_count: usize,
    shape_vertex_index: usize,
    tile_material: i32,
    material_scale: i32,
    tile_rgb: i32,
    hard_shadow: bool,
    blend_overlay: bool,
    use_simple_triangles: bool,
    use_vertex_colours: bool,
    underlay_vertex_count: i32,
    overlay_vertex_count: i32,
    vertex_table_a: &'static [i32],
    vertex_table_b: &'static [i32],
    vertex_table_c: &'static [i32],
    shape_vertex_counts: Option<&'static [i32; 4]>,
}

/// The colouring of the perimeter of a tile: eight points (corners and edge
/// midpoints) plus spare slots, as the neighbouring blending overlays write
/// it. `priority` is -1 for a slot no overlay has claimed; `edges` holds the
/// direction bits of the overlays that reach the slot.
#[derive(Clone, Copy, Debug, Default)]
struct BlendSlots {
    rgb: [i32; 13],
    average: [i32; 13],
    material: [i32; 13],
    material_scale: [i32; 13],
    priority: [i32; 13],
    edges: [i32; 13],
}

/// What building one level's tiles reads and writes: the floor under
/// construction and its level, the blended underlay colours, the heights of
/// the other floors the tile depths are relative to (`underwater_heights`
/// while building the surface, `surface_heights` while building the seabed),
/// the tile flags, the water fog grid and the tile marks.
struct LevelBuild<'a> {
    floor: &'a mut FloorBuilder,
    level: usize,
    blended: &'a [i32],
    underwater_heights: Option<&'a FloorHeights>,
    surface_heights: Option<&'a FloorHeights>,
    flags: &'a SceneLevelTileFlags,
    water_fog: Option<&'a mut WaterFogGrid>,
    tiles: &'a mut TileMarks,
}

/// The per-tile tables of one level: overlay shapes and rotations, and the
/// underlay and overlay ids (all indexed by tile).
#[derive(Clone, Copy)]
struct TileTables<'a> {
    shapes: &'a [i8],
    rots: &'a [i8],
    ul_ids: &'a [i16],
    ov_ids: &'a [i16],
}

/// The heights a tile's vertices are measured against: its own floor, and
/// the surface and seabed floors of the other set when there is one.
#[derive(Clone, Copy)]
struct FloorSources<'a> {
    heights: &'a FloorHeights,
    surface: Option<&'a FloorHeights>,
    underwater: Option<&'a FloorHeights>,
}

/// A tile's underlay and the underlay ids of the tile itself and of its
/// north, north-east and east neighbours (0 where a neighbour has none).
#[derive(Clone, Copy)]
struct UnderlayTile {
    underlay: Option<Ul>,
    here: i32,
    north: i32,
    ne: i32,
    east: i32,
}

/// What occlusion marking needs of a tile: its underlay and overlay and
/// their raw ids.
#[derive(Clone, Copy)]
struct OcclusionTile {
    underlay: Option<Ul>,
    overlay: Option<Ov>,
    ul_wire: i32,
    ov_wire: i32,
}

/// A three-point walk along a neighbour's perimeter: the slot and the
/// neighbour's point index to start at, and how each steps (both are masked
/// to 0..8 every step).
#[derive(Clone, Copy)]
struct SweepPath {
    slot: i32,
    point: i32,
    slot_step: i32,
    point_step: i32,
}

/// Per-tile vertex arrays handed to the floor builder's `add_tile`.
struct TileArrays {
    /// Alternate colours (only with `useVertexColours`).
    alt: Option<Vec<i32>>,
    /// X.
    xs: Vec<i32>,
    /// Z.
    zs: Vec<i32>,
    /// Rgb.
    rgb: Vec<i32>,
    /// Material (`-1` init).
    material: Vec<i32>,
    /// Material scale.
    scale: Vec<i32>,
    /// Water depth (only with surface heights).
    depth: Option<Vec<i32>>,
    /// Vertical offset (with surface or underwater heights).
    offset: Option<Vec<i32>>,
}

impl<'a> MapLoader<'a> {
    /// A loader for a scene of `levels` planes and the given extent.
    #[must_use]
    pub fn new(
        flo: &'a FloTables,
        materials: &'a MaterialStore,
        levels: usize,
        max_x: usize,
        max_z: usize,
        underwater: bool,
    ) -> Self {
        let tiles = max_x * max_z;
        let verts = (max_x + 1) * (max_z + 1);
        Self {
            flo,
            materials,
            levels,
            max_x,
            max_z,
            underwater,
            scenery_shadows: 0,
            is_water_detail: false,
            is_lighting_detail: false,
            is_ground_blending: false,
            is_texturing: false,
            is_blending: false,
            level_heightmap: vec![vec![0; verts]; levels],
            level_occludemap: vec![vec![0; verts]; levels],
            level_tile_underlay_ids: vec![vec![0; tiles]; levels],
            level_tile_overlay_ids: vec![vec![0; tiles]; levels],
            level_tile_overlay_shape: vec![vec![0; tiles]; levels],
            level_tile_overlay_rotation: vec![vec![0; tiles]; levels],
            camera_height_offset_map: Vec::new(),
            blend_hue: Vec::new(),
            blend_saturation: Vec::new(),
            blend_lightness: Vec::new(),
            blend_chroma: Vec::new(),
            blend_magnitude: Vec::new(),
            triangle_points: [0; 6],
            slots: BlendSlots::default(),
            shape_id: 0,
            angle: 0,
            vertex_count: 0,
            shape_vertex_index: 0,
            tile_material: 0,
            material_scale: 0,
            tile_rgb: 0,
            hard_shadow: false,
            blend_overlay: false,
            use_simple_triangles: false,
            use_vertex_colours: false,
            underlay_vertex_count: 0,
            overlay_vertex_count: 0,
            vertex_table_a: &[],
            vertex_table_b: &[],
            vertex_table_c: &[],
            shape_vertex_counts: None,
        }
    }
    /// A loader over empty flo tables (tests).
    #[cfg(test)]
    pub(crate) fn new_empty(
        materials: &'a MaterialStore,
        levels: usize,
        max_x: usize,
        max_z: usize,
    ) -> Self {
        Self::new(default_flo_tables(), materials, levels, max_x, max_z, false)
    }
    /// Turn tile blending on.
    pub fn enable_blending(&mut self) {
        self.is_blending = true;
    }
    /// Turn tile blending off and free the blend accumulators.
    pub fn disable_blending(&mut self) {
        self.blend_hue = Vec::new();
        self.blend_saturation = Vec::new();
        self.blend_lightness = Vec::new();
        self.blend_chroma = Vec::new();
        self.blend_magnitude = Vec::new();
        self.is_blending = false;
    }
    #[inline]
    pub(super) fn vi(&self, x: usize, z: usize) -> usize {
        x * (self.max_z + 1) + z
    }
    #[inline]
    pub(super) fn ti(&self, x: usize, z: usize) -> usize {
        x * self.max_z + z
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_landscape_copies_from_plane_below() {
        let materials = MaterialStore::default();
        let mut loader = MapLoader::new_empty(&materials, 2, 8, 8);
        for v in loader.level_heightmap[0].iter_mut() {
            *v = -320;
        }
        loader.clear_landscape(0, 0, 8, 8);
        // Plane 0 of a cleared square is zeroed (`source_level > 0 ? ... : 0`), and
        // plane 1 sits 960 below it.
        assert_eq!(loader.level_heightmap[0][loader.vi(3, 3)], 0);
        assert_eq!(loader.level_heightmap[1][loader.vi(3, 3)], -960);
    }
    #[test]
    fn region_heights_follow_destination_planes_for_each_rotation_and_edge() -> anyhow::Result<()> {
        use crate::protocol910::terrain::{RegionCopy, Terrain};
        const PLANES: usize = 4;
        const SQUARE_TILES: usize = 64;
        const CHUNK_TILES: usize = 8;
        const WINDOW_TILES: usize = 24;
        const HEIGHT_PRESENT: u8 = 8;
        const FLAGS_PRESENT: u8 = 2;
        const ROOF_FLAG: u8 = 4;
        const ZERO_HEIGHT_SENTINEL: u8 = 1;
        const DESTINATION_BELOW_BASE: i32 = -2100;
        const DESTINATION_BELOW_GRADIENT: i32 = 7;
        const QUARTER_TURNS: i32 = 4;
        const HEIGHT_FORMS: usize = 3;
        let materials = MaterialStore::default();
        for source_plane in [0, PLANES - 1] {
            let mut land = Vec::new();
            for plane in 0..PLANES {
                for x in 0..SQUARE_TILES {
                    for z in 0..SQUARE_TILES {
                        let height = match (x + z) % HEIGHT_FORMS {
                            0 => None,
                            1 => Some(ZERO_HEIGHT_SENTINEL),
                            _ => Some(
                                (plane + x % CHUNK_TILES + z % CHUNK_TILES + HEIGHT_FORMS) as u8,
                            ),
                        };
                        land.push(FLAGS_PRESENT | height.map_or(0, |_| HEIGHT_PRESENT));
                        land.push(ROOF_FLAG);
                        land.extend(height);
                    }
                }
            }
            for destination_plane in [0, PLANES - 1] {
                for rotation in 0..QUARTER_TURNS {
                    for source_chunk in [0, SQUARE_TILES as i32 / CHUNK_TILES as i32 - 1] {
                        let mut cpu = Terrain::new(WINDOW_TILES, WINDOW_TILES).unwrap();
                        let mut scene =
                            MapLoader::new_empty(&materials, PLANES, WINDOW_TILES, WINDOW_TILES);
                        if destination_plane > 0 {
                            for (index, height) in scene.level_heightmap[destination_plane - 1]
                                .iter_mut()
                                .enumerate()
                            {
                                *height = DESTINATION_BELOW_BASE
                                    + index as i32 * DESTINATION_BELOW_GRADIENT;
                                let cpu_index = cpu.point(
                                    destination_plane - 1,
                                    index / (WINDOW_TILES + 1),
                                    index % (WINDOW_TILES + 1),
                                );
                                cpu.heights[cpu_index] = *height;
                            }
                        }
                        let before = cpu.clone();
                        let copy = RegionCopy {
                            level: destination_plane,
                            tile_x: CHUNK_TILES as i32,
                            tile_z: CHUNK_TILES as i32,
                            src_level: source_plane,
                            src_chunk_x: source_chunk,
                            src_chunk_z: source_chunk,
                            rotation,
                        };
                        let decoded = cpu.read_region(&land, copy).unwrap();
                        assert_eq!(cpu, before, "input terrain remains an atomic transaction");
                        let mut packet = Packet::new(&land);
                        let mut flags =
                            SceneLevelTileFlags::new(PLANES, WINDOW_TILES, WINDOW_TILES);
                        scene.read_region_landscape(&mut packet, &mut flags, copy)?;
                        assert_eq!(decoded.consumed, packet.pos, "same LAND cursor");
                        assert_eq!(
                            decoded.consumed,
                            land.len(),
                            "complete source square consumed"
                        );
                        for plane in 0..PLANES {
                            for x in 0..=WINDOW_TILES {
                                for z in 0..=WINDOW_TILES {
                                    assert_eq!(
                                        decoded.state.heights[decoded.state.point(plane,x,z)],
                                        scene.level_heightmap[plane][scene.vi(x,z)],
                                        "source={source_plane} destination={destination_plane} rotation={rotation} chunk={source_chunk} vertex={plane}:{x}:{z}"
                                    );
                                }
                            }
                        }
                        for x in 0..WINDOW_TILES {
                            for z in 0..WINDOW_TILES {
                                assert_eq!(
                                    decoded.state.tiles
                                        [decoded.state.tile(destination_plane, x, z)]
                                    .flags,
                                    flags.get(destination_plane, x, z)
                                );
                            }
                        }
                        assert!(cpu.read_region(&land[..land.len() - 1], copy).is_err());
                        assert_eq!(cpu, before, "truncated LAND does not mutate its input");
                    }
                }
            }
        }
        Ok(())
    }
}
