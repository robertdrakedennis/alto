//! Normal and instanced landscape decoding, clearing and heightmap copying.

use anyhow::Context;

use crate::map;

use crate::protocol910::terrain::RegionCopy;

use crate::tileflags::SceneLevelTileFlags;

pub use rs910_config::landscape_packet::*;

use rs910_core::perlin::perlin;

use super::{MapLoader, TileRead};

impl<'a> MapLoader<'a> {
    // -- landscape decode ---------------------------------------------------

    /// Clear the landscape of every level over the given rectangle.
    pub fn clear_landscape(&mut self, x: i32, z: i32, w: i32, h: i32) {
        for level in 0..self.levels {
            self.clear_landscape_level(level, x, z, w, h);
        }
    }
    /// Fill a missing
    /// square from the plane below / the neighbouring columns.
    pub fn clear_landscape_level(&mut self, level: usize, x0: i32, z0: i32, w: i32, h: i32) {
        let (mx, mz) = (self.max_x as i32, self.max_z as i32);
        for z in z0..z0 + h {
            for x in x0..x0 + w {
                if x >= 0 && x < mx && z >= 0 && z < mz {
                    let v = if level > 0 {
                        self.level_heightmap[level - 1][self.vi(x as usize, z as usize)] - 960
                    } else {
                        0
                    };
                    let i = self.vi(x as usize, z as usize);
                    self.level_heightmap[level][i] = v;
                }
            }
        }
        if x0 > 0 && x0 < mx {
            for z in z0 + 1..z0 + h {
                if z >= 0 && z < mz {
                    let v = self.level_heightmap[level][self.vi(x0 as usize - 1, z as usize)];
                    let i = self.vi(x0 as usize, z as usize);
                    self.level_heightmap[level][i] = v;
                }
            }
        }
        if z0 > 0 && z0 < mz {
            for x in x0 + 1..x0 + w {
                if x >= 0 && x < mx {
                    let v = self.level_heightmap[level][self.vi(x as usize, z0 as usize - 1)];
                    let i = self.vi(x as usize, z0 as usize);
                    self.level_heightmap[level][i] = v;
                }
            }
        }
        if x0 < 0 || z0 < 0 || x0 >= mx || z0 >= mz {
            return;
        }
        let (x0u, z0u) = (x0 as usize, z0 as usize);
        let h = |lv: usize, x: usize, z: usize| self.level_heightmap[lv][x * (self.max_z + 1) + z];
        let v;
        if level == 0 {
            if x0 > 0 && h(0, x0u - 1, z0u) != 0 {
                v = Some(h(0, x0u - 1, z0u));
            } else if z0 > 0 && h(0, x0u, z0u - 1) != 0 {
                v = Some(h(0, x0u, z0u - 1));
            } else if x0 > 0 && z0 > 0 && h(0, x0u - 1, z0u - 1) != 0 {
                v = Some(h(0, x0u - 1, z0u - 1));
            } else {
                v = None;
            }
        } else if x0 > 0 && h(level - 1, x0u - 1, z0u) != h(level, x0u - 1, z0u) {
            v = Some(h(level, x0u - 1, z0u));
        } else if z0 > 0 && h(level - 1, x0u, z0u - 1) != h(level, x0u, z0u - 1) {
            v = Some(h(level, x0u, z0u - 1));
        } else if x0 > 0 && z0 > 0 && h(level - 1, x0u - 1, z0u - 1) != h(level, x0u - 1, z0u - 1) {
            v = Some(h(level, x0u - 1, z0u - 1));
        } else {
            v = None;
        }
        if let Some(v) = v {
            let i = self.vi(x0u, z0u);
            self.level_heightmap[level][i] = v;
        }
    }
    /// Read a whole 64x64 square at scene-local `(tile_x, tile_z)`, with absolute
    /// `(base_x + tile_x, base_z + tile_z)` feeding the perlin fallback.
    pub fn read_normal_landscape(
        &mut self,
        packet: &mut Packet<'_>,
        flags: &mut SceneLevelTileFlags,
        tile_x: i32,
        tile_z: i32,
        base_x: i32,
        base_z: i32,
    ) -> anyhow::Result<()> {
        rs910_core::profile::scope!("build landscape");
        let abs_x = tile_x + base_x;
        let abs_z = tile_z + base_z;
        for level in 0..self.levels {
            for x in 0..64 {
                for z in 0..64 {
                    self.read_landscape(
                        packet,
                        flags,
                        TileRead {
                            level,
                            tile: [tile_x + x, tile_z + z],
                            shift: [0, 0],
                            absolute: [abs_x + x, abs_z + z],
                            rotation: 0,
                            skip_store: false,
                        },
                    )
                    .with_context(|| format!("landscape level {level} tile {x},{z}"))?;
                }
            }
        }
        Ok(())
    }
    /// Copy one rotated 8x8 chunk out of a full square stream.
    pub fn read_region_landscape(
        &mut self,
        packet: &mut Packet<'_>,
        flags: &mut SceneLevelTileFlags,
        copy: RegionCopy,
    ) -> anyhow::Result<()> {
        let RegionCopy {
            level,
            tile_x,
            tile_z,
            src_level,
            src_chunk_x,
            src_chunk_z,
            rotation,
        } = copy;
        let cx = (src_chunk_x & 0x7) * 8;
        let cz = (src_chunk_z & 0x7) * 8;
        let abs_x = (src_chunk_x & !0x7) << 3;
        let abs_z = (src_chunk_z & !0x7) << 3;
        let (mut dx, mut dz) = (0, 0);
        if rotation == 1 {
            dz = 1;
        } else if rotation == 2 {
            dx = 1;
            dz = 1;
        } else if rotation == 3 {
            dx = 1;
        }
        for lv in 0..self.levels {
            for x in 0..64 {
                for z in 0..64 {
                    if src_level == lv && x >= cx && x <= cx + 8 && z >= cz && z <= cz + 8 {
                        let (tx, tz);
                        if cx + 8 == x || cz + 8 == z {
                            let (a, b) = rotate_edge(x - cx, z - cz, tile_x, tile_z, rotation);
                            tx = a;
                            tz = b;
                            self.read_landscape(
                                packet,
                                flags,
                                TileRead {
                                    level,
                                    tile: [tx, tz],
                                    shift: [0, 0],
                                    absolute: [abs_x + x, abs_z + z],
                                    rotation: 0,
                                    skip_store: true,
                                },
                            )?;
                        } else {
                            tx = tile_x + map::rotate_x(x & 0x7, z & 0x7, rotation);
                            tz = tile_z + map::rotate_z(x & 0x7, z & 0x7, rotation);
                            self.read_landscape(
                                packet,
                                flags,
                                TileRead {
                                    level,
                                    tile: [tx, tz],
                                    shift: [dx, dz],
                                    absolute: [abs_x + x, abs_z + z],
                                    rotation,
                                    skip_store: false,
                                },
                            )?;
                        }
                        if x == 63 || z == 63 {
                            let passes = if x == 63 && z == 63 { 3 } else { 1 };
                            for pass in 0..passes {
                                let (mut ex, mut ez) = (x, z);
                                if pass == 0 {
                                    ex = if x == 63 { 64 } else { x };
                                    ez = if z == 63 { 64 } else { z };
                                } else if pass == 1 {
                                    ex = 64;
                                } else {
                                    ez = 64;
                                }
                                let (ox, oz) =
                                    rotate_edge(ex - cx, ez - cz, tile_x, tile_z, rotation);
                                if ox >= 0
                                    && ox < self.max_x as i32
                                    && oz >= 0
                                    && oz < self.max_z as i32
                                {
                                    let src = self.level_heightmap[level]
                                        [self.vi((dx + tx) as usize, (dz + tz) as usize)];
                                    let i = self.vi(ox as usize, oz as usize);
                                    self.level_heightmap[level][i] = src;
                                }
                            }
                        }
                    } else {
                        self.read_landscape(
                            packet,
                            flags,
                            TileRead {
                                level: 0,
                                tile: [-1, -1],
                                shift: [0, 0],
                                absolute: [0, 0],
                                rotation: 0,
                                skip_store: false,
                            },
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
    /// Read one tile.
    pub fn read_landscape(
        &mut self,
        packet: &mut Packet<'_>,
        flags: &mut SceneLevelTileFlags,
        read: TileRead,
    ) -> anyhow::Result<()> {
        let TileRead {
            level,
            tile: [tile_x, tile_z],
            shift: [mut dx, mut dz],
            absolute: [abs_x, abs_z],
            rotation,
            skip_store,
        } = read;
        if rotation == 1 {
            dz = 1;
        } else if rotation == 2 {
            dx = 1;
            dz = 1;
        } else if rotation == 3 {
            dx = 1;
        }
        if tile_x < 0 || tile_x >= self.max_x as i32 || tile_z < 0 || tile_z >= self.max_z as i32 {
            let op = packet.g1()?;
            if op & 0x1 != 0 {
                packet.g1()?;
                packet.gsmart1or2()?;
            }
            if op & 0x2 != 0 {
                packet.pos += 1;
            }
            if op & 0x4 != 0 {
                packet.gsmart1or2()?;
            }
            if op & 0x8 != 0 {
                packet.g1()?;
            }
            return Ok(());
        }
        let (x, z) = (tile_x as usize, tile_z as usize);
        if !self.underwater && !skip_store {
            flags.set(level, x, z, 0);
        }
        let op = packet.g1()?;
        if op & 0x1 != 0 {
            if skip_store {
                packet.g1()?;
                packet.gsmart1or2()?;
            } else {
                let shape_byte = packet.g1()?;
                let id = packet.gsmart1or2()?;
                let ti = self.ti(x, z);
                self.level_tile_overlay_ids[level][ti] = id as i16;
                self.level_tile_overlay_shape[level][ti] = (shape_byte >> 2) as i8;
                self.level_tile_overlay_rotation[level][ti] = ((rotation + shape_byte) & 0x3) as i8;
            }
        }
        if op & 0x2 != 0 {
            if self.underwater || skip_store {
                packet.pos += 1;
            } else {
                let f = packet.g1b()?;
                flags.set(level, x, z, f);
            }
        }
        if op & 0x4 != 0 {
            if skip_store {
                packet.gsmart1or2()?;
            } else {
                let id = packet.gsmart1or2()?;
                let ti = self.ti(x, z);
                self.level_tile_underlay_ids[level][ti] = id as i16;
            }
        }
        let hx = (tile_x + dx) as usize;
        let hz = (tile_z + dz) as usize;
        let vi = self.vi(hx, hz);
        if op & 0x8 != 0 {
            let mut h = packet.g1()?;
            if self.underwater {
                self.level_heightmap[0][vi] = (h * 8) << 2;
            } else {
                if h == 1 {
                    h = 0;
                }
                if level == 0 {
                    self.level_heightmap[0][vi] = (-h * 8) << 2;
                } else {
                    self.level_heightmap[level][vi] =
                        self.level_heightmap[level - 1][vi] - ((h * 8) << 2);
                }
            }
        } else if self.underwater {
            self.level_heightmap[0][vi] = 0;
        } else if level == 0 {
            self.level_heightmap[0][vi] = (-perlin(abs_x + 932_731, abs_z + 556_238) * 8) << 2;
        } else {
            self.level_heightmap[level][vi] = self.level_heightmap[level - 1][vi] - 960;
        }
        Ok(())
    }
    /// Add `other` onto the level's heightmap.
    pub fn add_heightmap(&mut self, level: usize, other: &[i32]) {
        for (dst, src) in self.level_heightmap[level].iter_mut().zip(other) {
            *dst += *src;
        }
    }
}

/// Point rotation shared by every tile builder.
#[inline]
pub(super) fn rotate_point(px: i32, pz: i32, rot: i32) -> (i32, i32) {
    match rot {
        1 => (pz, 512 - px),
        2 => (512 - px, 512 - pz),
        3 => (512 - pz, px),
        _ => (px, pz),
    }
}

/// Edge-row placement in the region landscape read: the 9th row/column of a rotated chunk.
pub(super) fn rotate_edge(lx: i32, lz: i32, tile_x: i32, tile_z: i32, rotation: i32) -> (i32, i32) {
    match rotation {
        0 => (lx + tile_x, lz + tile_z),
        1 => (lz + tile_x, tile_z + 8 - lx),
        2 => (tile_x + 8 - lx, tile_z + 8 - lz),
        _ => (tile_x + 8 - lz, lx + tile_z),
    }
}
