//! World-map chunk rasterisation, overlay shapes, icons and shaded borders.

use crate::{
    cache::Pack,
    config::{Loc, LocStore},
    ui_sprites::Sprite,
};

use super::{
    shape_rotation, shape_table, Area, Defaults, MapScenes, TileLocs, CORNER_TEMPLATE, EAST, NORTH,
    NORTHEAST, NORTHWEST, SOUTH, SOUTHEAST, SOUTHWEST, WALL_DIAGONAL, WALL_L, WALL_SQUARE_CORNER,
    WALL_STRAIGHT, WEST,
};

/// One tile's paint inputs: its pixel rectangle `[x0, x1, y0, y1]` within
/// the chunk sprite, its floor colour, overlay and shape, and its locs.
pub(super) struct TilePaint<'a> {
    rect: [i32; 4],
    walls: bool,
    colour: i32,
    overlay: i32,
    shape: i32,
    locs: Option<(&'a [i32], &'a [i8])>,
    ground: bool,
}

/// The blend inputs shared by the pieces of one shaded block: the sprite
/// size, the shade alpha and its inverse, and which of the eight neighbouring
/// blocks are member-accessible.
#[derive(Clone, Copy)]
pub(super) struct ShadeBlock<'a> {
    size: i32,
    alpha: i32,
    inverse: i32,
    neighbours: &'a [bool; 8],
}

/// Which layers one chunk render draws.
#[derive(Clone, Copy)]
pub(super) struct ChunkLayers {
    pub(super) locs: bool,
    pub(super) icons: bool,
    pub(super) members: bool,
}

/// Everything the painter reads while it renders one chunk sprite.
pub struct ChunkPainter<'a> {
    pub area: &'a Area,
    pub defaults: &'a Defaults,
    /// The tile-shape masks and their texel size.
    pub shapes: &'a [[Vec<u8>; 4]],
    pub shape_size: i32,
    /// The overlay colour per overlay id.
    pub overlay_colours: &'a [i32],
    pub locs: Option<&'a LocStore>,
    pub scenes: &'a mut MapScenes,
    pub pack: &'a Pack,
    /// The current area's background colour.
    pub background: i32,
    /// Whether the current area is the members-shaded map.
    pub members_map: bool,
    /// The last shade input and output.
    pub shade_cache: &'a mut (i32, i32),
}

impl ChunkPainter<'_> {
    pub(super) fn loc(&self, id: i32) -> Option<&Loc> {
        self.locs?.get(u32::try_from(id).ok()?)
    }
    /// Renders chunk `(cx, cz)` at `size` pixels.
    pub fn render(
        &mut self,
        cx: i32,
        cz: i32,
        size: i32,
        draw_locs: bool,
        draw_icons: bool,
        members: bool,
    ) -> Sprite {
        let n = size.max(0) as usize;
        let mut px = vec![0i32; n * n];
        let x0 = cx * 64;
        let z0 = cz * 64;
        let shaded = self.members_map && !members;
        let background = self.background | 0xFF00_0000u32 as i32;
        let w = self.area.size[0];
        for tx in 0..64 {
            let x = x0 + tx;
            let px0 = size * tx / 64;
            let px1 = ((tx + 1) * size / 64 - 1).max(px0);
            for tz in 0..64 {
                let z = z0 + tz;
                let walls = draw_locs && (!shaded || self.area.member_block(x, z));
                let Some(i) = self.area.index(x, z) else {
                    continue;
                };
                let rgb = self.area.colour[i];
                let colour = if rgb == 0 {
                    background
                } else {
                    rgb | 0xFF00_0000u32 as i32
                };
                let pz0 = size * tz / 64;
                let pz1 = ((tz + 1) * size / 64 - 1).max(pz0);
                let locs = self.area.locs.get(&i).map(TileLocs::split);
                self.tile(
                    &mut px,
                    size,
                    TilePaint {
                        rect: [px0, px1, pz0, pz1],
                        walls,
                        colour,
                        overlay: i32::from(self.area.overlay[i]),
                        shape: i32::from(self.area.shape[i]),
                        locs: locs.as_ref().map(|(a, b)| (a.as_slice(), b.as_slice())),
                        ground: true,
                    },
                );
            }
        }
        if draw_icons {
            for tx in 0..64 {
                let x = x0 + tx;
                let left_edge = tx == 0 && x != 0;
                for tz in 0..64 {
                    let z = z0 + tz;
                    if shaded && !self.area.member_block(x, z) {
                        continue;
                    }
                    let bottom_edge = tz == 0 && z != 0;
                    // Icons of the western/southern neighbours overlap into
                    // this chunk.
                    if left_edge && bottom_edge {
                        self.ground_icons(&mut px, size, -1, -1, x - 1, z - 1);
                    }
                    if left_edge {
                        self.ground_icons(&mut px, size, -1, tz, x - 1, z);
                    }
                    if bottom_edge {
                        self.ground_icons(&mut px, size, tx, -1, x, z - 1);
                    }
                    self.ground_icons(&mut px, size, tx, tz, x, z);
                }
            }
        }
        let _ = w;
        for level in 0..3 {
            let Some(chunk) = self.area.upper_chunk(level, cx, cz) else {
                continue;
            };
            for tx in 0..64 {
                let x = x0 + tx;
                let px0 = size * tx / 64;
                let px1 = ((tx + 1) * size / 64 - 1).max(px0);
                for tz in 0..64 {
                    let z = z0 + tz;
                    let walls = draw_locs && (!shaded || self.area.member_block(x, z));
                    let pz0 = size * tz / 64;
                    let pz1 = ((tz + 1) * size / 64 - 1).max(pz0);
                    let key = (((x & 0x3F) << 8) + (z & 0x3F)) as u16;
                    if let Some(t) = chunk.get(&key) {
                        let locs = t.locs.as_ref().map(|(a, b)| (a.as_slice(), b.as_slice()));
                        self.tile(
                            &mut px,
                            size,
                            TilePaint {
                                rect: [px0, px1, pz0, pz1],
                                walls,
                                colour: t.colour,
                                overlay: i32::from(t.overlay),
                                shape: i32::from(t.shape),
                                locs,
                                ground: false,
                            },
                        );
                    }
                }
            }
            for tx in 0..64 {
                let x = x0 + tx;
                for tz in 0..64 {
                    let z = z0 + tz;
                    if shaded && !self.area.member_block(x, z) {
                        continue;
                    }
                    let key = (((x & 0x3F) << 8) + (z & 0x3F)) as u16;
                    if let Some(t) = chunk.get(&key) {
                        if let Some((ids, angles)) = t.locs.as_ref() {
                            self.icons(&mut px, size, tx, tz, ids, angles);
                        }
                    }
                }
            }
        }
        if shaded {
            let blocks = 8;
            for bx in 0..blocks {
                let x = bx * 8 + x0;
                let px0 = size * bx / blocks;
                let px1 = (bx + 1) * size / blocks - 1;
                for bz in 0..blocks {
                    let z = bz * 8 + z0;
                    if self.area.member_block(x, z) {
                        continue;
                    }
                    let pz0 = size * bz / blocks;
                    let pz1 = (bz + 1) * size / blocks - 1;
                    if size < 64 {
                        self.shade(&mut px, px0, px1, pz0, pz1, size);
                    } else {
                        let mut around = [false; 8];
                        around[NORTH] = self.area.member_block(x, z + 8);
                        around[NORTHWEST] = self.area.member_block(x + 8, z + 8);
                        around[NORTHEAST] = self.area.member_block(x + 8, z);
                        around[SOUTHWEST] = self.area.member_block(x + 8, z - 8);
                        around[SOUTH] = self.area.member_block(x, z - 8);
                        around[WEST] = self.area.member_block(x - 8, z - 8);
                        around[EAST] = self.area.member_block(x - 8, z);
                        around[SOUTHEAST] = self.area.member_block(x - 8, z + 8);
                        self.shade_bordered(&mut px, [px0, px1, pz0, pz1], size, &around);
                    }
                }
            }
        }
        Sprite {
            paletted: None,
            size: [size, size],
            padding: [0; 4],
            argb: px,
        }
    }

    /// One tile's floor and walls.
    pub(super) fn tile(&mut self, px: &mut [i32], size: i32, tile: TilePaint<'_>) {
        let TilePaint {
            rect: [x0, x1, y0, y1],
            walls,
            colour,
            overlay,
            shape,
            locs,
            ground,
        } = tile;
        if ground || colour != 0 || overlay > 0 {
            if overlay == 0 {
                fill(px, x0, x1, y0, y1, size, colour | 0xFF00_0000u32 as i32);
            } else {
                let shape_id = shape & 0x3F;
                let over = self
                    .overlay_colours
                    .get(overlay as usize)
                    .copied()
                    .unwrap_or(0);
                if shape_id == 0 || self.shape_size == 0 {
                    if ground || over != 0 {
                        fill(px, x0, x1, y0, y1, size, over | 0xFF00_0000u32 as i32);
                    }
                } else {
                    let rotation = shape_rotation(shape >> 6 & 0x3, shape_id);
                    let table = shape_table(shape_id);
                    if let Some(mask) = self
                        .shapes
                        .get((table - 1) as usize)
                        .and_then(|row| row.get(rotation as usize))
                    {
                        shaped(
                            px,
                            size,
                            &ShapedOverlay {
                                rect: [x0, x1, y0, y1],
                                under: colour,
                                over,
                                mask,
                                shape_size: self.shape_size,
                                mode: if ground { 0 } else { 1 },
                            },
                        );
                    }
                }
            }
        }
        let (true, Some((ids, angles))) = (walls, locs) else {
            return;
        };
        let width = x1 - x0 + 1;
        let height = y1 - y0 + 1;
        for (index, &id) in ids.iter().enumerate() {
            let angle = i32::from(angles[index]);
            let kind = angle & 0x3F;
            if !matches!(
                kind,
                WALL_STRAIGHT | WALL_L | WALL_SQUARE_CORNER | WALL_DIAGONAL
            ) {
                continue;
            }
            let Some(loc) = self.loc(id) else { continue };
            if loc.mapsceneicon != -1 {
                continue;
            }
            let c = if loc.active == 1 {
                -3_407_872
            } else {
                -3_355_444
            };
            let rotation = angle >> 6 & 0x3;
            match kind {
                WALL_STRAIGHT => match rotation {
                    0 => column(px, x0, y1, height, size, c),
                    1 => row(px, x0, y1, width, size, c),
                    2 => column(px, x1, y1, height, size, c),
                    _ => row(px, x0, y0, width, size, c),
                },
                WALL_L => match rotation {
                    0 => {
                        column(px, x0, y1, height, size, -1);
                        row(px, x0, y1, width, size, c);
                    }
                    1 => {
                        column(px, x1, y1, height, size, -1);
                        row(px, x0, y1, width, size, c);
                    }
                    2 => {
                        column(px, x1, y1, height, size, -1);
                        row(px, x0, y0, width, size, c);
                    }
                    _ => {
                        column(px, x0, y1, height, size, -1);
                        row(px, x0, y0, width, size, c);
                    }
                },
                WALL_SQUARE_CORNER => match rotation {
                    0 => row(px, x0, y1, 1, size, c),
                    1 => row(px, x1, y1, 1, size, c),
                    2 => row(px, x1, y0, 1, size, c),
                    _ => row(px, x0, y0, 1, size, c),
                },
                _ => {
                    if rotation == 0 || rotation == 2 {
                        for i in 0..height {
                            row(px, x0 + i, y0 + i, 1, size, c);
                        }
                    } else {
                        for i in 0..height {
                            row(px, x0 + i, y1 - i, 1, size, c);
                        }
                    }
                }
            }
        }
    }

    /// The ground-level map scene icons of tile `(x, z)` drawn at chunk tile
    /// `(tx, tz)`.
    pub(super) fn ground_icons(
        &mut self,
        px: &mut [i32],
        size: i32,
        tx: i32,
        tz: i32,
        x: i32,
        z: i32,
    ) {
        let Some(i) = self.area.index(x, z) else {
            return;
        };
        let Some(locs) = self.area.locs.get(&i) else {
            return;
        };
        let (ids, angles) = locs.split();
        self.icons(px, size, tx, tz, &ids, &angles);
    }

    /// Draws the map scene icons of the given locs.
    pub(super) fn icons(
        &mut self,
        px: &mut [i32],
        size: i32,
        tx: i32,
        tz: i32,
        ids: &[i32],
        angles: &[i8],
    ) {
        for (index, &id) in ids.iter().enumerate() {
            let Some(loc) = self.loc(id) else { continue };
            let msi = loc.mapsceneicon;
            if msi == -1 {
                continue;
            }
            let angle = i32::from(angles[index]);
            let rotation = if loc.mapsceneiconrotate {
                angle >> 6 & 0x3
            } else {
                0
            };
            let mirror = loc.mapsceneiconmirror && loc.mirror;
            let (width, length) = (i32::from(loc.width), i32::from(loc.length));
            let Some(([sw, sh], pixels)) = self.scenes.icon(self.pack, msi, rotation, mirror)
            else {
                continue;
            };
            let mut w = (size * sw / 64) >> 2;
            let mut h = (size * sh / 64) >> 2;
            if self.scenes.scale_to_loc(msi) {
                let (mut lw, mut ll) = (width, length);
                if angle >> 6 & 0x1 == 1 {
                    std::mem::swap(&mut lw, &mut ll);
                }
                w = size * lw / 64;
                h = size * ll / 64;
            }
            if w == 0 || h == 0 {
                continue;
            }
            let left = size * tx / 64;
            let top = (64 - tz) * size / 64 - h + 1;
            for dx in 0..w {
                let x = left + dx;
                if x < 0 {
                    continue;
                }
                if x >= size {
                    break;
                }
                for dy in 0..h {
                    let y = top + dy;
                    if y < 0 {
                        continue;
                    }
                    if y >= size {
                        break;
                    }
                    let Some(&p) = pixels.get((sw * dx / w + sh * dy / h * sw) as usize) else {
                        continue;
                    };
                    if p >> 24 & 0xFF != 0 {
                        px[(size * y + x) as usize] = p;
                    }
                }
            }
        }
    }

    /// The blend of the defaults' shade colour over one pixel, memoised on
    /// the last input.
    pub(super) fn shaded(&mut self, under: i32, a: i32, inverse: i32) -> i32 {
        if self.shade_cache.0 != under {
            let s = self.defaults.shade;
            self.shade_cache.0 = under;
            self.shade_cache.1 = ((s & 0xFF00FF)
                .wrapping_mul(a)
                .wrapping_add((under & 0xFF00FF).wrapping_mul(inverse))
                & -16_711_936)
                .wrapping_add(
                    (s & 0xFF00)
                        .wrapping_mul(a)
                        .wrapping_add((under & 0xFF00).wrapping_mul(inverse))
                        & 0xFF0000,
                )
                >> 8
                | 0xFF00_0000u32 as i32;
        }
        self.shade_cache.1
    }

    /// Shades the pixel rectangle `[x0, x1] x [y0, y1]` without a border.
    pub(super) fn shade(&mut self, px: &mut [i32], x0: i32, x1: i32, y0: i32, y1: i32, size: i32) {
        let a = self.defaults.shade >> 24 & 0xFF;
        let inverse = 255 - a;
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(i) = at(size, x, size - y - 1) {
                    px[i] = self.shaded(px[i], a, inverse);
                }
            }
        }
    }

    /// A shaded 8x8 block with borders against its member-accessible
    /// neighbours, over the pixel rectangle `[x0, x1, y0, y1]`.
    pub(super) fn shade_bordered(
        &mut self,
        px: &mut [i32],
        rect: [i32; 4],
        size: i32,
        n: &[bool; 8],
    ) {
        let [x0, x1, y0, y1] = rect;
        let a = self.defaults.shade >> 24 & 0xFF;
        let inv = 255 - a;
        let border = self.defaults.border;
        let corner = self.defaults.corner;
        let block = ShadeBlock {
            size,
            alpha: a,
            inverse: inv,
            neighbours: n,
        };
        let put = |px: &mut [i32], row: i32, x: i32| {
            if let Some(i) = at(size, x, row) {
                px[i] = border;
            }
        };
        if (!n[NORTHEAST] && !n[EAST]) || (!n[NORTH] && !n[SOUTH]) {
            self.shade_rect(px, [x0, x1, y0, y1], [true, true, true, true], &block);
            if n[NORTHWEST] {
                put(px, size - y1 - 1, x1);
                put(px, size - y1, x1);
                put(px, size - y1 - 1, x1 - 1);
            }
            if n[SOUTHWEST] {
                put(px, size - y0 - 1, x1);
                put(px, size - y0 - 2, x1);
                put(px, size - y0 - 1, x1 - 1);
            }
            if n[WEST] {
                put(px, size - y0 - 1, x0);
                put(px, size - y0 - 2, x0);
                put(px, size - y0 - 1, x0 + 1);
            }
            if n[SOUTHEAST] {
                put(px, size - y1 - 1, x0);
                put(px, size - y1, x0);
                put(px, size - y1 - 1, x0 + 1);
            }
            return;
        }
        if n[NORTH] && n[NORTHEAST] {
            self.shade_corner(px, x1 - corner + 1, y1 - corner + 1, &block, NORTHWEST);
        } else {
            self.shade_rect(
                px,
                [x1 - corner + 1, x1, y1 - corner + 1, y1],
                [true, true, false, false],
                &block,
            );
            if n[NORTHWEST] {
                put(px, size - y1 - 1, x1);
                put(px, size - y1, x1);
                put(px, size - y1 - 1, x1 - 1);
            }
        }
        if n[SOUTH] && n[NORTHEAST] {
            self.shade_corner(px, x1 - corner + 1, y0, &block, SOUTHWEST);
        } else {
            self.shade_rect(
                px,
                [x1 - corner + 1, x1, y0, corner + y0 - 1],
                [false, true, true, false],
                &block,
            );
            if n[SOUTHWEST] {
                put(px, size - y0 - 1, x1);
                put(px, size - y0 - 2, x1);
                put(px, size - y0 - 1, x1 - 1);
            }
        }
        if n[SOUTH] && n[EAST] {
            self.shade_corner(px, x0, y0, &block, WEST);
        } else {
            self.shade_rect(
                px,
                [x0, corner + x0 - 1, y0, corner + y0 - 1],
                [false, false, true, true],
                &block,
            );
            if n[WEST] {
                put(px, size - y0 - 1, x0);
                put(px, size - y0 - 2, x0);
                put(px, size - y0 - 1, x0 + 1);
            }
        }
        if n[NORTH] && n[EAST] {
            self.shade_corner(px, x0, y1 - corner + 1, &block, SOUTHEAST);
        } else {
            self.shade_rect(
                px,
                [x0, corner + x0 - 1, y1 - corner + 1, y1],
                [true, false, false, true],
                &block,
            );
            if n[SOUTHEAST] {
                put(px, size - y1 - 1, x0);
                put(px, size - y1, x0);
                put(px, size - y1 - 1, x0 + 1);
            }
        }
        if corner + x0 < x1 - corner {
            self.shade_rect(
                px,
                [corner + x0, x1 - corner, y0, y1],
                [true, false, true, false],
                &block,
            );
        }
        if corner + y0 + 1 < y1 - corner - 1 {
            self.shade_rect(
                px,
                [x0, corner + x0 - 1, corner + y0, y1 - corner],
                [false, false, false, true],
                &block,
            );
            self.shade_rect(
                px,
                [x1 - corner + 1, x1, corner + y0, y1 - corner],
                [false, true, false, false],
                &block,
            );
        }
    }

    /// Shades the pixel rectangle `[x0, x1, y0, y1]`; `edges` selects which
    /// sides (north, east, south, west) draw a border against a
    /// member-accessible neighbour.
    pub(super) fn shade_rect(
        &mut self,
        px: &mut [i32],
        [x0, x1, y0, y1]: [i32; 4],
        edges: [bool; 4],
        block: &ShadeBlock<'_>,
    ) {
        let ShadeBlock {
            size,
            alpha: a,
            inverse: inv,
            neighbours: n,
        } = *block;
        let width = self.defaults.border_width;
        let border = self.defaults.border;
        for x in x0..=x1 {
            let east_edge = edges[1] && n[NORTHEAST] && x1 - x < width;
            let west_edge = edges[3] && n[EAST] && x - x0 < width;
            for y in y0..=y1 {
                let Some(i) = at(size, x, size - y - 1) else {
                    continue;
                };
                if east_edge || west_edge {
                    px[i] = border;
                } else {
                    let north_edge = edges[0] && n[NORTH] && y1 - y < width;
                    let south_edge = edges[2] && n[SOUTH] && y - y0 < width;
                    px[i] = if north_edge || south_edge {
                        border
                    } else {
                        self.shaded(px[i], a, inv)
                    };
                }
            }
        }
    }

    /// One rounded corner of a shaded block from the corner template,
    /// mirrored for the compass point `which`.
    pub(super) fn shade_corner(
        &mut self,
        px: &mut [i32],
        x: i32,
        y: i32,
        block: &ShadeBlock<'_>,
        which: usize,
    ) {
        let ShadeBlock {
            size,
            alpha: a,
            inverse: inv,
            ..
        } = *block;
        let corner = self.defaults.corner;
        for i in 0..corner {
            let ti = if which == NORTHWEST || which == SOUTHWEST {
                corner - i - 1
            } else {
                i
            };
            for j in 0..corner {
                let tj = if which == SOUTHWEST || which == WEST {
                    corner - j - 1
                } else {
                    j
                };
                let cell = CORNER_TEMPLATE
                    .get(ti as usize)
                    .and_then(|row| row.get(tj as usize))
                    .copied()
                    .unwrap_or(0);
                let Some(p) = at(size, x + i, size - y - j - 1) else {
                    continue;
                };
                if cell == 1 {
                    px[p] = self.shaded(px[p], a, inv);
                } else if cell == 2 {
                    px[p] = self.defaults.border;
                }
            }
        }
    }
}

/// `size * row + x` when inside the sprite.
pub(super) fn at(size: i32, x: i32, row: i32) -> Option<usize> {
    (x >= 0 && row >= 0 && x < size && row < size).then(|| (size * row + x) as usize)
}

/// Fills the tile rectangle `(x0..=x1, y0..=y1)` with `colour`.
pub(super) fn fill(px: &mut [i32], x0: i32, x1: i32, y0: i32, y1: i32, size: i32, colour: i32) {
    for x in x0..=x1 {
        for y in y0..=y1 {
            if let Some(i) = at(size, x, size - y - 1) {
                px[i] = colour;
            }
        }
    }
}

/// A run of `len` pixels along row `size - y - 1`.
pub(super) fn row(px: &mut [i32], x: i32, y: i32, len: i32, size: i32, colour: i32) {
    for i in 0..len {
        if let Some(p) = at(size, x + i, size - y - 1) {
            px[p] = colour;
        }
    }
}

/// A run of `len` pixels down column `x` from row `size - y - 1`.
pub(super) fn column(px: &mut [i32], x: i32, y: i32, len: i32, size: i32, colour: i32) {
    for i in 0..len {
        if let Some(p) = at(size, x, size - y - 1 + i) {
            px[p] = colour;
        }
    }
}

/// A shaped overlay over the tile rectangle `rect` (`[x0, x1, y0, y1]`),
/// scaling the `shape_size` mask; mode 1 alpha-blends both colours.
#[derive(Clone, Copy)]
pub(super) struct ShapedOverlay<'a> {
    pub(super) rect: [i32; 4],
    pub(super) under: i32,
    pub(super) over: i32,
    pub(super) mask: &'a [u8],
    pub(super) shape_size: i32,
    pub(super) mode: i32,
}

/// Paints `overlay` into the `size * size` sprite pixels.
pub(super) fn shaped(px: &mut [i32], size: i32, overlay: &ShapedOverlay<'_>) {
    let ShapedOverlay {
        rect: [x0, x1, y0, y1],
        under,
        over,
        mask,
        shape_size,
        mode,
    } = *overlay;
    let width = x1 - x0 + 1;
    let height = y1 - y0 + 1;
    let step_x = (shape_size << 16) / width;
    let step_y = ((mask.len() as i32 / shape_size) << 16) / height;
    let mut at_px = (size - y1 - 1) * size + x0;
    let under_alpha = under >> 24;
    let over_alpha = over >> 24;
    let mut v = 0;
    let opaque = mode == 0 || mode == 1 && under_alpha == 255 && over_alpha == 255;
    for _ in 0..height {
        let row_at = (v >> 16) * shape_size;
        let mut u = 0;
        for _ in 0..width {
            let covered = mask
                .get(((u >> 16) + row_at) as usize)
                .copied()
                .unwrap_or(0)
                != 0;
            if let Some(slot) = usize::try_from(at_px).ok().and_then(|i| px.get_mut(i)) {
                if opaque {
                    *slot = if covered { over } else { under };
                } else {
                    let c = if covered { over } else { under };
                    let a = (c as u32 >> 24) as i32;
                    let inverse = 255 - a;
                    let old = *slot;
                    *slot = ((c & 0xFF00FF)
                        .wrapping_mul(a)
                        .wrapping_add((old & 0xFF00FF).wrapping_mul(inverse))
                        & -16_711_936)
                        .wrapping_add(
                            (c & 0xFF00)
                                .wrapping_mul(a)
                                .wrapping_add((old & 0xFF00).wrapping_mul(inverse))
                                & 0xFF0000,
                        )
                        >> 8
                        | 0xFF00_0000u32 as i32;
                }
            }
            at_px += 1;
            u += step_x;
        }
        v += step_y;
        at_px += size - width;
    }
}
