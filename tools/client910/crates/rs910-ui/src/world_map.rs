//! The world-map area renderer: the tile-shape masks, the map defaults, the
//! decoded area (ground tiles, upper levels, locs and elements), the
//! chunk-sprite caches and the chunk painter.
//!
//! One [`WorldMap`] value owns the area-independent state and the loaded
//! [`Area`]. The loading steps, the view, element drawing, flashes and input
//! live in `world_map_client.rs`.
//!
//! Tile arrays are indexed `size_x * z + x` relative to [`Area::origin`], and
//! a chunk sprite's row 0 is its top (highest z) edge: `fill` writes row
//! `size - z - 1`.

#[cfg(test)]
use chunk_painter::{column, fill, row, shaped, ShapedOverlay};

mod tile_shapes;
pub use tile_shapes::{shape_rotation, shape_table, tile_shapes};
mod area;
pub use area::{
    display_to_source, source_to_display, source_to_display_any_level, Element, StaticElements,
    TileLocs, UpperTile,
};
mod chunk_cache;
pub use chunk_cache::{ChunkCache, CHUNK_IDLE_CLEANS};
mod area_tiles;
pub use area_tiles::{blend, overlay_colour, Area};
mod map_scenes;
pub use map_scenes::MapScenes;
mod chunk_painter;
use chunk_painter::ChunkLayers;
pub use chunk_painter::ChunkPainter;

use crate::{
    cache::Pack, config::LocStore, minimap::MsiStore, ui_bytes::Cursor, ui_sprites::Sprite,
};

use anyhow::{Context, Result};

use std::rc::Rc;

/// Compass directions (serial ids) that index the eight neighbour flags of a
/// shaded 8x8 block.
pub const NORTH: usize = 0;

pub const NORTHWEST: usize = 1;

pub const NORTHEAST: usize = 2;

pub const SOUTHWEST: usize = 3;

pub const SOUTH: usize = 4;

pub const WEST: usize = 5;

pub const EAST: usize = 6;

pub const SOUTHEAST: usize = 7;

/// Loc shape ids of the four wall shapes the tile painter draws.
const WALL_STRAIGHT: i32 = 0;

const WALL_L: i32 = 2;

const WALL_SQUARE_CORNER: i32 = 3;

const WALL_DIAGONAL: i32 = 9;

/// The corner template of a shaded block: 1 is shade, 2 is border.
const CORNER_TEMPLATE: [[i32; 6]; 6] = [
    [2, 2, 0, 0, 0, 0],
    [2, 2, 2, 0, 0, 0],
    [1, 2, 2, 2, 0, 0],
    [1, 1, 1, 2, 2, 0],
    [1, 1, 1, 2, 2, 2],
    [1, 1, 1, 1, 2, 2],
];

/// The time slice of an incremental area decode, in milliseconds.
const DECODE_SLICE_MS: u64 = 5;

/// The per-frame pixel budget for re-rendering chunks whose cached sprite has
/// a stale size.
const RERENDER_BUDGET: i32 = 262_144;

/// The world-map defaults (group 10 of the defaults archive).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Defaults {
    /// Opcode 1: the map whose non-members areas are shaded.
    pub members_map: i32,
    /// Opcode 2: the ARGB shade over inaccessible blocks.
    pub shade: i32,
    /// Opcode 3: the shaded-area border colour.
    pub border: i32,
    /// Opcode 4: border width.
    pub border_width: i32,
    /// Opcode 5: the corner template size.
    pub corner: i32,
    /// Opcode 6: the default map (the fallback when the player is in no
    /// area) whose tile file stays cached.
    pub default_map: i32,
    /// Opcode 7: the element label shadow colour.
    pub text_shadow: i32,
    /// `fonts[textSize][zoom]` (opcodes 100+): label font ids.
    pub fonts: [[i32; 5]; 3],
}

impl Defaults {
    /// Decodes the defaults record.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let mut d = Self::default();
        loop {
            let op = c.g1()?;
            match op {
                0 => return Ok(d),
                1 => d.members_map = c.g4s()?,
                2 => d.shade = c.g4s()?,
                3 => d.border = c.g4s()?,
                4 => d.border_width = i32::from(c.g1()?),
                5 => d.corner = i32::from(c.g1()?),
                6 => d.default_map = c.g4s()?,
                7 => d.text_shadow = c.g4s()?,
                op if op >= 100 => {
                    let v = usize::from(op - 100);
                    let value = i32::from(c.g2()?);
                    *d.fonts
                        .get_mut(v & 7)
                        .and_then(|row| row.get_mut(v >> 3))
                        .context("world-map defaults font slot")? = value;
                }
                // Opcodes 8-99 are ignored.
                _ => {}
            }
        }
    }
    /// Loads defaults group 10 from the pack.
    pub fn load(pack: &Pack) -> Result<Self> {
        let bytes = crate::js5_fetch::fetch_file(pack, "defaults", 10)?
            .context("world-map defaults (defaults group 10) missing")?;
        Self::decode(&bytes)
    }
}

/// `ceil` to `i32` for the positive chunk sizes of the chunk draw.
fn ceil_i32(v: f32) -> i32 {
    f64::from(v).ceil() as i32
}

/// The draw view, relative to the area origin: the tile window and the
/// screen rectangle it maps to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// The visible tile window: left, top, right and bottom edges.
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    /// The screen edges the tile window maps to.
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl View {
    /// The 16.16 fixed-point pixels-per-tile scales `[x, y]`.
    pub fn scale(&self) -> [i32; 2] {
        let w = self.right - self.left;
        let h = self.top - self.bottom;
        [
            if w == 0 {
                0
            } else {
                ((self.x1 - self.x0) << 16) / w
            },
            if h == 0 {
                0
            } else {
                ((self.y1 - self.y0) << 16) / h
            },
        ]
    }
    /// An element's screen point.
    pub fn project(&self, x: i32, z: i32) -> [i32; 2] {
        let [sx, sz] = self.scale();
        [
            ((x - self.left).wrapping_mul(sx) >> 16) + self.x0,
            self.y1 - ((z - self.bottom).wrapping_mul(sz) >> 16),
        ]
    }
    /// Exact-division projection of a polygon vertex.
    pub fn project_exact(&self, x: i32, z: i32) -> [i32; 2] {
        let w = (self.right - self.left).max(1);
        let h = (self.top - self.bottom).max(1);
        [
            (self.x1 - self.x0).wrapping_mul(x - self.left) / w + self.x0,
            self.y1 - (self.y1 - self.y0).wrapping_mul(z - self.bottom) / h,
        ]
    }
    /// The `(x1 - x0) / (right - left)` pixel-per-tile ratio as its integer
    /// terms `[screen width, tiles across, screen height, tiles up]`.
    pub fn span(&self) -> [i32; 4] {
        [
            self.x1 - self.x0,
            self.right - self.left,
            self.y1 - self.y0,
            self.top - self.bottom,
        ]
    }
}

/// The resumable area-decode state: the tile file and its read position
/// (kept between slices) and the underlay and overlay palettes.
struct Decoder {
    bytes: Rc<Vec<u8>>,
    pos: usize,
    underlays: Vec<i32>,
    overlays: Vec<i32>,
}

/// The area-independent world-map state plus the loaded [`Area`].
pub struct WorldMap {
    pub pack: Option<Pack>,
    pub defaults: Defaults,
    /// The area metadata in the order the 16-bucket area table iterates
    /// (bucket `id & 15`, then file order).
    pub areas: Vec<crate::minimap::AreaMetadata>,
    /// The current area (an index into [`Self::areas`]).
    pub current: Option<usize>,
    /// Whether the current area is the members-shaded map.
    pub members_map: bool,
    pub area: Option<Area>,
    /// The drawn zoom and its target.
    pub zoom: f32,
    pub target_zoom: f32,
    /// The tile-shape mask texel size and the masks themselves.
    pub shape_size: i32,
    pub shapes: Vec<[Vec<u8>; 4]>,
    /// The per-load hue and lightness jitter.
    pub hue_jitter: i32,
    pub lightness_jitter: i32,
    /// The overlay colour per overlay id.
    pub overlay_colours: Vec<i32>,
    decoder: Option<Decoder>,
    /// True once the tile file is consumed (the blend and element listing
    /// may still be pending on a later slice).
    pub decoded: bool,
    /// The default map's retained tile file.
    default_file: Option<Rc<Vec<u8>>>,
    /// Decode incrementally (a player-driven map switch).
    pub incremental: bool,
    /// The draw view.
    pub view: View,
    /// The main and overview chunk-sprite caches; `overview_active` selects
    /// the overview one.
    pub main_cache: ChunkCache,
    pub overview_cache: ChunkCache,
    pub overview_active: bool,
    /// The area's static elements.
    pub static_elements: Option<StaticElements>,
    /// Config owners, filled by [`Self::install`].
    pub flo: Option<crate::flo::FloStore>,
    pub materials: Option<crate::texture::MaterialStore>,
    pub locs: Option<Rc<LocStore>>,
    pub scenes: MapScenes,
    /// The HSV-to-RGB palette table.
    pub palette: Vec<i32>,
    /// The last shade input and output.
    shade_cache: (i32, i32),
    /// The background sprite in use, cached at key -1.
    background: Option<Rc<Sprite>>,
}

impl Default for WorldMap {
    fn default() -> Self {
        Self {
            pack: None,
            defaults: Defaults::default(),
            areas: Vec::new(),
            current: None,
            members_map: true,
            area: None,
            zoom: 0.0,
            target_zoom: 0.0,
            shape_size: 0,
            shapes: Vec::new(),
            // Initial jitter: -5..=5 hue and -8..=8 lightness.
            hue_jitter: (crate::ui_cam2::random_unit() * 11.0) as i32 - 5,
            lightness_jitter: (crate::ui_cam2::random_unit() * 17.0) as i32 - 8,
            overlay_colours: Vec::new(),
            decoder: None,
            decoded: false,
            default_file: None,
            incremental: true,
            view: View::default(),
            main_cache: ChunkCache::default(),
            overview_cache: ChunkCache::default(),
            overview_active: false,
            static_elements: None,
            flo: None,
            materials: None,
            locs: None,
            scenes: MapScenes::default(),
            palette: Vec::new(),
            shade_cache: (0, 0),
            background: None,
        }
    }
}

impl WorldMap {
    /// Loads the area metadata of group 0 (in table order: bucket `id & 15`,
    /// then file order) and the config lists.
    pub fn install(&mut self, pack: &Pack) {
        self.pack = Some(pack.clone());
        match crate::minimap::load_area_metadata(pack) {
            Ok(mut areas) => {
                areas.sort_by_key(|a| a.id & 15);
                self.areas = areas;
            }
            Err(error) => log::warn!("[client910] world-map areas unavailable: {error:#}"),
        }
        match Defaults::load(pack) {
            Ok(d) => self.defaults = d,
            Err(error) => log::warn!("[client910] world-map defaults unavailable: {error:#}"),
        }
        match crate::flo::FloStore::load(pack) {
            Ok(flo) => self.flo = Some(flo),
            Err(error) => log::warn!("[client910] world-map floor types unavailable: {error:#}"),
        }
        match crate::texture::MaterialStore::load(pack) {
            Ok(m) => self.materials = Some(m),
            Err(error) => log::warn!("[client910] world-map materials unavailable: {error:#}"),
        }
        match MsiStore::load(pack) {
            Ok(m) => self.scenes = MapScenes::new(m),
            Err(error) => log::warn!("[client910] world-map msi types unavailable: {error:#}"),
        }
        if self.locs.is_none() {
            match LocStore::load(pack) {
                Ok(l) => self.locs = Some(Rc::new(l)),
                Err(error) => log::warn!("[client910] world-map loc types unavailable: {error:#}"),
            }
        }
        // The HSV-to-RGB palette table.
        self.palette = crate::colour::build_hsv_table();
    }
    pub fn metadata(&self) -> Option<&crate::minimap::AreaMetadata> {
        self.areas.get(self.current?)
    }
    /// The index of the area with id `id`.
    pub fn by_id(&self, id: i32) -> Option<usize> {
        self.areas.iter().position(|a| a.id as i32 == id)
    }
    /// The index of the active area containing source tile `(x, z)`.
    pub fn map_at(&self, x: i32, z: i32) -> Option<usize> {
        self.areas.iter().position(|a| a.active && a.contains(x, z))
    }
    /// Selects the area with id `id`, unless it is already current.
    pub fn select_id(&mut self, id: i32) {
        if let Some(index) = self.by_id(id) {
            if self.current != Some(index) {
                self.select(index);
            }
        }
    }
    fn select(&mut self, index: usize) {
        self.current = Some(index);
        self.members_map = self.defaults.members_map == self.areas[index].id as i32;
    }
    /// Selects `index`; true when it changed the current area.
    pub fn select_area(&mut self, index: Option<usize>) -> bool {
        match index {
            Some(index) if self.current != Some(index) => {
                self.select(index);
                true
            }
            _ => false,
        }
    }
    /// Rebuilds the overlay colour table for the load's jitter.
    pub fn build_overlay_colours(&mut self, hue_offset: i32, lightness_offset: i32) {
        let Some(flo) = self.flo.as_ref() else {
            self.overlay_colours = vec![0];
            return;
        };
        let count = flo.overlays.keys().last().map_or(0, |id| id + 1) as usize;
        let mut out = vec![0; count + 1];
        for (i, slot) in out.iter_mut().enumerate().skip(1) {
            *slot = overlay_colour(
                flo,
                self.materials.as_ref(),
                (i - 1) as u32,
                hue_offset,
                lightness_offset,
                &self.palette,
            );
        }
        self.overlay_colours = out;
    }
    /// Allocates the area for the current map's chunk-aligned bounds.
    pub fn allocate(&mut self, origin: [i32; 2], size: [i32; 2]) {
        self.area = Some(Area::new(origin, size));
    }
    /// Drops the loaded area and any partial decode.
    pub fn release(&mut self) {
        self.area = None;
        self.decoder = None;
        self.decoded = false;
    }

    /// Decodes the current area's tile file into [`Self::area`] (within a 5 ms slice
    /// when `incremental`), then blend the levels and list the loc elements.
    /// Returns whether the decode finished.
    pub fn decode(
        &mut self,
        incremental: bool,
        read_var: &dyn Fn(bool, i32) -> i32,
    ) -> Result<bool> {
        let start = crate::logic_clock::now();
        let slice = std::time::Duration::from_millis(DECODE_SLICE_MS);
        if !self.decoded {
            if !self.decode_tiles(incremental, start, slice)? {
                return Ok(false);
            }
            self.decoded = true;
            self.blend_levels();
        }
        if incremental && crate::logic_clock::now().duration_since(start) >= slice {
            return Ok(false);
        }
        self.list_loc_elements(read_var);
        Ok(true)
    }

    /// The tile-record loop; true once the file is consumed (the read
    /// position is kept between slices).
    fn decode_tiles(
        &mut self,
        incremental: bool,
        start: std::time::Instant,
        slice: std::time::Duration,
    ) -> Result<bool> {
        let meta = self
            .metadata()
            .cloned()
            .context("world-map decode without a current map")?;
        if self.decoder.is_none() {
            let pack = self.pack.clone().context("world-map pack")?;
            let bytes = if self.defaults.default_map == meta.id as i32 {
                if self.default_file.is_none() {
                    self.default_file = Some(Rc::new(tile_file(&pack, &meta.name)?));
                }
                self.default_file.clone().unwrap()
            } else {
                Rc::new(tile_file(&pack, &meta.name)?)
            };
            let mut c = Cursor::new(&bytes);
            let underlays = (0..c.g1()?)
                .map(|_| c.gsmart1or2())
                .collect::<Result<Vec<_>>>()?;
            let overlays = (0..c.g1()?)
                .map(|_| c.gsmart1or2())
                .collect::<Result<Vec<_>>>()?;
            let pos = c.pos();
            self.decoder = Some(Decoder {
                bytes,
                pos,
                underlays,
                overlays,
            });
        }
        let decoder = self.decoder.as_mut().unwrap();
        let area = self
            .area
            .as_mut()
            .context("world-map decode without an allocated area")?;
        let bytes = decoder.bytes.clone();
        let mut c = Cursor::new(&bytes[decoder.pos..]);
        while c.remaining() > 0
            && (!incremental || crate::logic_clock::now().duration_since(start) < slice)
        {
            let (ox, oz) = (area.origin[0], area.origin[1]);
            if c.g1()? == 0 {
                let rx = i32::from(c.g1()?);
                let rz = i32::from(c.g1()?);
                for bx in 0..8 {
                    let bits = i32::from(c.g1()?);
                    let x8 = rx * 8 + bx - ox / 8;
                    for bz in 0..8 {
                        let z8 = rz * 8 + bz - oz / 8;
                        area.set_member_block(x8, z8, bits & 1 << bz != 0);
                    }
                }
                for x in 0..64 {
                    for z in 0..64 {
                        let lx = rx * 64 + x - ox;
                        let lz = rz * 64 + z - oz;
                        area.decode_tile(
                            &mut c,
                            [rx, rz],
                            lx,
                            lz,
                            &decoder.underlays,
                            &decoder.overlays,
                        )?;
                    }
                }
            } else {
                let rx = i32::from(c.g1()?);
                let rz = i32::from(c.g1()?);
                let bx = i32::from(c.g1()?);
                let bz = i32::from(c.g1()?);
                let member = c.g1()? != 0;
                area.set_member_block(rx * 8 + bx - ox / 8, rz * 8 + bz - oz / 8, member);
                for x in 0..8 {
                    for z in 0..8 {
                        let lx = rx * 64 + bx * 8 + x - ox;
                        let lz = rz * 64 + bz * 8 + z - oz;
                        area.decode_tile(
                            &mut c,
                            [rx, rz],
                            lx,
                            lz,
                            &decoder.underlays,
                            &decoder.overlays,
                        )?;
                    }
                }
            }
        }
        decoder.pos = bytes.len() - c.remaining();
        if c.remaining() > 0 {
            return Ok(false);
        }
        self.decoder = None;
        Ok(true)
    }

    /// Blends the three upper levels (into the shared output) and then the
    /// ground.
    fn blend_levels(&mut self) {
        let (Some(area), Some(flo)) = (self.area.as_mut(), self.flo.as_ref()) else {
            return;
        };
        if area.underlay.is_empty() {
            return;
        }
        let (hue, light) = (self.hue_jitter, self.lightness_jitter);
        let size = area.size;
        let w = size[0];
        let cz_count = area.chunks_z();
        let mut out = vec![0i32; area.colour.len()];
        for level in 0..3 {
            let mut ids = vec![0i16; area.colour.len()];
            for (at, chunk) in area.upper[level].iter().enumerate() {
                let Some(chunk) = chunk else { continue };
                let (cx, cz) = (at as i32 / cz_count, at as i32 % cz_count);
                for t in chunk.values() {
                    let i = ((cz * 64 + i32::from(t.z)) * w + cx * 64 + i32::from(t.x)) as usize;
                    ids[i] = t.colour as i16;
                }
            }
            blend(&ids, &mut out, size, flo, hue, light, &self.palette);
            for (at, chunk) in area.upper[level].iter_mut().enumerate() {
                let Some(chunk) = chunk else { continue };
                let (cx, cz) = (at as i32 / cz_count, at as i32 % cz_count);
                for t in chunk.values_mut() {
                    let i = ((cz * 64 + i32::from(t.z)) * w + cx * 64 + i32::from(t.x)) as usize;
                    t.colour = out[i];
                    if t.colour != 0 {
                        t.colour |= 0xFF00_0000u32 as i32;
                    }
                }
            }
        }
        let ground = std::mem::take(&mut area.underlay);
        blend(&ground, &mut out, size, flo, hue, light, &self.palette);
        area.colour = out;
    }

    /// The map element of a loc, resolved through its multiloc variable.
    fn loc_element(&self, id: i32, read_var: &dyn Fn(bool, i32) -> i32) -> i32 {
        let Some(locs) = self.locs.as_ref() else {
            return -1;
        };
        let Some(loc) = u32::try_from(id).ok().and_then(|id| locs.get(id)) else {
            return -1;
        };
        let mut element = loc.mapelement;
        if loc.has_multiloc && !loc.multiloc.is_empty() {
            let value = if loc.multivarbit != -1 {
                read_var(true, loc.multivarbit)
            } else if loc.multivarp != -1 {
                read_var(false, loc.multivarp)
            } else {
                -1
            };
            let last = loc.multiloc.len() - 1;
            let next = if value >= 0 && (value as usize) < last {
                loc.multiloc[value as usize]
            } else {
                loc.multiloc[last]
            };
            if next != -1 {
                if let Some(multi) = locs.get(next as u32) {
                    element = multi.mapelement;
                }
            }
        }
        element
    }

    /// An element per mapped loc: ground level column-major, then the upper
    /// levels.
    fn list_loc_elements(&mut self, read_var: &dyn Fn(bool, i32) -> i32) {
        let Some(area) = self.area.as_ref() else {
            return;
        };
        let mut elements = Vec::new();
        let [w, h] = area.size;
        for x in 0..w {
            for z in 0..h {
                let Some(locs) = area.locs.get(&((w * z + x) as usize)) else {
                    continue;
                };
                let ids = match locs {
                    TileLocs::One(id, _) => vec![*id],
                    TileLocs::Many(ids, _) => ids.clone(),
                };
                for id in ids {
                    let element = self.loc_element(id, read_var);
                    if element != -1 {
                        elements.push(Element::new(element, x, z));
                    }
                }
            }
        }
        let cz_count = area.chunks_z();
        let cx_count = area.size[0] >> 6;
        for level in 0..3 {
            for cx in 0..cx_count {
                for cz in 0..cz_count {
                    let Some(chunk) = area.upper[level][(cx * cz_count + cz) as usize].as_ref()
                    else {
                        continue;
                    };
                    // The chunk's tiles are listed in hash-bucket order; keys
                    // are unique tiles, so the order only affects the list
                    // position of same-chunk upper-level elements.
                    let mut tiles: Vec<&UpperTile> = chunk.values().collect();
                    tiles.sort_by_key(|t| {
                        hash_bucket_order(((i32::from(t.x)) << 8) + i32::from(t.z), chunk.len())
                    });
                    for t in tiles {
                        let Some((ids, _)) = t.locs.as_ref() else {
                            continue;
                        };
                        for &id in ids {
                            let element = self.loc_element(id, read_var);
                            if element != -1 {
                                elements.push(Element::new(
                                    element,
                                    ((area.origin[0] >> 6) + cx) * 64 + i32::from(t.x)
                                        - area.origin[0],
                                    ((area.origin[1] >> 6) + cz) * 64 + i32::from(t.z)
                                        - area.origin[1],
                                ));
                            }
                        }
                    }
                }
            }
        }
        if let Some(area) = self.area.as_mut() {
            area.elements = elements;
        }
    }

    /// Lists the static elements through the current map's subareas.
    pub fn list_static_elements(&mut self) {
        let (Some(meta), Some(list)) = (self.metadata().cloned(), self.static_elements.as_ref())
        else {
            return;
        };
        let Some(area) = self.area.as_mut() else {
            return;
        };
        for (index, &packed) in list.coords.iter().enumerate() {
            if let Some([x, z]) = source_to_display(
                &meta,
                packed >> 28 & 0x3,
                packed >> 14 & 0x3FFF,
                packed & 0x3FFF,
            ) {
                area.elements.push(Element::new(
                    list.elements[index],
                    x - area.origin[0],
                    z - area.origin[1],
                ));
            }
        }
    }

    /// Sets the draw view from absolute display coordinates: the tile window
    /// is stored relative to the area origin.
    pub fn set_view(&mut self, view: View) {
        let [ox, oz] = self.area.as_ref().map_or([0, 0], |a| a.origin);
        self.view = View {
            left: view.left - ox,
            top: view.top - oz,
            right: view.right - ox,
            bottom: view.bottom - oz,
            ..view
        };
    }

    /// The chunk sprite size for the target zoom.
    pub fn chunk_size(&self) -> i32 {
        (self.target_zoom * 64.0 / 2.0) as i32
    }

    fn cache(&mut self) -> &mut ChunkCache {
        if self.overview_active {
            &mut self.overview_cache
        } else {
            &mut self.main_cache
        }
    }

    /// Renders missing/stale chunk sprites and draws them through `canvas`.
    pub fn draw_chunks(
        &mut self,
        canvas: &mut dyn Canvas,
        draw_locs: bool,
        draw_icons: bool,
        members: bool,
        overview: bool,
    ) {
        let v = self.view;
        let Some(area) = self.area.as_ref() else {
            return;
        };
        let [aw, ah] = area.size;
        let tiles_x = v.right - v.left;
        let tiles_z = v.top - v.bottom;
        if tiles_x == 0 || tiles_z == 0 {
            return;
        }
        let sx = ((v.x1 - v.x0) << 16) / tiles_x;
        let sz = ((v.y1 - v.y0) << 16) / tiles_z;
        let mut span_x = tiles_x;
        let mut span_z = tiles_z;
        if v.right < aw {
            span_x += 1;
        }
        if v.top < ah {
            span_z += 1;
        }
        let (first_x, first_z) = (v.left / 64, v.bottom / 64);
        let (last_x, last_z) = ((v.left + span_x) / 64, (v.bottom + span_z) / 64);
        let (size, drawn) = if overview {
            let chunks = (v.right - v.left) / 64;
            let s = if chunks == 0 {
                0
            } else {
                (v.x1 - v.x0) / chunks
            };
            (s, s)
        } else {
            (self.chunk_size(), ceil_i32(self.zoom * 64.0 / 2.0))
        };
        if size <= 0 {
            return;
        }
        let layers = ChunkLayers {
            locs: draw_locs,
            icons: draw_icons,
            members,
        };
        let mut stale = Vec::new();
        let mut spent = 0i32;
        let cost = size.wrapping_mul(size);
        for cx in first_x..=last_x {
            for cz in first_z..=last_z {
                let key = (i64::from(cx) << 16) + i64::from(cz);
                let cached = self.cache().width(key);
                if size == cached {
                    continue;
                }
                if cx < 0 || cx * 64 >= aw || cz < 0 || cz * 64 >= ah {
                    self.background_chunk(size, key);
                } else if cached == -1 {
                    self.render_chunk(cx, cz, size, key, layers);
                    spent = spent.wrapping_add(cost);
                } else {
                    stale.push((cx, cz, key));
                }
            }
        }
        for (cx, cz, key) in stale {
            if spent >= RERENDER_BUDGET {
                break;
            }
            self.render_chunk(cx, cz, size, key, layers);
            spent = spent.wrapping_add(cost);
        }
        for cx in first_x..=last_x {
            let dx = cx * 64 - v.left;
            let left = (sx.wrapping_mul(dx) >> 16) + v.x0;
            let right = ((dx + 64).wrapping_mul(sx) >> 16) + v.x0;
            let width = if drawn + left != right {
                right - left
            } else {
                drawn
            };
            for cz in first_z..=last_z {
                let dz = cz * 64 - v.bottom;
                let key = (i64::from(cx) << 16) + i64::from(cz);
                let bottom = v.y1 - (sz.wrapping_mul(dz) >> 16);
                let top = v.y1 - ((dz + 64).wrapping_mul(sz) >> 16);
                let height = if bottom - drawn != top {
                    bottom - top
                } else {
                    drawn
                };
                let y = bottom - height;
                // A chunk evicted mid-draw is skipped.
                let Some(sprite) = self.cache().get(key) else {
                    continue;
                };
                if sprite.size[0] == width && sprite.size[1] == height {
                    canvas.sprite(&sprite, [left, y]);
                } else {
                    canvas.scaled(&sprite, [left, y, width, height]);
                }
            }
        }
    }

    /// The background-coloured sprite shared by out-of-area chunks.
    fn background_chunk(&mut self, size: i32, key: i64) {
        let background = self.metadata().map_or(-1, |m| m.background);
        let sprite = match self.cache().get(-1) {
            Some(s) if s.size[0] == size => s,
            _ => {
                let colour = if background == -1 {
                    -16_777_216
                } else {
                    background | 0xFF00_0000u32 as i32
                };
                let n = size.max(0) as usize;
                let s = Rc::new(Sprite {
                    paletted: None,
                    size: [size, size],
                    padding: [0; 4],
                    argb: vec![colour; n * n],
                });
                self.cache().put(s.clone(), -1);
                s
            }
        };
        self.background = Some(sprite.clone());
        self.cache().put(sprite, key);
    }

    fn render_chunk(&mut self, cx: i32, cz: i32, size: i32, key: i64, layers: ChunkLayers) {
        let (Some(area), Some(pack)) = (self.area.as_ref(), self.pack.as_ref()) else {
            return;
        };
        let background = self
            .areas
            .get(self.current.unwrap_or(usize::MAX))
            .map_or(-1, |m| m.background);
        let mut painter = ChunkPainter {
            area,
            defaults: &self.defaults,
            shapes: &self.shapes,
            shape_size: self.shape_size,
            overlay_colours: &self.overlay_colours,
            locs: self.locs.as_deref(),
            scenes: &mut self.scenes,
            pack,
            background,
            members_map: self.members_map,
            shade_cache: &mut self.shade_cache,
        };
        let sprite =
            Rc::new(painter.render(cx, cz, size, layers.locs, layers.icons, layers.members));
        self.cache().put(sprite, key);
    }

    /// Projects every element to its screen point.
    pub fn project_elements(&mut self) {
        let v = self.view;
        if let Some(area) = self.area.as_mut() {
            for e in &mut area.elements {
                e.screen = v.project(e.x, e.z);
            }
        }
    }
}

/// The `worldmapareas` group's file 0: the area's tile file.
fn tile_file(pack: &Pack, name: &str) -> Result<Vec<u8>> {
    let group = pack
        .group_id_by_name("worldmapareas", name)?
        .with_context(|| format!("world-map area data group {name:?}"))?;
    pack.read_group("worldmapareas", group)?
        .remove(&0)
        .with_context(|| format!("world-map area data {name:?} file 0"))
}

/// The bucket position of an integer key in a hash map of `len` entries
/// (default capacity 16, doubled at a 0.75 load factor; the hash spread
/// `h ^ h >>> 16` is the identity for these 14-bit keys).
fn hash_bucket_order(key: i32, len: usize) -> (usize, i32) {
    let mut capacity = 16usize;
    while len as f32 > capacity as f32 * 0.75 {
        capacity *= 2;
    }
    ((key as usize) & (capacity - 1), key)
}

/// The drawing calls the world-map draw makes.
pub trait Canvas {
    /// Draws the sprite at `pos`.
    fn sprite(&mut self, sprite: &Rc<Sprite>, pos: [i32; 2]);
    /// Draws the sprite scaled into `rect`.
    fn scaled(&mut self, sprite: &Rc<Sprite>, rect: [i32; 4]);
    /// Draws the sprite at `pos` tinted by `colour`.
    fn sprite_tinted(&mut self, sprite: &Rc<Sprite>, pos: [i32; 2], colour: i32);
    /// Draws the sprite rotated by `angle` about `centre`.
    fn rotated(&mut self, sprite: &Rc<Sprite>, centre: [f32; 2], angle: i32);
    /// Fills `rect` (`[x, y, w, h]`).
    fn fill(&mut self, rect: [i32; 4], colour: i32);
    /// Outlines `rect` (`[x, y, w, h]`).
    fn outline(&mut self, rect: [i32; 4], colour: i32);
    /// Draws a line in the given dash `style` (a dash <= 0 is the solid line).
    fn line(&mut self, from: [i32; 2], to: [i32; 2], colour: i32, style: [i32; 3]);
    /// Fills the polygon of flat `[x, y, ...]` `points`.
    fn polygon(&mut self, points: &[i32], colour: i32);
    /// The `[height, width]` of `text` in font `font`; `None` when the font
    /// is not loaded.
    fn measure(&mut self, font: i32, text: &str) -> Option<[i32; 2]>;
    /// Draws `text` in `rect` with a shadow colour.
    fn text(&mut self, font: i32, text: &str, rect: [i32; 4], colour: i32, shadow: i32);
    /// Draws `text` centred at `pos` in the default full font.
    fn text_centre(&mut self, text: &str, pos: [i32; 2], colour: i32);
}

#[cfg(test)]
mod tests;
