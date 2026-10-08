//! NXT terrain (renderer plan M10): the scene's ground built from map file
//! 5, NXT's own terrain (`rs910_config::nxt::map_terrain`), instead of the
//! classic floor meshes; CPU only (the GPU half is `crate::frame::terrain`, the
//! WGSL [`wgsl::TERRAIN_WGSL`], the texture array [`atlas`]).
//!
//! # The data (`nxt-data-formats.md` §2, proven over the pack)
//!
//! Per map square and level 66 x 66 tiles (a one-tile border of the
//! neighbours' tiles): a height code, underlay and overlay ids, the
//! overlay's shape and rotation, the tile's colour (the classic floor's
//! colour blend, precomputed), and for a water tile its surface code and
//! the ids of its bed. The heights are the classic floor's (classic `y` is
//! `-y_up`), so the terrain sits exactly on the classic floor (collision,
//! picking and loc placement keep reading the classic floor heights;
//! nothing here changes them).
//!
//! # The builder
//!
//! - Sources: per tile its own source (a plain underlay or overlay), or the
//!   overlay part (srcA) and the underlay part (srcB) of a shaped overlay;
//!   a shaped water tile's parts are its LAND and bed underlays. A source
//!   is a material, a texture scale (`material_scale / 512` tiles per
//!   repeat), a colour ([`terrain_colour`]), a blend flag and a priority;
//!   colour 0 (a magenta overlay) draws nothing.
//! - Shapes ([`shape_triangles`]): the 12 overlay shapes (classes Plain,
//!   Basic, CornerTri, HalfSquare), rotated by corner index ([`rotate`]);
//!   the plain tile's diagonal comes from its neighbours' underlays
//!   (`plain_diagonal`).
//! - Vertices: a corner takes the highest-priority blending source owning
//!   its grid point in the four tiles around it (the shared vertex and the
//!   dominant source), its height (a water tile: its bed) and the grid
//!   normal; centre and edge points are bilinear. Every triangle carries
//!   its three vertices' materials as three slots and each vertex a
//!   one-hot weight, so the shader blends up to three textures per
//!   triangle. Non-indexed, as the modern client draws them; grouped by
//!   tile for the plan's tile selection.
//!
//! - Colours: the source's HSL at the classic floor's vertex shade (`74 -`
//!   the level's blurred shade map, [`shade`]; the fixed 74 where the map
//!   is 0), so building interiors keep the classic floor's darkening,
//!   which the modern client leaves to its indoor probes.
//!
//! Not reproduced (deliberate): the modern client zeroes the normals'
//! slopes at square edges (its patches are per square; the scene here is
//! continuous) and crosses the UV scales of slots 1 and 3; its low-detail
//! colours and the water-depth colour fog of its terrain vertices.
//!
//! [`TerrainScene::build`] builds the installed scene's terrain once.

pub mod atlas;

#[cfg(test)]
mod tests;

use crate::draw::FloorSelection;
use crate::models::draw_list::DrawList;
use crate::scene_snapshot::SceneSnapshot;
use crate::texture::MaterialStore;

/// Fine units per tile.
pub const GRID_SIZE: f32 = 512.0;

/// Whether the terrain draws the specular term (off only in the
/// `CLIENT910_MODERN_TERRAIN=no-spec` debug view).
#[must_use]
pub fn specular_on() -> bool {
    crate::modern_debug_flags::flags().terrain
        != Some(crate::modern_debug_flags::TerrainDebug::NoSpecular)
}

/// One terrain vertex (`crate::frame::terrain`'s layout: the vertex
/// shader's inputs, packed).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TerrainVertex {
    /// Scene-local fine units, classic y (down).
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// The vertex colour, display-referred RGBA8.
    pub colour: [u8; 4],
    /// Texture-array layer of the three material slots (`0xFFFF`: none),
    /// w padding.
    pub slots: [u16; 4],
    /// The slots' texture scales (xyz).
    pub scale: [f32; 4],
    /// xyz: 255 for this vertex's own slot (the blend weight), w: the
    /// level (point-light grid lookup).
    pub weight: [u8; 4],
}

/// What identifies an installed scene's terrain: the scene base, its size,
/// and the classic floors' identity (a rebuild makes new floors).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneKey {
    pub base: [i32; 2],
    pub floors: Vec<Option<(usize, usize, usize)>>,
}

impl SceneKey {
    #[must_use]
    pub fn of(snapshot: &SceneSnapshot<'_>) -> Self {
        Self {
            base: snapshot.floor_base,
            floors: snapshot
                .floors
                .iter()
                .map(|f| {
                    f.as_ref().map(|g| {
                        let id = g.calls.as_ref().map_or(g.stream0.as_ptr() as usize, |c| {
                            std::sync::Arc::as_ptr(c) as usize
                        });
                        (id, g.tiles_x, g.tiles_z)
                    })
                })
                .collect(),
        }
    }
}

/// One level's mesh: vertices, triangle indices grouped by tile, and each
/// tile's range (`z * tiles_x + x`, as `FloorSelection::tiles` names
/// tiles).
#[derive(Clone, Debug, Default)]
pub struct LevelMesh {
    pub level: usize,
    pub tiles: [usize; 2],
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u32>,
    pub tile_ranges: Vec<(u32, u32)>,
    /// Per grid vertex (`x * (tiles_z + 1) + z`): its height (classic y) and
    /// whether its tile (the one to its north-east) is a water tile (the
    /// height is then the bed's). Tests and the log.
    pub grid: Vec<(f32, bool)>,
}

impl LevelMesh {
    /// The indices of the tiles `selection` shows, in its tile order.
    #[must_use]
    pub fn select(&self, selection: &FloorSelection) -> Vec<u32> {
        let mut out = Vec::new();
        for tile in selection.tiles(self.tiles[0], self.tiles[1]) {
            if let Some(&(start, count)) = self.tile_ranges.get(tile) {
                out.extend_from_slice(&self.indices[start as usize..(start + count) as usize]);
            }
        }
        out
    }

    /// Whether tile `(x, z)` has terrain triangles.
    #[must_use]
    pub fn has_tile(&self, x: usize, z: usize) -> bool {
        self.tile_ranges
            .get(z * self.tiles[0] + x)
            .is_some_and(|&(_, n)| n > 0)
    }
}

/// The installed scene's terrain: per level its mesh, the texture-array
/// layers and their settings.
#[derive(Default)]
pub struct TerrainScene {
    pub base: [i32; 2],
    pub tiles: [usize; 2],
    /// Per scene level (`FloorGeometry` levels): the terrain mesh, `None`
    /// where the level keeps the classic floor.
    pub levels: Vec<Option<LevelMesh>>,
    /// The material id of each texture-array layer.
    pub layer_materials: Vec<i32>,
    pub layer_texels: Vec<atlas::LayerLevels>,
    /// Per layer: `[specular power, specular mask, textured, property
    /// byte]` (`TerrainMaterial` of the WGSL).
    pub layer_params: Vec<[f32; 4]>,
    /// Whether the terrain replaces the classic floor in this scene (see
    /// [`TerrainScene::build`]).
    pub usable: bool,
    /// Why not, when not.
    pub reason: String,
    pub stats: BuildStats,
}

/// Build counters (the log and the tests).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildStats {
    pub squares: usize,
    pub tiles: usize,
    pub triangles: usize,
    pub vertices: usize,
    pub textured_triangles: usize,
    pub blended_triangles: usize,
}

impl TerrainScene {
    /// One line for the log.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} ({}): {} squares, {} levels, {} tiles, {} triangles ({} textured, {} blending materials), {} vertices, {} layers",
            if self.usable { "on" } else { "off" },
            self.reason,
            self.stats.squares,
            self.levels.iter().filter(|l| l.is_some()).count(),
            self.stats.tiles,
            self.stats.triangles,
            self.stats.textured_triangles,
            self.stats.blended_triangles,
            self.stats.vertices,
            self.layer_materials.len(),
        )
    }

    /// `CLIENT910_MODERN_CHECK` (M10): per level the terrain draws, the
    /// selected tiles where the classic floor has a non-water triangle must
    /// all have terrain triangles, a terrain tile the classic floor has no
    /// ground on must be a classic water tile (its bed), and the terrain draws no tile
    /// outside the selection (by construction: its indices are the
    /// selection's).
    /// Returns `(ok, report)`.
    #[must_use]
    pub fn check(&self, list: &DrawList<'_>, materials: Option<&MaterialStore>) -> (bool, String) {
        let mut ok = true;
        let mut parts = Vec::new();
        for floor in &list.floors {
            let Some(Some(mesh)) = self.levels.get(floor.level) else {
                continue;
            };
            let g = floor.geometry;
            let (mut classic, mut missing, mut terrain, mut beds, mut extra) = (0, 0, 0, 0, 0);
            for tile in floor.selection.tiles(g.tiles_x, g.tiles_z) {
                let (x, z) = (tile % g.tiles_x, tile / g.tiles_x);
                let (mut ground, mut water) = (false, false);
                for b in &g.batches {
                    if b.tri_mask.get(tile).is_some_and(|&m| m != 0) {
                        if materials
                            .is_some_and(|m| crate::water_body::is_water_material(m, b.material))
                        {
                            water = true;
                        } else {
                            ground = true;
                        }
                    }
                }
                let ours = mesh.has_tile(x, z);
                classic += usize::from(ground);
                terrain += usize::from(ours);
                missing += usize::from(ground && !ours);
                // A terrain tile the classic floor draws only as water is its bed.
                beds += usize::from(ours && !ground && water);
                extra += usize::from(ours && !ground && !water);
            }
            ok &= missing == 0 && extra == 0;
            parts.push(format!(
                "l{}: {classic} classic ground tiles, {terrain} terrain tiles, {missing} missing, {beds} water beds, {extra} other terrain-only",
                floor.level
            ));
        }
        (ok, parts.join("; "))
    }
}

// ---------------------------------------------------------------------------
// The builder (see the module docs)
// ---------------------------------------------------------------------------

use rs910_config::flo::FloStore;
use rs910_config::nxt::map_terrain::{NxtTerrainTile, TERRAIN_SIDE};
use rs910_config::nxt::{map_group, MAP_ARCHIVE, TERRAIN_FILE};

/// One material layer of a tile: texture, scale, colour, blend flag,
/// priority.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    /// Material id (-1: untextured).
    pub material: i32,
    /// Tiles per texture repeat.
    pub scale: f32,
    /// Display-referred RGBA; alpha 0: the source draws no triangles
    /// (colour 0).
    pub colour: [u8; 4],
    pub blend: bool,
    pub priority: i32,
    /// The HSL16 colour [`Self::colour`] is made from (lane Q-FLOOR: the
    /// vertex shade recolours it, [`shade`]).
    pub hsl: u16,
}

impl Source {
    /// The empty source: no triangles.
    pub const EMPTY: Self = Self {
        material: -1,
        scale: 1.0,
        colour: [0; 4],
        blend: false,
        priority: -1,
        hsl: 0,
    };

    #[must_use]
    pub fn draws(&self) -> bool {
        self.colour[3] != 0
    }
}

/// Which source a shape triangle takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The tile's own source (shape 0).
    Own,
    /// The overlay part (srcA).
    A,
    /// The underlay part (srcB).
    B,
}

/// A tile-local point: `(x, z)` in tile units (0..1), x east, z north.
pub type Point = [f32; 2];

const SW: Point = [0.0, 0.0];
const NW: Point = [0.0, 1.0];
const NE: Point = [1.0, 1.0];
const SE: Point = [1.0, 0.0];
const SM: Point = [0.5, 0.0];
const NM: Point = [0.5, 1.0];
const WM: Point = [0.0, 0.5];
const C: Point = [0.5, 0.5];
const F: Point = [0.25, 0.75];

/// The triangles of overlay shape `shape` at rotation 0, in the modern
/// client's order (tile-local, counter-clockwise seen from above with x
/// east and z north): classes Plain (0, 1), Basic (2-5, 9-11: a fan about
/// the centre point), CornerTri (7, 8), HalfSquare (6). Shape 0's diagonal
/// is chosen per tile ([`plain_diagonal`]). `None`: no such shape (ids
/// above 11).
#[must_use]
pub fn shape_triangles(shape: u8) -> Option<&'static [(Part, [Point; 3])]> {
    use Part::{Own, A, B};
    Some(match shape {
        0 => &[(Own, [SW, SE, NW]), (Own, [SE, NE, NW])],
        1 => &[(A, [SW, SE, NW]), (B, [SE, NE, NW])],
        2 => &[(A, [NM, NW, SW]), (B, [NM, SW, SE]), (B, [NM, SE, NE])],
        3 => &[(A, [NM, SE, NE]), (B, [NM, SW, SE]), (B, [NM, NW, SW])],
        4 => &[(A, [NM, SW, SE]), (A, [NM, SE, NE]), (B, [NM, NW, SW])],
        5 => &[(A, [NM, SW, SE]), (A, [NM, NW, SW]), (B, [NM, SE, NE])],
        6 => &[
            (A, [NM, NW, SW]),
            (A, [NM, SW, SM]),
            (B, [NM, SM, SE]),
            (B, [NM, SE, NE]),
        ],
        7 => &[
            (A, [WM, SW, SM]),
            (B, [WM, NE, NW]),
            (B, [WM, SM, NE]),
            (B, [SM, SE, NE]),
        ],
        8 => &[
            (A, [WM, NE, NW]),
            (A, [WM, SM, NE]),
            (A, [SM, SE, NE]),
            (B, [WM, SW, SM]),
        ],
        9 => &[
            (A, [F, NE, NW]),
            (A, [F, NW, SW]),
            (B, [F, SW, SE]),
            (B, [F, SE, NE]),
        ],
        10 => &[
            (A, [F, SW, SE]),
            (A, [F, SE, NE]),
            (B, [F, NE, NW]),
            (B, [F, NW, SW]),
        ],
        11 => &[
            (A, [C, SW, SE]),
            (B, [C, SE, NE]),
            (B, [C, NE, NW]),
            (B, [C, NW, SW]),
        ],
        _ => return None,
    })
}

/// Rotate a tile-local point by `rotation` quarter turns: corner `c(j)`
/// goes to `c(j + r)` (SW -> NW -> NE -> SE; corner index `(j + r) & 3`,
/// as the classic rotation does).
#[must_use]
pub fn rotate(p: Point, rotation: u8) -> Point {
    let mut p = p;
    for _ in 0..(rotation & 3) {
        p = [p[1], 1.0 - p[0]];
    }
    p
}

/// The NXT tile data of one scene level over the scene (with a one-tile
/// margin on each side for normals and the vertex sharing).
struct LevelGrid {
    /// Tiles per side of the scene (`FloorGeometry::tiles_x/z`).
    tiles: [usize; 2],
    /// `(x + 1) * (tiles_z + 2) + z + 1`, x and z from -1.
    cells: Vec<Option<NxtTerrainTile>>,
    /// Heights (fine, classic y: down positive) per cell, from the level's
    /// height codes.
    heights: Vec<f32>,
    /// The classic window's closed tile rectangle in this grid's
    /// coordinates `[x0, z0, x1, z1]` (a far square's build): a vertex on it
    /// takes its normal as the near terrain does, from the heights clamped
    /// into the window's grid (`[x0 - 1, x1]`), so both sides of the seam
    /// agree. `None` (the near terrain): no clamp beyond the grid's own.
    near_box: Option<[i32; 4]>,
}

impl LevelGrid {
    fn index(&self, x: i32, z: i32) -> Option<usize> {
        let (w, h) = (self.tiles[0] as i32 + 2, self.tiles[1] as i32 + 2);
        let (ix, iz) = (x + 1, z + 1);
        (ix >= 0 && iz >= 0 && ix < w && iz < h).then(|| (ix * h + iz) as usize)
    }

    /// The height at grid vertex `(x, z)`, clamped into the grid.
    fn height(&self, x: i32, z: i32) -> f32 {
        let x = x.clamp(-1, self.tiles[0] as i32);
        let z = z.clamp(-1, self.tiles[1] as i32);
        self.index(x, z).map_or(0.0, |i| self.heights[i])
    }

    /// The vertex normal: the central differences of the height grid,
    /// `(h(x-1) - h(x+1), 2 unit, h(z-1) - h(z+1))` with y up, here in
    /// classic axes (y down).
    fn normal(&self, x: i32, z: i32) -> [f32; 3] {
        let (xs, zs) = match self.near_box {
            Some([x0, z0, x1, z1]) if (x0..=x1).contains(&x) && (z0..=z1).contains(&z) => {
                ((x0 - 1, x1), (z0 - 1, z1))
            }
            _ => ((i32::MIN, i32::MAX), (i32::MIN, i32::MAX)),
        };
        let dx =
            self.height((x + 1).clamp(xs.0, xs.1), z) - self.height((x - 1).clamp(xs.0, xs.1), z);
        let dz =
            self.height(x, (z + 1).clamp(zs.0, zs.1)) - self.height(x, (z - 1).clamp(zs.0, zs.1));
        let n = glam::Vec3::new(dx, -2.0 * GRID_SIZE, dz).normalize_or(glam::Vec3::NEG_Y);
        n.to_array()
    }
}

/// A tile's sources and shape: own source, srcA/srcB, shape and rotation.
#[derive(Clone, Copy, Debug)]
struct TileSources {
    own: Source,
    a: Source,
    b: Source,
    shape: u8,
    rotation: u8,
    /// The underlay id (the plain diagonal's neighbour test).
    underlay: Option<u16>,
}

impl TileSources {
    fn source(&self, part: Part) -> Source {
        match part {
            Part::Own => self.own,
            Part::A => self.a,
            Part::B => self.b,
        }
    }

    /// The centre source: the
    /// highest-priority blending source of srcA/srcB, else empty.
    fn centre_source(&self) -> Source {
        let mut best: Option<Source> = None;
        for s in [self.a, self.b] {
            if s.blend && best.is_none_or(|b| s.priority > b.priority) {
                best = Some(s);
            }
        }
        best.unwrap_or(Source::EMPTY)
    }

    /// The tile's triangles (tile-local, rotated), with their parts.
    fn triangles(&self) -> Vec<(Part, [Point; 3])> {
        let Some(tris) = shape_triangles(self.shape) else {
            return Vec::new();
        };
        tris.iter()
            .map(|&(part, pts)| (part, pts.map(|p| rotate(p, self.rotation))))
            .collect()
    }

    /// The dominant source: the
    /// source that owns tile corner `corner` (a tile-local corner point):
    /// among the drawn triangles touching it, a blending one first, then
    /// the highest priority.
    fn dominant(&self, corner: Point) -> Option<Source> {
        let mut best: Option<Source> = None;
        for (part, pts) in self.triangles() {
            let s = self.source(part);
            if !s.draws() || !pts.contains(&corner) {
                continue;
            }
            let better = best.is_none_or(|b| (s.blend, s.priority) > (b.blend, b.priority));
            if better {
                best = Some(s);
            }
        }
        best
    }
}

/// The builder's view of one level: the grid, the tiles' sources and the
/// normals.
struct LevelBuild<'a> {
    grid: &'a LevelGrid,
    sources: Vec<Option<TileSources>>,
    normals: Vec<[f32; 3]>,
}

impl LevelBuild<'_> {
    fn tile(&self, x: i32, z: i32) -> Option<&TileSources> {
        self.grid.index(x, z).and_then(|i| self.sources[i].as_ref())
    }

    fn normal(&self, x: i32, z: i32) -> [f32; 3] {
        let x = x.clamp(-1, self.grid.tiles[0] as i32);
        let z = z.clamp(-1, self.grid.tiles[1] as i32);
        self.grid
            .index(x, z)
            .map_or([0.0, -1.0, 0.0], |i| self.normals[i])
    }

    /// The shared vertex: a corner vertex of a blending
    /// source takes the highest-priority blending source that owns the
    /// grid point in the four tiles around it (the tile to its north-east
    /// first, `>=`, then the others, `>`); a non-blending source keeps its
    /// own.
    fn shared_source(&self, x: i32, z: i32, source: Source) -> Source {
        if !source.blend {
            return source;
        }
        let mut best = source;
        let around = [
            (x, z, SW, true),
            (x - 1, z - 1, NE, false),
            (x, z - 1, NW, false),
            (x - 1, z, SE, false),
        ];
        for (tx, tz, corner, first) in around {
            let Some(c) = self.tile(tx, tz).and_then(|t| t.dominant(corner)) else {
                continue;
            };
            let wins = if first {
                c.priority >= best.priority
            } else {
                c.priority > best.priority
            };
            if c.blend && wins {
                best = c;
            }
        }
        best
    }
}

/// The PLAIN-tile diagonal: `1` splits the tile along
/// SW-NE, `0` along SE-NW; chosen by which diagonal's corners agree more
/// on the neighbours' underlay, ties by the flatter diagonal.
fn plain_diagonal(grid: &LevelGrid, sources: &[Option<TileSources>], x: i32, z: i32) -> u8 {
    let under = |dx: i32, dz: i32| {
        grid.index(x + dx, z + dz)
            .and_then(|i| sources[i].as_ref())
            .and_then(|t| t.underlay)
    };
    let own = under(0, 0);
    let same = |dx: i32, dz: i32| -> i32 {
        if under(dx, dz) == own {
            1
        } else {
            -1
        }
    };
    let sw = same(-1, -1) + same(0, -1) + same(-1, 0);
    let ne = same(1, 1) + same(0, 1) + same(1, 0);
    let se = same(1, -1) + same(0, -1) + same(1, 0);
    let nw = same(-1, 1) + same(0, 1) + same(-1, 0);
    let (mut d1, mut d2) = ((sw - ne).abs() as f32, (se - nw).abs() as f32);
    if d1 == d2 {
        d1 = (grid.height(x, z) - grid.height(x + 1, z + 1)).abs();
        d2 = (grid.height(x + 1, z) - grid.height(x, z + 1)).abs();
    }
    u8::from(d1 < d2)
}

/// The terrain's lightness factor over 127 (`74 / 127`, the classic
/// floor's unshadowed `74 - 0`).
const TERRAIN_LIGHTNESS: u32 = 74;

// The height-code decode and the tile heights live in the far scene's
// crate with the square decode, which the near and far terrains share.
pub use rs910_far_scene::far_terrain::{height_offset, tile_height_up, SquareTerrain};

/// An HSL16 colour as the modern client's terrain colours it: the
/// lightness scaled by `74 / 127` (the classic floor scales it by
/// `(74 - shadow) / 128`), then the HSL table (the modern client uses a
/// per-session gamma of `0.7 +- 0.015`; the fixed classic 0.7 here), then
/// the material's colour offsets (towards near-white (254 per channel) by the grey blend,
/// times `(256 + boost) / 256`, the grey target `2 * 74` per channel; the
/// classic floor applies the same material terms). Display-referred RGBA.
#[must_use]
pub fn terrain_colour(hsl: u16, material: Option<&crate::texture::Material>) -> [u8; 4] {
    let shade: u32 = TERRAIN_LIGHTNESS;
    let l = u32::from(hsl & 0x7F) * shade / 127;
    let hsl = (hsl & 0xFF80) | l as u16;
    let rgb = rs910_core::colour::hsl_tables().rgb[usize::from(hsl)];
    let mut c = [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF].map(|v| v as u32);
    if let Some(m) = material {
        let grey = if m.effect == 4 {
            0
        } else {
            u32::from(m.grey_blend)
        };
        if grey != 0 {
            let grey = grey.min(256);
            // The classic target: the shade's grey, twice the shade per channel;
            // 148 at the unshadowed shade 74, the modern client's
            // `0.580392`.
            c = c.map(|v| (grey * 2 * shade + (256 - grey) * v) >> 8);
        }
        let boost = u32::from(m.brightness_boost);
        if boost != 0 {
            c = c.map(|v| ((v * (256 + boost)) >> 8).min(255));
        }
    }
    [c[0] as u8, c[1] as u8, c[2] as u8, 255]
}

fn material_of(materials: &MaterialStore, id: Option<u32>) -> Option<&crate::texture::Material> {
    id.and_then(|id| materials.get(id))
}

/// An underlay source: the tile's own pre-blended HSL colour (file 5's
/// `u16`: the classic 10 x 10 underlay blend over NXT's ids; the underlay
/// type's own colour, opcode 1, is skipped), the underlay type's material
/// and scale (`g2 / 128`, the classic material scale / 512), blending,
/// priority 0.
fn underlay_source(
    flo: &FloStore,
    materials: &MaterialStore,
    id: u16,
    colour: u16,
) -> Option<Source> {
    let u = flo.get_underlay(u32::from(id))?;
    Some(Source {
        material: u.texture.map_or(-1, |t| t as i32),
        scale: u.material_scale as f32 / 512.0,
        colour: terrain_colour(colour, material_of(materials, u.texture)),
        blend: true,
        priority: 0,
        hsl: colour,
    })
}

/// An overlay source: the overlay type's colour (none for the magenta "no
/// colour"; the average colour is the low-detail colour only), material,
/// scale, blend flag (opcode 12) and raw priority (opcode 11; the classic
/// floor folds the id in).
fn overlay_source(flo: &FloStore, materials: &MaterialStore, id: u16) -> Option<Source> {
    let o = flo.get_overlay(u32::from(id))?;
    Some(Source {
        material: o.texture.map_or(-1, |t| t as i32),
        scale: o.material_scale as f32 / 512.0,
        colour: if o.rgb >= 0 {
            terrain_colour(o.rgb as u16, material_of(materials, o.texture))
        } else {
            [0; 4]
        },
        blend: o.blend,
        priority: (o.priority >> 8) as i32,
        hsl: if o.rgb >= 0 { o.rgb as u16 } else { 0 },
    })
}

/// The sources of one decoded tile: no triangles for a height-only tile or one naming an
/// id the configs lack; the underlay alone (plain); the overlay alone (a
/// plain overlay); else the overlay part (srcA) and the underlay part
/// (srcB) of its shape. A shaped water tile with both an underlay and a
/// LAND underlay (the id after its shape byte) takes the LAND underlay as
/// its srcA and the bed underlay as its srcB (the file-4 overlay is not
/// drawn), as the modern client does.
fn tile_sources(
    flo: &FloStore,
    materials: &MaterialStore,
    t: &NxtTerrainTile,
) -> Option<TileSources> {
    if t.flags == 0 {
        return None;
    }
    let colour = t.underlay_u16.unwrap_or(0);
    let under = match t.underlay {
        Some(id) => Some(underlay_source(flo, materials, id, colour)?),
        None => None,
    };
    let land_under = match t.water_underlay {
        Some(id) => Some(underlay_source(flo, materials, id, colour)?),
        None => None,
    };
    let over = match t.overlay {
        Some(id) => Some(overlay_source(flo, materials, id)?),
        None => None,
    };
    if let Some(id) = t.water_overlay {
        flo.get_overlay(u32::from(id))?;
    }
    let empty = Source::EMPTY;
    let (own, a, b, shape, rotation) = match (over, t.overlay_shape) {
        (Some(over), Some(byte)) if byte >> 2 == 0 => (over, empty, empty, 0, 0),
        (Some(over), Some(byte)) => {
            let under = under.unwrap_or(empty);
            let a = match land_under {
                Some(land) if t.underlay.is_some() => land,
                _ => over,
            };
            (empty, a, under, byte >> 2, byte & 3)
        }
        _ => (under.unwrap_or(empty), empty, empty, 0, 0),
    };
    Some(TileSources {
        own,
        a,
        b,
        shape,
        rotation,
        underlay: t.underlay,
    })
}

/// Decode map file 5 of every square under the scene (plus a one-tile
/// margin).
fn load_squares(
    pack: &crate::cache::Pack,
    base: [i32; 2],
    tiles: [usize; 2],
) -> std::collections::HashMap<(i32, i32), SquareTerrain> {
    let mut out = std::collections::HashMap::new();
    let lo = [(base[0] - 1).div_euclid(64), (base[1] - 1).div_euclid(64)];
    let hi = [
        (base[0] + tiles[0] as i32 + 1).div_euclid(64),
        (base[1] + tiles[1] as i32 + 1).div_euclid(64),
    ];
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
            // The decode the far squares share.
            let Some(square) = files
                .get(&TERRAIN_FILE)
                .and_then(|b| rs910_far_scene::far_terrain::decode_square(group, b))
            else {
                continue;
            };
            out.insert((sx, sz), square);
        }
    }
    out
}

/// The material table entry of a layer (`TerrainMaterial.params`:
/// specular power, specular mask, textured, the property byte). The
/// property nibbles stay neutral: this builder bakes the material's grey
/// blend and brightness into the vertex colour ([`terrain_colour`]); the
/// modern client instead passes them to the shader. One or the other.
pub(crate) fn layer_params(
    material: Option<&crate::texture::Material>,
    textured: bool,
) -> [f32; 4] {
    let info = crate::models::materials::material_info(material);
    let mask = info.flags & crate::models::materials::FLAG_ALPHA_IS_MASK != 0;
    [
        info.spec_power,
        if mask { 1.0 } else { 0.0 },
        if textured { 1.0 } else { 0.0 },
        // An effect-6 (classic unlit) layer.
        if crate::atmosphere::fog::material_flags(material) != 0 {
            crate::atmosphere::fog::TERRAIN_UNLIT
        } else {
            0.0
        },
    ]
}

/// Emit triangles in the order the forward pipeline treats as front
/// facing (seen from above in classic axes): the shape tables are
/// counter-clockwise with x east and z north.
const REVERSE_WINDING: bool = false;

impl TerrainScene {
    /// Build the terrain of the snapshot's installed scene from map file 5
    /// (see the module docs). Not usable (the classic floor stays) without
    /// floors, when the scene's squares have no file 5, or when the
    /// scene's ground does not match file 5's (an instanced scene built
    /// from other squares: fewer than 90% of the level-0 tiles the classic
    /// floor draws a triangle on have terrain).
    #[must_use]
    pub fn build(
        pack: &crate::cache::Pack,
        materials: &MaterialStore,
        snapshot: &SceneSnapshot<'_>,
    ) -> Self {
        Self::build_with(pack, materials, snapshot, |ids| {
            ids.iter()
                .map(|&id| layer_of(pack, materials, id))
                .collect()
        })
    }

    /// [`Self::build`] with the layers' texels made by `layers` from the
    /// layer materials in order ([`layer_of`] each; the renderer runs them
    /// on its threads, performance plan P5).
    #[must_use]
    pub fn build_with(
        pack: &crate::cache::Pack,
        materials: &MaterialStore,
        snapshot: &SceneSnapshot<'_>,
        layers_of: impl FnOnce(&[i32]) -> Vec<Option<atlas::LayerLevels>>,
    ) -> Self {
        let mut scene = Self {
            base: snapshot.floor_base,
            ..Self::default()
        };
        let Some(Some(first)) = snapshot.floors.first() else {
            scene.reason = "no floor".into();
            return scene;
        };
        let tiles = [first.tiles_x, first.tiles_z];
        scene.tiles = tiles;
        let flo = match FloStore::load(pack) {
            Ok(flo) => flo,
            Err(err) => {
                scene.reason = format!("flo configs: {err:#}");
                return scene;
            }
        };
        let squares = load_squares(pack, scene.base, tiles);
        scene.stats.squares = squares.len();
        if squares.is_empty() {
            scene.reason = "no map file 5 under the scene".into();
            return scene;
        }
        let mut layers: std::collections::HashMap<i32, u16> = std::collections::HashMap::new();
        for (level, floor) in snapshot.floors.iter().enumerate() {
            let Some(floor) = floor else {
                scene.levels.push(None);
                continue;
            };
            if level >= 4 || [floor.tiles_x, floor.tiles_z] != tiles {
                scene.levels.push(None);
                continue;
            }
            let grid = level_grid(&squares, scene.base, tiles, level);
            let mesh = build_level(
                &grid,
                &flo,
                materials,
                level,
                &mut |material| {
                    *layers.entry(material).or_insert_with(|| {
                        let id = scene.layer_materials.len() as u16;
                        scene.layer_materials.push(material);
                        id
                    })
                },
                &mut scene.stats,
            );
            scene.levels.push(Some(mesh));
        }
        // The layers: each material's texels (untextured: white).
        let texels = layers_of(&scene.layer_materials);
        for (&material, texels) in scene.layer_materials.iter().zip(texels) {
            let m = u32::try_from(material)
                .ok()
                .and_then(|id| materials.get(id));
            scene.layer_params.push(layer_params(m, texels.is_some()));
            scene.layer_texels.push(texels.unwrap_or_default());
        }
        // Does the scene's ground come from these squares?
        let (mut classic, mut covered) = (0, 0);
        if let Some(Some(mesh)) = scene.levels.first() {
            for z in 0..tiles[1] {
                for x in 0..tiles[0] {
                    let tile = z * tiles[0] + x;
                    // A tile with a drawn classic triangle (one some batch
                    // owns; an overlay without a colour has none).
                    if first
                        .batches
                        .iter()
                        .any(|b| b.tri_mask.get(tile).is_some_and(|&m| m != 0))
                    {
                        classic += 1;
                        covered += usize::from(mesh.has_tile(x, z));
                    }
                }
            }
        }
        scene.usable = classic > 0 && covered * 10 >= classic * 9;
        scene.reason = format!("{covered} of {classic} classic level-0 floor tiles have terrain");
        scene
    }
}

/// The texels of layer material `material` ([`atlas::layer_texels`];
/// `None`: untextured).
#[must_use]
pub fn layer_of(
    pack: &crate::cache::Pack,
    materials: &MaterialStore,
    material: i32,
) -> Option<atlas::LayerLevels> {
    let m = u32::try_from(material)
        .ok()
        .and_then(|id| materials.get(id))?;
    atlas::layer_texels(pack, m).ok()
}

/// The NXT tiles and heights of scene level `level` (with the margin).
fn level_grid(
    squares: &std::collections::HashMap<(i32, i32), SquareTerrain>,
    base: [i32; 2],
    tiles: [usize; 2],
    level: usize,
) -> LevelGrid {
    let (w, h) = (tiles[0] + 2, tiles[1] + 2);
    let mut grid = LevelGrid {
        tiles,
        cells: vec![None; w * h],
        heights: vec![0.0; w * h],
        near_box: None,
    };
    for ix in 0..w {
        for iz in 0..h {
            let (wx, wz) = (base[0] + ix as i32 - 1, base[1] + iz as i32 - 1);
            let key = (wx.div_euclid(64), wz.div_euclid(64));
            let Some(Some((cells, heights))) = squares.get(&key).map(|s| &s.levels[level]) else {
                continue;
            };
            let (lx, lz) = (
                wx.rem_euclid(64) as usize + 1,
                wz.rem_euclid(64) as usize + 1,
            );
            let i = lx * TERRAIN_SIDE + lz;
            grid.cells[ix * h + iz] = Some(cells[i]);
            // Classic axes: y down.
            grid.heights[ix * h + iz] = -heights[i];
        }
    }
    grid
}

/// The mesh of one level (see the module docs).
fn build_level(
    grid: &LevelGrid,
    flo: &FloStore,
    materials: &MaterialStore,
    level: usize,
    layer: &mut dyn FnMut(i32) -> u16,
    stats: &mut BuildStats,
) -> LevelMesh {
    let (w, h) = (grid.tiles[0] + 2, grid.tiles[1] + 2);
    let mut sources: Vec<Option<TileSources>> = grid
        .cells
        .iter()
        .map(|c| c.as_ref().and_then(|t| tile_sources(flo, materials, t)))
        .collect();
    // The plain tiles' diagonals.
    for ix in 0..w {
        for iz in 0..h {
            let i = ix * h + iz;
            if sources[i].is_some_and(|t| t.shape == 0) {
                let d = plain_diagonal(grid, &sources, ix as i32 - 1, iz as i32 - 1);
                if let Some(t) = sources[i].as_mut() {
                    t.rotation = d;
                }
            }
        }
    }
    let normals = (0..w * h)
        .map(|i| grid.normal((i / h) as i32 - 1, (i % h) as i32 - 1))
        .collect();
    let b = LevelBuild {
        grid,
        sources,
        normals,
    };
    let mut mesh = LevelMesh {
        level,
        tiles: grid.tiles,
        ..LevelMesh::default()
    };
    mesh.tile_ranges = vec![(0, 0); grid.tiles[0] * grid.tiles[1]];
    for x in 0..=grid.tiles[0] as i32 {
        for z in 0..=grid.tiles[1] as i32 {
            let water = grid
                .index(x, z)
                .and_then(|i| grid.cells[i])
                .is_some_and(|t| t.is_water());
            mesh.grid.push((grid.height(x, z), water));
        }
    }
    let mut slot_of = |material: i32| {
        if material < 0 {
            0xFFFF
        } else {
            layer(material)
        }
    };
    for z in 0..grid.tiles[1] as i32 {
        for x in 0..grid.tiles[0] as i32 {
            let Some(tile) = b.tile(x, z).copied() else {
                continue;
            };
            let start = mesh.indices.len() as u32;
            stats.tiles += 1;
            for (part, pts) in tile.triangles() {
                let src = tile.source(part);
                if !src.draws() {
                    continue;
                }
                let centre = if src.blend { tile.centre_source() } else { src };
                let corners = pts.map(|p| {
                    let corner = (p[0] == 0.0 || p[0] == 1.0) && (p[1] == 0.0 || p[1] == 1.0);
                    let (gx, gz) = (x + p[0] as i32, z + p[1] as i32);
                    if corner {
                        let s = b.shared_source(gx, gz, src);
                        let pos = [
                            gx as f32 * GRID_SIZE,
                            grid.height(gx, gz),
                            gz as f32 * GRID_SIZE,
                        ];
                        let colour = s.colour;
                        (pos, b.normal(gx, gz), s, colour)
                    } else {
                        // The centre vertex: bilinear over
                        // the tile's corners.
                        let (u, v) = (p[0], p[1]);
                        let wts = [(1.0 - u) * (1.0 - v), u * (1.0 - v), (1.0 - u) * v, u * v];
                        let at = [(x, z), (x + 1, z), (x, z + 1), (x + 1, z + 1)];
                        let mut y = 0.0;
                        let mut n = [0.0_f32; 3];
                        for (wt, (cx, cz)) in wts.iter().zip(at) {
                            y += wt * grid.height(cx, cz);
                            let cn = b.normal(cx, cz);
                            for k in 0..3 {
                                n[k] += wt * cn[k];
                            }
                        }
                        let pos = [(x as f32 + u) * GRID_SIZE, y, (z as f32 + v) * GRID_SIZE];
                        (pos, n, centre, centre.colour)
                    }
                });
                let slots = corners.map(|(_, _, s, _)| (slot_of(s.material), s.scale));
                let blended = slots.iter().any(|s| s.0 != slots[0].0);
                let textured = slots.iter().any(|s| s.0 != 0xFFFF);
                stats.triangles += 1;
                stats.blended_triangles += usize::from(blended);
                stats.textured_triangles += usize::from(textured);
                let order: [usize; 3] = if REVERSE_WINDING {
                    [2, 1, 0]
                } else {
                    [0, 1, 2]
                };
                for k in order {
                    let (pos, normal, _, colour) = corners[k];
                    let mut weight = [0_u8; 4];
                    weight[k] = 255;
                    weight[3] = level as u8;
                    mesh.indices.push(mesh.vertices.len() as u32);
                    mesh.vertices.push(TerrainVertex {
                        pos,
                        normal,
                        colour,
                        slots: [slots[0].0, slots[1].0, slots[2].0, 0],
                        scale: [slots[0].1, slots[1].1, slots[2].1, 0.0],
                        weight,
                    });
                }
            }
            let count = mesh.indices.len() as u32 - start;
            mesh.tile_ranges[z as usize * grid.tiles[0] + x as usize] = (start, count);
        }
    }
    stats.vertices += mesh.vertices.len();
    mesh
}

// ---------------------------------------------------------------------------
// One map square's terrain
// ---------------------------------------------------------------------------

/// Map square `square`'s terrain for the far scene (lane Q-FAR1,
/// `nxt-render-distance.md` §6.2 "Terrain"): the near terrain's builder over
/// the square's 64 x 64 tiles, built with one more tile of its neighbours on
/// each side (`squares`: the decoded square and its eight neighbours) and
/// then cropped, so a vertex on the square's edge sees the four tiles
/// around it and the height grid's central differences, as an interior
/// vertex does. Every vertex takes the height, blending source, colour and
/// material the near terrain gives the same grid point (one decode, one
/// builder); colours at the fixed shade 74 (no classic shade map outside
/// the window). Positions are square-local (tile `(0, 0)` of the square at
/// the origin, classic y); tiles `z * 64 + x`; per level 0..4, `None` where a
/// level has no triangles. `layer` numbers the materials (the far scene's
/// own texture array). `window` (absolute tiles `[x0, z0, x1, z1)`, the
/// classic window): the vertices on its boundary take the near terrain's
/// normals there (the near grid is clamped at its east and north edges).
pub fn build_square(
    squares: &std::collections::HashMap<(i32, i32), SquareTerrain>,
    square: (i32, i32),
    flo: &FloStore,
    materials: &MaterialStore,
    window: Option<[i32; 4]>,
    layer: &mut dyn FnMut(i32) -> u16,
) -> Vec<Option<LevelMesh>> {
    const N: usize = 64;
    let base = [square.0 * 64 - 1, square.1 * 64 - 1];
    let tiles = [N + 2, N + 2];
    let mut stats = BuildStats::default();
    (0..4)
        .map(|level| {
            squares.get(&square)?.levels[level].as_ref()?;
            let mut grid = level_grid(squares, base, tiles, level);
            grid.near_box = window
                .map(|[x0, z0, x1, z1]| [x0 - base[0], z0 - base[1], x1 - base[0], z1 - base[1]]);
            let full = build_level(&grid, flo, materials, level, layer, &mut stats);
            let mesh = crop_square(&full, N);
            (!mesh.indices.is_empty()).then_some(mesh)
        })
        .collect()
}

/// The inner `n x n` tiles of `full` (built with a one-tile ring around
/// them), moved to the inner tiles' origin.
fn crop_square(full: &LevelMesh, n: usize) -> LevelMesh {
    let w = full.tiles[0];
    let mut out = LevelMesh {
        level: full.level,
        tiles: [n, n],
        tile_ranges: vec![(0, 0); n * n],
        ..LevelMesh::default()
    };
    for z in 0..n {
        for x in 0..n {
            let (start, count) = full.tile_ranges[(z + 1) * w + x + 1];
            let first = out.indices.len() as u32;
            for &i in &full.indices[start as usize..(start + count) as usize] {
                let mut v = full.vertices[i as usize];
                v.pos[0] -= GRID_SIZE;
                v.pos[2] -= GRID_SIZE;
                out.indices.push(out.vertices.len() as u32);
                out.vertices.push(v);
            }
            out.tile_ranges[z * n + x] = (first, out.indices.len() as u32 - first);
        }
    }
    let hz = full.tiles[1] + 1;
    for x in 0..=n {
        for z in 0..=n {
            out.grid.push(full.grid[(x + 1) * hz + z + 1]);
        }
    }
    out
}
