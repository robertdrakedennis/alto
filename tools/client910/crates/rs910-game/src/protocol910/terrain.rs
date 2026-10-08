//! CPU surface LAND state: tile flags, vertex heights and the fine-height
//! lookup. Environment bytes are returned to the caller, never discarded or acknowledged.
//! Rotated region templates are assembled by `read_region`; underwater and
//! environment trailer ownership remain separate from this CPU surface.
use super::{Context, Error, Packet, Result};
use rs910_core::perlin::perlin;

const LAND_SQUARE_TILES: usize = 64;
const LAND_PLANES: usize = 4;
const LAND_HEIGHT_UNIT: i32 = 32;
const DEFAULT_PLANE_SEPARATION: i32 = 960;
const HEIGHT_NOISE_OFFSET_X: i32 = 932_731;
const HEIGHT_NOISE_OFFSET_Z: i32 = 556_238;
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tile {
    pub underlay: i16,
    pub overlay: i16,
    pub shape: i8,
    pub rotation: i8,
    pub flags: i8,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terrain {
    pub width: usize,
    pub height: usize,
    pub heights: Vec<i32>,
    pub tiles: Vec<Tile>,
}
/// Where one 8x8 region template lands and which source square chunk feeds
/// it: destination plane and tile, source plane and chunk, quarter turns.
#[derive(Clone, Copy, Debug)]
pub struct RegionCopy {
    pub level: usize,
    pub tile_x: i32,
    pub tile_z: i32,
    pub src_level: usize,
    pub src_chunk_x: i32,
    pub src_chunk_z: i32,
    pub rotation: i32,
}
pub struct Decoded {
    pub state: Terrain,
    pub consumed: usize,
    pub environment: Vec<u8>,
}
impl Terrain {
    pub fn new(width: usize, height: usize) -> Result<Self> {
        if width == 0 || height == 0 || width > 256 || height > 256 {
            return Err(Error::Invalid("terrain dimensions"));
        }
        Ok(Self {
            width,
            height,
            heights: vec![0; 4 * (width + 1) * (height + 1)],
            tiles: vec![Tile::default(); 4 * width * height],
        })
    }
    pub fn point(&self, l: usize, x: usize, z: usize) -> usize {
        (l * (self.width + 1) + x) * (self.height + 1) + z
    }
    pub fn tile(&self, l: usize, x: usize, z: usize) -> usize {
        (l * self.width + x) * self.height + z
    }
    pub fn validate(&self) -> Result<()> {
        if self.width == 0
            || self.height == 0
            || self.width > 256
            || self.height > 256
            || self.heights.len() != 4 * (self.width + 1) * (self.height + 1)
            || self.tiles.len() != 4 * self.width * self.height
        {
            Err(Error::Invalid("terrain array dimensions"))
        } else {
            Ok(())
        }
    }
    pub fn context(&self, local: usize, base_x: i32, base_z: i32) -> Result<Context> {
        self.validate()?;
        if local >= 2048 {
            return Err(Error::Invalid("local index"));
        }
        let mut bridges = vec![];
        for x in 0..self.width {
            for z in 0..self.height {
                if self.tiles[self.tile(1, x, z)].flags & 2 != 0 {
                    bridges.push((x as i32, z as i32))
                }
            }
        }
        Ok(Context {
            local,
            base_x,
            base_z,
            width: self.width as i32,
            height: self.height as i32,
            bridges,
        })
    }
    /// Atomic valid surface decode. Existing overlays/underlays survive absent bits;
    /// tile flags are reset. Border heights at width/height remain untouched.
    pub fn read_normal(
        &self,
        b: &[u8],
        offset_x: i32,
        offset_z: i32,
        base_x: i32,
        base_z: i32,
    ) -> Result<Decoded> {
        self.decode_normal(b, offset_x, offset_z, base_x, base_z, None)
    }

    fn decode_normal(
        &self,
        b: &[u8],
        offset_x: i32,
        offset_z: i32,
        base_x: i32,
        base_z: i32,
        mut height_fields: Option<&mut [Option<i32>]>,
    ) -> Result<Decoded> {
        self.validate()?;
        let mut s = self.clone();
        let mut p = Packet::new(b);
        for l in 0..4 {
            for x in 0..64i32 {
                for z in 0..64i32 {
                    let tx = offset_x.wrapping_add(x);
                    let tz = offset_z.wrapping_add(z);
                    let inside =
                        tx >= 0 && tz >= 0 && (tx as usize) < s.width && (tz as usize) < s.height;
                    let op = p.byte()?;
                    let index = if inside {
                        Some(s.tile(l, tx as usize, tz as usize))
                    } else {
                        None
                    };
                    if let Some(i) = index {
                        s.tiles[i].flags = 0;
                    }
                    if op & 1 != 0 {
                        let shape = p.byte()?;
                        let overlay = p.smart1()? as i16;
                        if let Some(i) = index {
                            s.tiles[i].overlay = overlay;
                            s.tiles[i].shape = (shape >> 2) as i8;
                            s.tiles[i].rotation = (shape & 3) as i8;
                        }
                    }
                    if op & 2 != 0 {
                        let flags = p.byte()? as i8;
                        if let Some(i) = index {
                            s.tiles[i].flags = flags;
                        }
                    }
                    if op & 4 != 0 {
                        let underlay = p.smart1()? as i16;
                        if let Some(i) = index {
                            s.tiles[i].underlay = underlay;
                        }
                    }
                    let explicit = if op & 8 != 0 {
                        Some(p.byte()? as i32)
                    } else {
                        None
                    };
                    if let Some(fields) = height_fields.as_deref_mut() {
                        fields[(l * LAND_SQUARE_TILES + x as usize) * LAND_SQUARE_TILES
                            + z as usize] =
                            explicit.map(|height| if height == 1 { 0 } else { height });
                    }
                    if inside {
                        let px = tx as usize;
                        let pz = tz as usize;
                        let i = s.point(l, px, pz);
                        let h = if let Some(v) = explicit {
                            let v = if v == 1 { 0 } else { v };
                            if l == 0 {
                                -v * 32
                            } else {
                                s.heights[s.point(l - 1, px, pz)].wrapping_sub(v * 32)
                            }
                        } else if l == 0 {
                            -perlin(
                                base_x.wrapping_add(tx).wrapping_add(932731),
                                base_z.wrapping_add(tz).wrapping_add(556238),
                            ) * 32
                        } else {
                            s.heights[s.point(l - 1, px, pz)].wrapping_sub(960)
                        };
                        s.heights[i] = h;
                    }
                }
            }
        }
        Ok(Decoded {
            state: s,
            consumed: p.pos,
            environment: b[p.pos..].to_vec(),
        })
    }

    /// Decode one rotated 8x8 region template from a source LAND square.
    /// Decode the source tiles and retain their encoded height fields. Heights
    /// are resolved against the destination plane below, while ground-plane
    /// noise keeps the selected source tile's absolute position. The complete
    /// LAND cursor and rotated edge vertices remain part of the transaction.
    pub fn read_region(&self, b: &[u8], copy: RegionCopy) -> Result<Decoded> {
        let RegionCopy {
            level,
            tile_x,
            tile_z,
            src_level,
            src_chunk_x,
            src_chunk_z,
            rotation,
        } = copy;
        if src_level >= 4 || level >= 4 {
            return Err(Error::Invalid("region plane"));
        }
        self.validate()?;
        let mut height_fields = vec![None; LAND_PLANES * LAND_SQUARE_TILES * LAND_SQUARE_TILES];
        let source = Terrain::new(LAND_SQUARE_TILES, LAND_SQUARE_TILES)?.decode_normal(
            b,
            0,
            0,
            (src_chunk_x & !7) << 3,
            (src_chunk_z & !7) << 3,
            Some(&mut height_fields),
        )?;
        let mut state = self.clone();
        let sx0 = (src_chunk_x & 7) * 8;
        let sz0 = (src_chunk_z & 7) * 8;
        let rotate = |x: i32, z: i32| -> (i32, i32) {
            match rotation & 3 {
                0 => (x, z),
                1 => (z, 7 - x),
                2 => (7 - x, 7 - z),
                _ => (7 - z, x),
            }
        };
        for x in 0..8 {
            for z in 0..8 {
                let (dx, dz) = rotate(x, z);
                let tx = tile_x + dx;
                let tz = tile_z + dz;
                if tx < 0 || tz < 0 || tx >= state.width as i32 || tz >= state.height as i32 {
                    continue;
                }
                let src = source
                    .state
                    .tile(src_level, (sx0 + x) as usize, (sz0 + z) as usize);
                let dst = state.tile(level, tx as usize, tz as usize);
                state.tiles[dst] = source.state.tiles[src].clone();
            }
        }
        // Vertex heights follow the tile/edge destinations and bounds.
        // Interior tiles store their height at the rotated tile plus the
        // rotation offset; the far edge (x or z == 8) is read in place with
        // no offset; destinations at or beyond the map size are skipped, and
        // a source square's last row/column copies into the vertex 64
        // destinations.
        let (rot_off_x, rot_off_z) = match rotation & 3 {
            1 => (0, 1),
            2 => (1, 1),
            3 => (1, 0),
            _ => (0, 0),
        };
        let edge_dest = |x: i32, z: i32| -> (i32, i32) {
            match rotation & 3 {
                0 => (tile_x + x, tile_z + z),
                1 => (tile_x + z, tile_z + 8 - x),
                2 => (tile_x + 8 - x, tile_z + 8 - z),
                _ => (tile_x + 8 - z, tile_z + x),
            }
        };
        let (w, h) = (state.width as i32, state.height as i32);
        let inside = |x: i32, z: i32| x >= 0 && z >= 0 && x < w && z < h;
        for src_x in sx0..=(sx0 + 8).min(63) {
            for src_z in sz0..=(sz0 + 8).min(63) {
                let (x, z) = (src_x - sx0, src_z - sz0);
                let (dest_x, dest_z, ox, oz) = if x == 8 || z == 8 {
                    let (a, b) = edge_dest(x, z);
                    (a, b, 0, 0)
                } else {
                    let (dx, dz) = rotate(x, z);
                    (tile_x + dx, tile_z + dz, rot_off_x, rot_off_z)
                };
                if inside(dest_x, dest_z) {
                    let source_index = (src_level * LAND_SQUARE_TILES + src_x as usize)
                        * LAND_SQUARE_TILES
                        + src_z as usize;
                    let vertex_x = (dest_x + ox) as usize;
                    let vertex_z = (dest_z + oz) as usize;
                    let below = if level == 0 {
                        0
                    } else {
                        state.heights[state.point(level - 1, vertex_x, vertex_z)]
                    };
                    let height = match height_fields[source_index] {
                        Some(delta) => below.wrapping_sub(delta * LAND_HEIGHT_UNIT),
                        None if level > 0 => below.wrapping_sub(DEFAULT_PLANE_SEPARATION),
                        None => {
                            -perlin(
                                ((src_chunk_x & !7) << 3)
                                    .wrapping_add(src_x)
                                    .wrapping_add(HEIGHT_NOISE_OFFSET_X),
                                ((src_chunk_z & !7) << 3)
                                    .wrapping_add(src_z)
                                    .wrapping_add(HEIGHT_NOISE_OFFSET_Z),
                            ) * LAND_HEIGHT_UNIT
                        }
                    };
                    let dst = state.point(level, vertex_x, vertex_z);
                    state.heights[dst] = height;
                }
                if src_x == 63 || src_z == 63 {
                    let n = if src_x == 63 && src_z == 63 { 3 } else { 1 };
                    for edge_index in 0..n {
                        let (edge_src_x, edge_src_z) = match edge_index {
                            0 => (
                                if src_x == 63 { 64 } else { src_x },
                                if src_z == 63 { 64 } else { src_z },
                            ),
                            1 => (64, src_z),
                            _ => (src_x, 64),
                        };
                        let (edge_dest_x, edge_dest_z) =
                            edge_dest(edge_src_x - sx0, edge_src_z - sz0);
                        let (fx, fz) = (rot_off_x + dest_x, rot_off_z + dest_z);
                        if inside(edge_dest_x, edge_dest_z)
                            && fx >= 0
                            && fz >= 0
                            && fx <= w
                            && fz <= h
                        {
                            let from = state.point(level, fx as usize, fz as usize);
                            let dst =
                                state.point(level, edge_dest_x as usize, edge_dest_z as usize);
                            state.heights[dst] = state.heights[from];
                        }
                    }
                }
            }
        }
        Ok(Decoded {
            state,
            consumed: source.consumed,
            environment: source.environment,
        })
    }
    /// Clear the landscape of `width` x `height` tiles at (x, z) on every
    /// level (an absent normal mapsquare).
    pub fn clear_landscape(&mut self, x: i32, z: i32, width: i32, height: i32) -> Result<()> {
        for level in 0..4 {
            self.clear_landscape_level(level, x, z, width, height)?;
        }
        Ok(())
    }
    /// Clear one level of the landscape of `width` x `height` tiles at
    /// (x, z): the heights fall to the level below's less 960 (0 on level 0),
    /// the near edges follow their outer neighbours and the corner keeps a
    /// neighbour's raised height (an instanced chunk nothing is copied into).
    pub fn clear_landscape_level(
        &mut self,
        l: usize,
        x: i32,
        z: i32,
        width: i32,
        height: i32,
    ) -> Result<()> {
        self.validate()?;
        if l >= 4 || !(0..=256).contains(&width) || !(0..=256).contains(&height) {
            return Err(Error::Invalid("clear landscape dimensions"));
        }
        let valid = |x: i32, z: i32| {
            x >= 0 && z >= 0 && (x as usize) < self.width && (z as usize) < self.height
        };
        for dz in 0..height {
            for dx in 0..width {
                let px = x.wrapping_add(dx);
                let pz = z.wrapping_add(dz);
                if valid(px, pz) {
                    let i = self.point(l, px as usize, pz as usize);
                    self.heights[i] = if l == 0 {
                        0
                    } else {
                        self.heights[self.point(l - 1, px as usize, pz as usize)].wrapping_sub(960)
                    }
                }
            }
        }
        if x > 0 && (x as usize) < self.width {
            for dz in 1..height {
                let pz = z.wrapping_add(dz);
                if valid(x, pz) {
                    let i = self.point(l, x as usize, pz as usize);
                    self.heights[i] = self.heights[self.point(l, x as usize - 1, pz as usize)];
                }
            }
        }
        if z > 0 && (z as usize) < self.height {
            for dx in 1..width {
                let px = x.wrapping_add(dx);
                if valid(px, z) {
                    let i = self.point(l, px as usize, z as usize);
                    self.heights[i] = self.heights[self.point(l, px as usize, z as usize - 1)];
                }
            }
        }
        if !valid(x, z) {
            return Ok(());
        }
        for (px, pz) in [
            (x.wrapping_sub(1), z),
            (x, z.wrapping_sub(1)),
            (x.wrapping_sub(1), z.wrapping_sub(1)),
        ] {
            if !valid(px, pz) {
                continue;
            }
            let value = self.heights[self.point(l, px as usize, pz as usize)];
            if (l == 0 && value != 0)
                || (l > 0 && self.heights[self.point(l - 1, px as usize, pz as usize)] != value)
            {
                let i = self.point(l, x as usize, z as usize);
                self.heights[i] = value;
                break;
            }
        }
        Ok(())
    }
    pub fn fine_height(&self, x: i32, z: i32, level: i32) -> Result<i32> {
        self.validate()?;
        let tx = x >> 9;
        let tz = z >> 9;
        if tx < 0 || tz < 0 || tx >= self.width as i32 || tz >= self.height as i32 {
            return Ok(0);
        }
        let l = usize::try_from(level)
            .ok()
            .filter(|&l| l < 4)
            .ok_or(Error::Invalid("heightmap level"))?;
        let (tx, tz) = (tx as usize, tz as usize);
        let fx = x & 511;
        let fz = z & 511;
        let h = |dx, dz| self.heights[self.point(l, tx + dx, tz + dz)];
        let mix = |a: i32, b: i32, t: i32| {
            ((512 - t).wrapping_mul(a).wrapping_add(t.wrapping_mul(b))) >> 9
        };
        Ok(mix(
            mix(h(0, 0), h(1, 0), fx),
            mix(h(0, 1), h(1, 1), fx),
            fz,
        ))
    }
}
pub fn height(scene: Option<&Terrain>, x: i32, z: i32, level: i32) -> Result<i32> {
    let Some(s) = scene else { return Ok(0) };
    s.validate()?;
    let tx = x >> 9;
    let tz = z >> 9;
    if tx < 0 || tz < 0 || tx >= s.width as i32 || tz >= s.height as i32 {
        return Ok(0);
    }
    let level = if level < 3 && s.tiles[s.tile(1, tx as usize, tz as usize)].flags & 2 != 0 {
        level.wrapping_add(1)
    } else {
        level
    };
    s.fine_height(x, z, level)
}
/// Footprint samples on a bridge boundary use the
/// actor's reference tile when the two adjacent tiles disagree about the bridge.
pub fn footprint_height(
    scene: Option<&Terrain>,
    x: i32,
    z: i32,
    reference: [i32; 2],
    mut level: i32,
) -> Result<i32> {
    let Some(s) = scene else { return Ok(0) };
    s.validate()?;
    if level < 3 {
        let (tx, tz) = (x >> 9, z >> 9);
        let inside = |x, z| x >= 0 && z >= 0 && x < s.width as i32 && z < s.height as i32;
        if !inside(reference[0], reference[1]) || !inside(tx, tz) || tx < 1 || tz < 1 {
            return Ok(0);
        }
        let bridge = |x, z| s.tiles[s.tile(1, x as usize, z as usize)].flags & 2 != 0;
        let mut raised = bridge(tx, tz);
        if x & 511 == 0 && bridge(tx - 1, tz) != bridge(tx, tz) {
            raised = bridge(reference[0], reference[1]);
        }
        if z & 511 == 0 && bridge(tx, tz - 1) != bridge(tx, tz) {
            raised = bridge(reference[0], reference[1]);
        }
        if raised {
            level = level.wrapping_add(1);
        }
    }
    s.fine_height(x, z, level)
}
