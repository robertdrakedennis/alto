//! Floor hard shadows: the shadow rasteriser, the per-level shadow mask and the
//! glue that stamps tile and entity shadows into the floors.
//!
//! An upper floor level stamps a byte mask ("hard shadow") of its solid
//! tiles onto every level below, offset along the sun direction by the
//! height difference. Each tile is rasterised at texel resolution
//! (`tileSize >> SHADOW_TEXEL_SHIFT` = 16 texels per tile) into a
//! [`HardShadow`], then added (byte-wise, wrapping) into the lower level's
//! [`FloorHardShadows`] mask, which the GPU later samples as a shadow
//! texture (not part of this crate).
//!
//! Everything is `int` arithmetic: arithmetic `>>`, truncating `/`, wrapping
//! byte adds. The rasteriser's loop shapes are kept as they are.
//!
//! Loc entity hard shadows: every placed static entity with a hard shadow
//! rasterises its lit model along the sun ([`crate::gpumodel::GpuModel::hard_shadow`])
//! and stamps it into every level up to its occlude level ([`apply_shadow`];
//! placement is in `locs.rs`). A runtime loc change subtracts the same shadow
//! again ([`stamp_shadow`]). Both run on the build's [`FloorBuilder`] and on
//! the finished [`FloorGeometry`] through [`ShadowFloor`].

use crate::floor::{FloorBuilder, FloorGeometry, SunLighting};
use std::collections::HashSet;

/// `log2(32) = 5`, i.e. 32 fine units per shadow texel, 16 texels per
/// 512-unit tile.
pub const SHADOW_TEXEL_SHIFT: i32 = 5;

// ---------------------------------------------------------------------------
// HardShadow: the rasterised coverage mask
// ---------------------------------------------------------------------------

/// A `byte[w * h]` coverage mask with an origin and
/// size set by [`Self::set_bounds`].
#[derive(Clone, Debug)]
pub struct HardShadow {
    /// Origin x.
    pub origin_x: i32,
    /// Origin y (z).
    pub origin_y: i32,
    /// Width (`x1 - x0`), also the row stride.
    pub width: i32,
    /// Height (`y1 - y0`).
    pub height: i32,
    /// The mask, row-major (`width` per row).
    pub data: Vec<u8>,
}

impl HardShadow {
    /// An empty buffer of `w * h` bytes.
    #[must_use]
    pub fn new(w: i32, h: i32) -> Self {
        Self {
            origin_x: 0,
            origin_y: 0,
            width: 0,
            height: 0,
            data: vec![0; (w * h) as usize],
        }
    }

    /// Sets the origin and extent from texel bounds.
    pub fn set_bounds(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.origin_x = x0;
        self.origin_y = y0;
        self.width = x1 - x0;
        self.height = y1 - y0;
    }

    /// Does the buffer hold `w * h` bytes?
    #[must_use]
    pub fn fits(&self, w: i32, h: i32) -> bool {
        self.data.len() as i32 >= w * h
    }

    /// Zero the whole buffer.
    pub fn clear(&mut self) {
        self.data.fill(0);
    }

    /// Fixed-point (16.16)
    /// scanline rasteriser. Argument order is the three **y**s then the
    /// three **x**s. Rows are filled with [`fill_span`] at `width * y`.
    /// Every branch of the original is kept; only the loop shape is rewritten
    /// (`while(true){ n--; if (n < 0) {...} }` → counted loops).
    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    pub fn raster_triangle(
        &mut self,
        mut y0: i32,
        mut y1: i32,
        mut y2: i32,
        x0: i32,
        x1: i32,
        x2: i32,
    ) {
        // Edge slopes, 16.16 dx/dy.
        let mut s01 = 0;
        if y0 != y1 {
            s01 = ((x1 - x0) << 16) / (y1 - y0);
        }
        let mut s12 = 0;
        if y1 != y2 {
            s12 = ((x2 - x1) << 16) / (y2 - y1);
        }
        let mut s20 = 0;
        if y0 != y2 {
            s20 = ((x0 - x2) << 16) / (y0 - y2);
        }
        let w = self.width;
        let data = &mut self.data;
        if y0 <= y1 && y0 <= y2 {
            if y1 < y2 {
                // Y0 top, y1 middle, y2 bottom.
                let mut xa = x0 << 16; // long edge 0->2
                let mut xb = x0 << 16; // edge 0->1
                if y0 < 0 {
                    xa -= y0 * s20;
                    xb -= y0 * s01;
                    y0 = 0;
                }
                let mut xc = x1 << 16; // edge 1->2
                if y1 < 0 {
                    xc -= y1 * s12;
                    y1 = 0;
                }
                if (y0 == y1 || s20 >= s01) && (y0 != y1 || s20 <= s12) {
                    let mut lower = y2 - y1;
                    let mut upper = y1 - y0;
                    let mut row = w * y0;
                    loop {
                        upper -= 1;
                        if upper < 0 {
                            loop {
                                lower -= 1;
                                if lower < 0 {
                                    return;
                                }
                                fill_span(data, row, xc >> 16, xa >> 16);
                                xa += s20;
                                xc += s12;
                                row += w;
                            }
                        }
                        fill_span(data, row, xb >> 16, xa >> 16);
                        xa += s20;
                        xb += s01;
                        row += w;
                    }
                } else {
                    let mut lower = y2 - y1;
                    let mut upper = y1 - y0;
                    let mut row = w * y0;
                    loop {
                        upper -= 1;
                        if upper < 0 {
                            loop {
                                lower -= 1;
                                if lower < 0 {
                                    return;
                                }
                                fill_span(data, row, xa >> 16, xc >> 16);
                                xa += s20;
                                xc += s12;
                                row += w;
                            }
                        }
                        fill_span(data, row, xa >> 16, xb >> 16);
                        xa += s20;
                        xb += s01;
                        row += w;
                    }
                }
            } else {
                // Y0 top, y2 middle, y1 bottom.
                let mut xa = x0 << 16; // edge 0->2
                let mut xb = x0 << 16; // long edge 0->1
                if y0 < 0 {
                    xa -= y0 * s20;
                    xb -= y0 * s01;
                    y0 = 0;
                }
                let mut xc = x2 << 16; // edge 2->1
                if y2 < 0 {
                    xc -= y2 * s12;
                    y2 = 0;
                }
                if (y0 == y2 || s20 >= s01) && (y0 != y2 || s12 <= s01) {
                    let mut lower = y1 - y2;
                    let mut upper = y2 - y0;
                    let mut row = w * y0;
                    loop {
                        upper -= 1;
                        if upper < 0 {
                            loop {
                                lower -= 1;
                                if lower < 0 {
                                    return;
                                }
                                fill_span(data, row, xb >> 16, xc >> 16);
                                xc += s12;
                                xb += s01;
                                row += w;
                            }
                        }
                        fill_span(data, row, xb >> 16, xa >> 16);
                        xa += s20;
                        xb += s01;
                        row += w;
                    }
                } else {
                    let mut lower = y1 - y2;
                    let mut upper = y2 - y0;
                    let mut row = w * y0;
                    loop {
                        upper -= 1;
                        if upper < 0 {
                            loop {
                                lower -= 1;
                                if lower < 0 {
                                    return;
                                }
                                fill_span(data, row, xc >> 16, xb >> 16);
                                xc += s12;
                                xb += s01;
                                row += w;
                            }
                        }
                        fill_span(data, row, xa >> 16, xb >> 16);
                        xa += s20;
                        xb += s01;
                        row += w;
                    }
                }
            }
        } else if y1 <= y2 {
            if y2 < y0 {
                // Y1 top, y2 middle, y0 bottom.
                let mut xa = x1 << 16; // long edge 1->0
                let mut xb = x1 << 16; // edge 1->2
                if y1 < 0 {
                    xa -= y1 * s01;
                    xb -= y1 * s12;
                    y1 = 0;
                }
                let mut xc = x2 << 16; // edge 2->0
                if y2 < 0 {
                    xc -= y2 * s20;
                    y2 = 0;
                }
                if (y1 != y2 && s01 < s12) || (y1 == y2 && s01 > s20) {
                    let mut lower = y0 - y2;
                    let mut upper = y2 - y1;
                    let mut row = w * y1;
                    loop {
                        upper -= 1;
                        if upper < 0 {
                            loop {
                                lower -= 1;
                                if lower < 0 {
                                    return;
                                }
                                fill_span(data, row, xa >> 16, xc >> 16);
                                xa += s01;
                                xc += s20;
                                row += w;
                            }
                        }
                        fill_span(data, row, xa >> 16, xb >> 16);
                        xa += s01;
                        xb += s12;
                        row += w;
                    }
                } else {
                    let mut lower = y0 - y2;
                    let mut upper = y2 - y1;
                    let mut row = w * y1;
                    loop {
                        upper -= 1;
                        if upper < 0 {
                            loop {
                                lower -= 1;
                                if lower < 0 {
                                    return;
                                }
                                fill_span(data, row, xc >> 16, xa >> 16);
                                xa += s01;
                                xc += s20;
                                row += w;
                            }
                        }
                        fill_span(data, row, xb >> 16, xa >> 16);
                        xa += s01;
                        xb += s12;
                        row += w;
                    }
                }
            } else {
                // Y1 top, y0 middle, y2 bottom.
                let mut xa = x1 << 16; // edge 1->0
                let mut xb = x1 << 16; // long edge 1->2
                if y1 < 0 {
                    xa -= y1 * s01;
                    xb -= y1 * s12;
                    y1 = 0;
                }
                let mut xc = x0 << 16; // edge 0->2
                if y0 < 0 {
                    xc -= y0 * s20;
                    y0 = 0;
                }
                if s01 < s12 {
                    let mut lower = y2 - y0;
                    let mut upper = y0 - y1;
                    let mut row = w * y1;
                    loop {
                        upper -= 1;
                        if upper < 0 {
                            loop {
                                lower -= 1;
                                if lower < 0 {
                                    return;
                                }
                                fill_span(data, row, xc >> 16, xb >> 16);
                                xc += s20;
                                xb += s12;
                                row += w;
                            }
                        }
                        fill_span(data, row, xa >> 16, xb >> 16);
                        xa += s01;
                        xb += s12;
                        row += w;
                    }
                } else {
                    let mut lower = y2 - y0;
                    let mut upper = y0 - y1;
                    let mut row = w * y1;
                    loop {
                        upper -= 1;
                        if upper < 0 {
                            loop {
                                lower -= 1;
                                if lower < 0 {
                                    return;
                                }
                                fill_span(data, row, xb >> 16, xc >> 16);
                                xc += s20;
                                xb += s12;
                                row += w;
                            }
                        }
                        fill_span(data, row, xb >> 16, xa >> 16);
                        xa += s01;
                        xb += s12;
                        row += w;
                    }
                }
            }
        } else if y0 < y1 {
            // Y2 top, y0 middle, y1 bottom.
            let mut xa = x2 << 16; // long edge 2->1
            let mut xb = x2 << 16; // edge 2->0
            if y2 < 0 {
                xa -= y2 * s12;
                xb -= y2 * s20;
                y2 = 0;
            }
            let mut xc = x0 << 16; // edge 0->1
            if y0 < 0 {
                xc -= y0 * s01;
                y0 = 0;
            }
            if s12 < s20 {
                let mut lower = y1 - y0;
                let mut upper = y0 - y2;
                let mut row = w * y2;
                loop {
                    upper -= 1;
                    if upper < 0 {
                        loop {
                            lower -= 1;
                            if lower < 0 {
                                return;
                            }
                            fill_span(data, row, xa >> 16, xc >> 16);
                            xa += s12;
                            xc += s01;
                            row += w;
                        }
                    }
                    fill_span(data, row, xa >> 16, xb >> 16);
                    xa += s12;
                    xb += s20;
                    row += w;
                }
            } else {
                let mut lower = y1 - y0;
                let mut upper = y0 - y2;
                let mut row = w * y2;
                loop {
                    upper -= 1;
                    if upper < 0 {
                        loop {
                            lower -= 1;
                            if lower < 0 {
                                return;
                            }
                            fill_span(data, row, xc >> 16, xa >> 16);
                            xa += s12;
                            xc += s01;
                            row += w;
                        }
                    }
                    fill_span(data, row, xb >> 16, xa >> 16);
                    xa += s12;
                    xb += s20;
                    row += w;
                }
            }
        } else {
            // Y2 top, y1 middle, y0 bottom.
            let mut xa = x2 << 16; // edge 2->1
            let mut xb = x2 << 16; // long edge 2->0
            if y2 < 0 {
                xa -= y2 * s12;
                xb -= y2 * s20;
                y2 = 0;
            }
            let mut xc = x1 << 16; // edge 1->0
            if y1 < 0 {
                xc -= y1 * s01;
                y1 = 0;
            }
            if s12 < s20 {
                let mut lower = y0 - y1;
                let mut upper = y1 - y2;
                let mut row = w * y2;
                loop {
                    upper -= 1;
                    if upper < 0 {
                        loop {
                            lower -= 1;
                            if lower < 0 {
                                return;
                            }
                            fill_span(data, row, xc >> 16, xb >> 16);
                            xc += s01;
                            xb += s20;
                            row += w;
                        }
                    }
                    fill_span(data, row, xa >> 16, xb >> 16);
                    xa += s12;
                    xb += s20;
                    row += w;
                }
            } else {
                let mut lower = y0 - y1;
                let mut upper = y1 - y2;
                let mut row = w * y2;
                loop {
                    upper -= 1;
                    if upper < 0 {
                        loop {
                            lower -= 1;
                            if lower < 0 {
                                return;
                            }
                            fill_span(data, row, xb >> 16, xc >> 16);
                            xc += s01;
                            xb += s20;
                            row += w;
                        }
                    }
                    fill_span(data, row, xb >> 16, xa >> 16);
                    xa += s12;
                    xb += s20;
                    row += w;
                }
            }
        }
    }
}

/// Sets `buf[row_offset + x0 .. row_offset + x1]` to 1 (no-op when
/// `x0 >= x1`).
pub fn fill_span(buf: &mut [u8], row_offset: i32, x0: i32, x1: i32) {
    if x0 >= x1 {
        return;
    }
    let start = (row_offset + x0) as usize;
    let end = (row_offset + x1) as usize;
    buf[start..end].fill(1);
}

// ---------------------------------------------------------------------------
// FloorHardShadows: the per-level mask
// ---------------------------------------------------------------------------

/// One level's shadow mask, `width x height` bytes with a 1-texel border on
/// every side.
#[derive(Clone, Debug)]
pub struct FloorHardShadows {
    /// Mask width in texels (`(tilesX * tileSize >> shift) + 2`).
    pub width: i32,
    /// Mask height in texels.
    pub height: i32,
    /// The mask, index `width * z + x`.
    pub mask: Vec<u8>,
    /// `shift + 7 - tileShift` (tiles per 128-texel GPU block, log2).
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub block_shift: i32,
    /// GPU blocks along X.
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub blocks_x: i32,
    /// GPU blocks along Z.
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub blocks_z: i32,
}

impl FloorHardShadows {
    /// `FloorHardShadows(toolkit, floor)`.
    #[must_use]
    pub fn new(tiles_x: usize, tiles_z: usize, tile_size: i32, tile_shift: i32) -> Self {
        let width = ((tiles_x as i32 * tile_size) >> SHADOW_TEXEL_SHIFT) + 2;
        let height = ((tiles_z as i32 * tile_size) >> SHADOW_TEXEL_SHIFT) + 2;
        let block_shift = SHADOW_TEXEL_SHIFT + 7 - tile_shift;
        Self {
            width,
            height,
            mask: vec![0; (width * height) as usize],
            block_shift,
            blocks_x: tiles_x as i32 >> block_shift,
            blocks_z: tiles_z as i32 >> block_shift,
        }
    }

    /// Set texel count (diagnostics).
    #[must_use]
    #[cfg(test)] // test-only diagnostics setter
    pub fn set_count(&self) -> usize {
        self.mask.iter().filter(|&&b| b != 0).count()
    }

    /// Clips a shadow at `(x, z)` against the mask. Returns the rectangle to
    /// copy or `None` when nothing is left.
    fn clip(&self, shadow: &HardShadow, x: i32, z: i32) -> Option<RowSpan> {
        let mut x0 = shadow.origin_x + 1 + x;
        let mut z0 = shadow.origin_y + 1 + z;
        let mut dst = self.width * z0 + x0;
        let mut src = 0;
        let mut h = shadow.height;
        let mut w = shadow.width;
        let mut dst_skip = self.width - w;
        let mut src_skip = 0;
        if z0 <= 0 {
            let cut = 1 - z0;
            h -= cut;
            src += w * cut;
            dst += self.width * cut;
            z0 = 1;
        }
        if z0 + h >= self.height {
            let cut = z0 + h + 1 - self.height;
            h -= cut;
        }
        if x0 <= 0 {
            let cut = 1 - x0;
            w -= cut;
            src += cut;
            dst += cut;
            src_skip += cut;
            dst_skip += cut;
            x0 = 1;
        }
        if x0 + w >= self.width {
            let cut = x0 + w + 1 - self.width;
            w -= cut;
            src_skip += cut;
            dst_skip += cut;
        }
        if w > 0 && h > 0 {
            Some(RowSpan {
                src_offset: src,
                dst_offset: dst,
                width: w,
                height: h,
                dst_skip,
                src_skip,
            })
        } else {
            None
        }
    }

    /// Adds the shadow's bytes into the mask at texel offset `(x, z)`,
    /// clipped. (The GPU block dirtying is not needed here.)
    pub fn add(&mut self, shadow: &HardShadow, x: i32, z: i32) {
        if let Some(span) = self.clip(shadow, x, z) {
            add_rows(&mut self.mask, &shadow.data, span);
        }
    }

    /// Subtract variant of [`Self::add`].
    pub fn sub(&mut self, shadow: &HardShadow, x: i32, z: i32) {
        if let Some(span) = self.clip(shadow, x, z) {
            sub_rows(&mut self.mask, &shadow.data, span);
        }
    }

    /// Is any sampled texel under the (clipped) shadow rectangle still zero?
    /// Samples every 8th texel plus the last column of each sampled row.
    #[must_use]
    pub fn test(&self, shadow: &HardShadow, x: i32, z: i32) -> bool {
        let mut x0 = shadow.origin_x + 1 + x;
        let mut z0 = shadow.origin_y + 1 + z;
        let mut dst = self.width * z0 + x0;
        let mut h = shadow.height;
        let mut w = shadow.width;
        let mut dst_skip = self.width - w;
        if z0 <= 0 {
            let cut = 1 - z0;
            h -= cut;
            dst += self.width * cut;
            z0 = 1;
        }
        if z0 + h >= self.height {
            let cut = z0 + h + 1 - self.height;
            h -= cut;
        }
        if x0 <= 0 {
            let cut = 1 - x0;
            w -= cut;
            dst += cut;
            dst_skip += cut;
            x0 = 1;
        }
        if x0 + w >= self.width {
            let cut = x0 + w + 1 - self.width;
            w -= cut;
            dst_skip += cut;
        }
        if w > 0 && h > 0 {
            let step = 8;
            let row_skip = (step - 1) * self.width + dst_skip;
            test_rows(&self.mask, dst, w, h, row_skip, step)
        } else {
            false
        }
    }
}

/// A rectangle to copy between two byte grids: where it starts in each, its
/// size, and how far to skip after each row in each.
#[derive(Clone, Copy, Debug)]
pub struct RowSpan {
    pub src_offset: i32,
    pub dst_offset: i32,
    pub width: i32,
    pub height: i32,
    pub dst_skip: i32,
    pub src_skip: i32,
}

/// `dst[i] += src[j]` over the rectangle, with byte wrap-around.
pub fn add_rows(dst: &mut [u8], src: &[u8], span: RowSpan) {
    let RowSpan {
        src_offset: mut s,
        dst_offset: mut d,
        width,
        height,
        dst_skip,
        src_skip,
    } = span;
    for _ in 0..height {
        for _ in 0..width {
            dst[d as usize] = dst[d as usize].wrapping_add(src[s as usize]);
            d += 1;
            s += 1;
        }
        d += dst_skip;
        s += src_skip;
    }
}

/// `dst[i] -= src[j]` over the rectangle, with byte wrap-around.
pub fn sub_rows(dst: &mut [u8], src: &[u8], span: RowSpan) {
    let RowSpan {
        src_offset: mut s,
        dst_offset: mut d,
        width,
        height,
        dst_skip,
        src_skip,
    } = span;
    for _ in 0..height {
        for _ in 0..width {
            dst[d as usize] = dst[d as usize].wrapping_sub(src[s as usize]);
            d += 1;
            s += 1;
        }
        d += dst_skip;
        s += src_skip;
    }
}

/// Is any sampled texel of the `w x h` rectangle at `off` zero? It samples
/// every `step`th texel of every `step`th row, plus the last column of each
/// sampled row.
pub fn test_rows(mask: &[u8], mut off: i32, w: i32, h: i32, row_skip: i32, step: i32) -> bool {
    let rem = w % step;
    let back = if rem == 0 { 0 } else { step - rem };
    let rows = (h + step - 1) / step;
    let cols = (w + step - 1) / step;
    for _ in 0..rows {
        for _ in 0..cols {
            if mask[off as usize] == 0 {
                return true;
            }
            off += step;
        }
        let end = off - back;
        if mask[(end - 1) as usize] == 0 {
            return true;
        }
        off = row_skip + end;
    }
    false
}

// ---------------------------------------------------------------------------
// Floor glue
// ---------------------------------------------------------------------------

/// Rasterises tile `(x, z)`'s triangles into a `16 x 16` [`HardShadow`], or
/// `None` when the tile does not cast.
/// `reuse` is handed back (cleared) when it is big enough.
#[must_use]
pub fn tile_shadow(
    floor: &FloorBuilder,
    x: usize,
    z: usize,
    reuse: Option<HardShadow>,
) -> Option<HardShadow> {
    if !floor.tile_casts_hard_shadow(x, z) {
        // The reused buffer is dropped too: the caller assigns the result
        // back over it.
        drop(reuse);
        return None;
    }
    let size = floor.heights.tile_size >> SHADOW_TEXEL_SHIFT;
    let mut shadow = match reuse {
        Some(mut s) if s.fits(size, size) => {
            s.clear();
            s
        }
        _ => HardShadow::new(size, size),
    };
    shadow.set_bounds(0, 0, size, size);
    raster_tile(floor, &mut shadow, x, z);
    Some(shadow)
}

/// Rasterises every triangle of the tile (fine units `>> SHADOW_TEXEL_SHIFT`)
/// whose signed area is positive into `shadow`.
pub fn raster_tile(floor: &FloorBuilder, shadow: &mut HardShadow, x: usize, z: usize) {
    let Some((xs, zs)) = floor.tile_vertex_xz(x, z) else {
        return;
    };
    let n = xs.len();
    let tx: Vec<i32> = xs.iter().map(|&v| v >> SHADOW_TEXEL_SHIFT).collect();
    let tz: Vec<i32> = zs.iter().map(|&v| v >> SHADOW_TEXEL_SHIFT).collect();
    let mut i = 0;
    while i < n {
        let (x0, z0) = (tx[i], tz[i]);
        let (x1, z1) = (tx[i + 1], tz[i + 1]);
        let (x2, z2) = (tx[i + 2], tz[i + 2]);
        i += 3;
        if (x0 - x1) * (z1 - z2) - (z1 - z0) * (x2 - x1) > 0 {
            shadow.raster_triangle(z0, z1, z2, x0, x1, x2);
        }
    }
}

/// The texel offset of a shadow cast from fine `(x, z)` at height difference
/// `dy`: `(x - (sun offset * dy >> 8)) >> SHADOW_TEXEL_SHIFT`.
#[must_use]
pub fn shadow_texel(sun: &SunLighting, x: i32, dy: i32, z: i32) -> (i32, i32) {
    (
        (x - ((sun.shadow_offset_x * dy) >> 8)) >> SHADOW_TEXEL_SHIFT,
        (z - ((sun.shadow_offset_z * dy) >> 8)) >> SHADOW_TEXEL_SHIFT,
    )
}

/// A floor a hard shadow is stamped into: the map build's [`FloorBuilder`]
/// or the uploaded [`FloorGeometry`] (the normal height map of a level, in
/// both phases).
pub trait ShadowFloor {
    /// The fine height at `(x, z)`.
    fn fine_height(&self, x: i32, z: i32) -> i32;
    /// The hard-shadow mask, `None` without hard shadows.
    fn shadow_mask_mut(&mut self) -> Option<&mut FloorHardShadows>;
}

impl ShadowFloor for FloorBuilder {
    fn fine_height(&self, x: i32, z: i32) -> i32 {
        self.heights.get_fine_height(x, z)
    }
    fn shadow_mask_mut(&mut self) -> Option<&mut FloorHardShadows> {
        self.hard_shadows_mut()
    }
}

impl ShadowFloor for FloorGeometry {
    fn fine_height(&self, x: i32, z: i32) -> i32 {
        self.heights.get_fine_height(x, z)
    }
    fn shadow_mask_mut(&mut self) -> Option<&mut FloorHardShadows> {
        self.hard_shadows.as_mut()
    }
}

/// Adds `shadow` into this floor's mask, displaced along the sun by `dy`.
/// No-op when the floor has no mask. Returns the
/// texel offset used, for GPU block dirtying.
pub fn stamp<F: ShadowFloor>(
    floor: &mut F,
    shadow: &HardShadow,
    x: i32,
    dy: i32,
    z: i32,
    sun: &SunLighting,
) -> Option<(i32, i32)> {
    let (tx, tz) = shadow_texel(sun, x, dy, z);
    let mask = floor.shadow_mask_mut()?;
    mask.add(shadow, tx, tz);
    Some((tx, tz))
}

/// Subtract variant of [`stamp`], reached from [`stamp_shadow`].
pub fn unstamp<F: ShadowFloor>(
    floor: &mut F,
    shadow: &HardShadow,
    x: i32,
    dy: i32,
    z: i32,
    sun: &SunLighting,
) -> Option<(i32, i32)> {
    let (tx, tz) = shadow_texel(sun, x, dy, z);
    let mask = floor.shadow_mask_mut()?;
    mask.sub(shadow, tx, tz);
    Some((tx, tz))
}

/// The 128-texel GPU blocks of `FloorHardShadows` a shadow stamped at texel
/// `(tx, tz)` touches, with the one-texel fringe the shadow pass filters
/// across.
pub fn mark_dirty_blocks(
    shadow: &HardShadow,
    tx: i32,
    tz: i32,
    dirty: &mut HashSet<(usize, usize)>,
) {
    for bz in ((tz + shadow.origin_y - 1).max(0) / 128)
        ..=((tz + shadow.origin_y + shadow.height).max(0) / 128)
    {
        for bx in ((tx + shadow.origin_x - 1).max(0) / 128)
            ..=((tx + shadow.origin_x + shadow.width).max(0) / 128)
        {
            dirty.insert((bx as usize, bz as usize));
        }
    }
}

/// Test variant of [`stamp`];
/// `false` when the floor has no mask.
#[allow(dead_code)]
#[must_use]
pub fn test_stamp(
    floor: &FloorBuilder,
    shadow: &HardShadow,
    x: i32,
    dy: i32,
    z: i32,
    sun: &SunLighting,
) -> bool {
    let (tx, tz) = shadow_texel(sun, x, dy, z);
    floor
        .hard_shadows()
        .is_some_and(|mask| mask.test(shadow, tx, tz))
}

// ---------------------------------------------------------------------------
// Scene.sweepShadows
// ---------------------------------------------------------------------------

/// `Scene.sweepShadows(1, maxLevel)`, as called by
/// `buildShadows()`: for every level `>= 1`, every tile
/// (z outer, x inner) that casts a hard shadow is rasterised once and
/// stamped onto every lower level, displaced by the mean of the four
/// corner height differences (`int` division).
pub fn sweep_shadows(builders: &mut [Option<FloorBuilder>], sun: &SunLighting) {
    let levels = builders.len();
    let mut reuse: Option<HardShadow> = None;
    for level in 1..levels {
        if builders[level].is_none() {
            continue;
        }
        let (max_x, max_z, shift) = {
            let f = builders[level].as_ref().expect("checked");
            (f.heights.tiles_x, f.heights.tiles_z, f.heights.shift)
        };
        for z in 0..max_z {
            for x in 0..max_x {
                let upper = builders[level].as_ref().expect("checked");
                reuse = tile_shadow(upper, x, z, reuse.take());
                let Some(shadow) = reuse.as_ref() else {
                    continue;
                };
                let fx = (x as i32) << shift;
                let fz = (z as i32) << shift;
                let uh = &upper.heights;
                let corners = [
                    uh.get_tile_height(x, z),
                    uh.get_tile_height(x + 1, z),
                    uh.get_tile_height(x + 1, z + 1),
                    uh.get_tile_height(x, z + 1),
                ];
                let head = &mut builders[..level];
                for lower_level in (0..level).rev() {
                    let Some(lower) = head[lower_level].as_mut() else {
                        continue;
                    };
                    let lh = &lower.heights;
                    let d0 = corners[0] - lh.get_tile_height(x, z);
                    let d1 = corners[1] - lh.get_tile_height(x + 1, z);
                    let d2 = corners[2] - lh.get_tile_height(x + 1, z + 1);
                    let d3 = corners[3] - lh.get_tile_height(x, z + 1);
                    let dy = (d0 + d1 + d2 + d3) / 4;
                    stamp(lower, shadow, fx, dy, fz, sun);
                }
            }
        }
    }
}

/// The hard-shadow mask of one level as the draw needs it: size, mask and
/// texel shift.
#[derive(Clone, Copy)]
pub struct ShadowMaskView<'a> {
    /// Texels along X (`tilesX * 512 >> texel_shift + 2`).
    pub width: usize,
    /// Texels along Z.
    #[allow(dead_code, reason = "no reader yet")]
    pub height: usize,
    /// The mask, row-major by Z (`width * z + x`).
    pub mask: &'a [u8],
    /// 5 in the client: 32 fine units per texel.
    pub texel_shift: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(s: &HardShadow) -> Vec<String> {
        (0..s.height)
            .map(|y| {
                (0..s.width)
                    .map(|x| {
                        if s.data[(y * s.width + x) as usize] != 0 {
                            '#'
                        } else {
                            '.'
                        }
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn right_triangle_16() {
        // Tile corner triangle (0,0)-(16,0)-(0,16) in a 16x16 mask, the
        // shape a flat tile's first triangle gives.
        let mut s = HardShadow::new(16, 16);
        s.set_bounds(0, 0, 16, 16);
        // y0=0,y1=0,y2=16 / x0=0,x1=16,x2=0
        s.raster_triangle(0, 0, 16, 0, 16, 0);
        let r = rows(&s);
        assert_eq!(r[0], "################");
        assert_eq!(r[1], "###############.");
        assert_eq!(r[8], "########........");
        assert_eq!(r[15], "#...............");
        // Rows 16 - y set: 16 + 15 + ... + 1 = 136.
        assert_eq!(s.data.iter().filter(|&&b| b != 0).count(), 136);
    }

    #[test]
    fn opposite_triangle_covers_rest() {
        // The other half (16,0)-(16,16)-(0,16): together the two halves fill
        // the tile exactly once (no double coverage, no gaps).
        let mut a = HardShadow::new(16, 16);
        a.set_bounds(0, 0, 16, 16);
        a.raster_triangle(0, 0, 16, 0, 16, 0);
        let mut b = HardShadow::new(16, 16);
        b.set_bounds(0, 0, 16, 16);
        b.raster_triangle(0, 16, 16, 16, 16, 0);
        let sum: Vec<u8> = a.data.iter().zip(&b.data).map(|(x, y)| x + y).collect();
        assert!(
            sum.iter().all(|&v| v == 1),
            "{:?}\n{:?}",
            rows(&a),
            rows(&b)
        );
    }

    #[test]
    fn all_orderings_same_area() {
        // Every vertex order of the same triangle covers the same texels.
        let tri = [(2, 1), (14, 5), (6, 13)];
        let orders = [
            [0, 1, 2],
            [1, 2, 0],
            [2, 0, 1],
            [0, 2, 1],
            [2, 1, 0],
            [1, 0, 2],
        ];
        let mut first: Option<Vec<u8>> = None;
        for o in orders {
            let mut s = HardShadow::new(16, 16);
            s.set_bounds(0, 0, 16, 16);
            let (x0, y0) = tri[o[0]];
            let (x1, y1) = tri[o[1]];
            let (x2, y2) = tri[o[2]];
            s.raster_triangle(y0, y1, y2, x0, x1, x2);
            assert!(s.data.iter().filter(|&&b| b != 0).count() > 30);
            match &first {
                None => first = Some(s.data.clone()),
                Some(f) => assert_eq!(f, &s.data, "order {o:?}\n{:?}", rows(&s)),
            }
        }
    }

    #[test]
    fn negative_top_is_clipped() {
        let mut s = HardShadow::new(16, 16);
        s.set_bounds(0, 0, 16, 16);
        s.raster_triangle(-8, -8, 8, 0, 16, 0);
        let r = rows(&s);
        assert_eq!(r[0], "########........");
        assert_eq!(r[7], "#...............");
        assert_eq!(r[8], "................");
    }

    #[test]
    fn degenerate_triangle_sets_nothing() {
        let mut s = HardShadow::new(16, 16);
        s.set_bounds(0, 0, 16, 16);
        s.raster_triangle(3, 3, 3, 0, 8, 16);
        assert!(s.data.iter().all(|&b| b == 0));
    }

    fn full_shadow() -> HardShadow {
        let mut s = HardShadow::new(16, 16);
        s.set_bounds(0, 0, 16, 16);
        s.data.fill(1);
        s
    }

    fn set_texels(m: &FloorHardShadows) -> Vec<(i32, i32)> {
        let mut v = Vec::new();
        for z in 0..m.height {
            for x in 0..m.width {
                if m.mask[(m.width * z + x) as usize] != 0 {
                    v.push((x, z));
                }
            }
        }
        v
    }

    #[test]
    fn mask_dimensions() {
        let m = FloorHardShadows::new(104, 104, 512, 9);
        assert_eq!((m.width, m.height), (1666, 1666));
        assert_eq!(m.block_shift, 3);
        assert_eq!((m.blocks_x, m.blocks_z), (13, 13));
    }

    #[test]
    fn add_inside() {
        // 2x2 tiles -> 34x34 mask (1-texel border). Stamp at texel (0,0)
        // lands at mask (1,1)..(16,16).
        let mut m = FloorHardShadows::new(2, 2, 512, 9);
        assert_eq!((m.width, m.height), (34, 34));
        m.add(&full_shadow(), 0, 0);
        let t = set_texels(&m);
        assert_eq!(t.len(), 256);
        assert_eq!(t[0], (1, 1));
        assert_eq!(t[255], (16, 16));
        // Adding again wraps bytes: 1 + 1 = 2 (still set).
        m.add(&full_shadow(), 0, 0);
        assert_eq!(m.mask[(34 + 1) as usize], 2);
        // Subtracting twice returns to zero.
        m.sub(&full_shadow(), 0, 0);
        m.sub(&full_shadow(), 0, 0);
        assert_eq!(m.set_count(), 0);
    }

    #[test]
    fn add_clips_top_left() {
        let mut m = FloorHardShadows::new(2, 2, 512, 9);
        // Shadow origin at texel (-5, -3): mask x 1-5+1 = -4.. -> clipped to
        // column 1, rows clipped to 1. Visible: x 1..=11 (11 cols) by z
        // 1..=13 (13 rows).
        m.add(&full_shadow(), -5, -3);
        let t = set_texels(&m);
        assert_eq!(t.len(), 11 * 13);
        assert_eq!(t[0], (1, 1));
        assert_eq!(*t.last().unwrap(), (11, 13));
        // Border column/row 0 never written.
        assert!(t.iter().all(|&(x, z)| x >= 1 && z >= 1));
    }

    #[test]
    fn add_clips_bottom_right() {
        let mut m = FloorHardShadows::new(2, 2, 512, 9);
        // Origin texel (25, 28): mask x 26..=41 -> clipped to 26..=32, z
        // 29..=44 -> clipped to 29..=32 (width/height 34 keeps a border
        // at 33).
        m.add(&full_shadow(), 25, 28);
        let t = set_texels(&m);
        assert_eq!(t.len(), 7 * 4);
        assert_eq!(t[0], (26, 29));
        assert_eq!(*t.last().unwrap(), (32, 32));
    }

    #[test]
    fn add_fully_outside_is_noop() {
        let mut m = FloorHardShadows::new(2, 2, 512, 9);
        m.add(&full_shadow(), 40, 0);
        m.add(&full_shadow(), 0, -40);
        m.add(&full_shadow(), -17, 0);
        assert_eq!(m.set_count(), 0);
    }

    #[test]
    fn add_uses_shadow_source_rows_after_clip() {
        // A shadow with only its last column set, clipped on the left:
        // the visible part must come from the right source columns.
        let mut s = HardShadow::new(16, 16);
        s.set_bounds(0, 0, 16, 16);
        for y in 0..16 {
            s.data[(y * 16 + 15) as usize] = 1;
        }
        let mut m = FloorHardShadows::new(2, 2, 512, 9);
        m.add(&s, -10, 0);
        let t = set_texels(&m);
        assert_eq!(t.len(), 16);
        assert!(t.iter().all(|&(x, _)| x == 6));
    }

    #[test]
    fn test_finds_holes() {
        let mut m = FloorHardShadows::new(2, 2, 512, 9);
        assert!(m.test(&full_shadow(), 0, 0));
        m.add(&full_shadow(), 0, 0);
        assert!(!m.test(&full_shadow(), 0, 0));
        assert!(m.test(&full_shadow(), 1, 0));
    }

    #[test]
    fn sun_texel_offsets() {
        let sun = SunLighting::environment_default(3, 0.0);
        assert_eq!((sun.shadow_offset_x, sun.shadow_offset_z), (213, 213));
        // dy = -480 (upper level 480 units higher): 213 * -480 >> 8 = -400,
        // x - (-400) = x + 400, >> 5.
        assert_eq!(
            shadow_texel(&sun, 3 << 9, -480, 5 << 9),
            ((1536 + 400) >> 5, (2560 + 400) >> 5)
        );
        assert_eq!(shadow_texel(&sun, 0, 0, 0), (0, 0));
    }
}

// ---------------------------------------------------------------------------
// Entity shadows
// ---------------------------------------------------------------------------

/// Stamps an entity's shadow (`GpuModel::hard_shadow`) at fine `(x, z)` into
/// every normal floor `0..=level`, displaced by the height difference
/// between `level` and that floor at `(x, z)`. Called right after the entity
/// is placed. `dirty` (one set per level) collects the GPU shadow blocks to
/// re-upload when the floors are already live.
pub fn apply_shadow<F: ShadowFloor>(
    floors: &mut [Option<F>],
    shadow: &HardShadow,
    level: usize,
    x: i32,
    z: i32,
    sun: &SunLighting,
    dirty: Option<&mut [HashSet<(usize, usize)>]>,
) {
    project_shadow(floors, shadow, level, [x, z], sun, dirty, stamp);
}

/// The inverse of [`apply_shadow`] — subtracts the shadow from every normal
/// floor `0..=level` at the same displacement. A runtime loc change reaches
/// it after re-deriving the entity's shadow from its model. The underwater
/// early return (the floors being the underwater set) is the caller's: these
/// floors are always the normal set.
pub fn stamp_shadow<F: ShadowFloor>(
    floors: &mut [Option<F>],
    shadow: &HardShadow,
    level: usize,
    x: i32,
    z: i32,
    sun: &SunLighting,
    dirty: Option<&mut [HashSet<(usize, usize)>]>,
) {
    project_shadow(floors, shadow, level, [x, z], sun, dirty, unstamp);
}

/// Per-floor projection step of [`project_shadow`].
type ShadowOp<F> = fn(&mut F, &HardShadow, i32, i32, i32, &SunLighting) -> Option<(i32, i32)>;

/// Shared loop of [`apply_shadow`] / [`stamp_shadow`] (they differ only in the
/// per-floor `op`). `at` is the fine `(x, z)` of the shadow.
fn project_shadow<F: ShadowFloor>(
    floors: &mut [Option<F>],
    shadow: &HardShadow,
    level: usize,
    at: [i32; 2],
    sun: &SunLighting,
    mut dirty: Option<&mut [HashSet<(usize, usize)>]>,
    op: ShadowOp<F>,
) {
    let [x, z] = at;
    let Some(top) = floors.get(level).and_then(|f| f.as_ref()) else {
        return;
    };
    let top_height = top.fine_height(x, z);
    for (level_index, floor) in floors[..=level].iter_mut().enumerate() {
        if let Some(floor) = floor.as_mut() {
            let dy = top_height - floor.fine_height(x, z);
            if let Some((tx, tz)) = op(floor, shadow, x, dy, z, sun) {
                if let Some(set) = dirty.as_deref_mut().and_then(|d| d.get_mut(level_index)) {
                    mark_dirty_blocks(shadow, tx, tz, set);
                }
            }
        }
    }
}

#[cfg(test)]
mod entity_shadow_tests {
    use super::*;
    use crate::floor::FloorHeights;

    fn floor(level_height: i32) -> FloorGeometry {
        let tiles = 16;
        let heights = vec![level_height; (tiles + 1) * (tiles + 1)];
        FloorGeometry {
            tiles_x: tiles,
            tiles_z: tiles,
            has_depth: false,
            has_normals: false,
            water_detail: false,
            underwater: false,
            stride_floats: 5,
            vertex_count: 0,
            index_count: 0,
            stream0: Vec::new(),
            base_colours: Vec::new(),
            tile_tris: Vec::new(),
            batches: Vec::new(),
            hard_shadow: Vec::new(),
            hard_shadows: Some(FloorHardShadows::new(tiles, tiles, 512, 9)),
            min_y: 0.0,
            max_y: 0.0,
            heights: FloorHeights::new(tiles, tiles, 512, heights),
            calls: None,
        }
    }

    fn wall_shadow() -> HardShadow {
        // A 20 x 6 texel wall footprint with a diagonal edge.
        let mut s = HardShadow::new(20, 6);
        s.set_bounds(-10, -3, 10, 3);
        s.raster_triangle(0, 0, 5, 0, 19, 0);
        s.raster_triangle(0, 5, 5, 0, 19, 19);
        s
    }

    /// `Scene.applyShadow` then `Scene.stampShadow` of the same shadow at the
    /// same spot leaves every level's mask byte-identical (the floor sweep
    /// and other entities' stamps untouched), including the wrap-around of
    /// overlapping `byte` adds.
    #[test]
    fn stamp_then_unstamp_restores_masks() {
        let sun = SunLighting::environment_default(3, 0.0);
        let mut floors = vec![Some(floor(0)), Some(floor(-480)), None, None];
        // Pre-existing coverage: a neighbour's shadow and saturated bytes.
        let other = wall_shadow();
        apply_shadow(&mut floors, &other, 1, 7 * 512, 7 * 512, &sun, None);
        for b in floors[0]
            .as_mut()
            .unwrap()
            .hard_shadows
            .as_mut()
            .unwrap()
            .mask
            .iter_mut()
            .step_by(7)
        {
            *b = b.wrapping_add(250);
        }
        let before: Vec<Vec<u8>> = floors
            .iter()
            .flatten()
            .map(|f| f.hard_shadows.as_ref().unwrap().mask.clone())
            .collect();
        let shadow = wall_shadow();
        let mut dirty: Vec<HashSet<(usize, usize)>> = (0..4).map(|_| HashSet::new()).collect();
        apply_shadow(
            &mut floors,
            &shadow,
            1,
            7 * 512 + 256,
            7 * 512,
            &sun,
            Some(&mut dirty),
        );
        let stamped: Vec<Vec<u8>> = floors
            .iter()
            .flatten()
            .map(|f| f.hard_shadows.as_ref().unwrap().mask.clone())
            .collect();
        assert_ne!(stamped, before, "the shadow must reach the masks");
        assert!(!dirty[0].is_empty() && !dirty[1].is_empty());
        stamp_shadow(
            &mut floors,
            &shadow,
            1,
            7 * 512 + 256,
            7 * 512,
            &sun,
            Some(&mut dirty),
        );
        let after: Vec<Vec<u8>> = floors
            .iter()
            .flatten()
            .map(|f| f.hard_shadows.as_ref().unwrap().mask.clone())
            .collect();
        assert_eq!(after, before);
    }

    /// Level 0 receives the level-1 shadow displaced by the height
    /// difference (the top height minus the floor's), level 1 undisplaced.
    #[test]
    fn stamp_displaces_lower_levels_along_the_sun() {
        let sun = SunLighting::environment_default(3, 0.0);
        let mut floors = vec![Some(floor(0)), Some(floor(-480))];
        let shadow = wall_shadow();
        apply_shadow(&mut floors, &shadow, 1, 4 * 512, 4 * 512, &sun, None);
        let mut expect0 = FloorHardShadows::new(16, 16, 512, 9);
        let (tx, tz) = shadow_texel(&sun, 4 * 512, -480, 4 * 512);
        expect0.add(&shadow, tx, tz);
        let mut expect1 = FloorHardShadows::new(16, 16, 512, 9);
        let (tx, tz) = shadow_texel(&sun, 4 * 512, 0, 4 * 512);
        expect1.add(&shadow, tx, tz);
        assert_eq!(
            floors[0]
                .as_ref()
                .unwrap()
                .hard_shadows
                .as_ref()
                .unwrap()
                .mask,
            expect0.mask
        );
        assert_eq!(
            floors[1]
                .as_ref()
                .unwrap()
                .hard_shadows
                .as_ref()
                .unwrap()
                .mask,
            expect1.mask
        );
    }
}
