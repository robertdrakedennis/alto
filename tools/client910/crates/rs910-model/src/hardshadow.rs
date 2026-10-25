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

    /// Fixed-point (16.16) scanline rasteriser. Argument order is the three
    /// **y**s then the three **x**s. Rows are filled with [`fill_span`] at
    /// `width * y`; rows above 0 are clipped.
    pub fn raster_triangle(&mut self, y0: i32, y1: i32, y2: i32, x0: i32, x1: i32, x2: i32) {
        // Edge slopes, 16.16 dx/dy.
        let slope = |xa: i32, ya: i32, xb: i32, yb: i32| {
            if ya == yb {
                0
            } else {
                ((xb - xa) << 16) / (yb - ya)
            }
        };
        let (s01, s12, s20) = (
            slope(x0, y0, x1, y1),
            slope(x1, y1, x2, y2),
            slope(x2, y2, x0, y0),
        );
        // Each vertex order keeps its own rule for which side the long edge
        // takes when slopes tie (`flat`: the clipped top and middle rows
        // coincide).
        if y0 <= y1 && y0 <= y2 {
            if y1 < y2 {
                self.raster_sorted([y0, y1, y2], [x0, x1], [s01, s12, s20], |flat| {
                    (flat || s20 >= s01) && (!flat || s20 <= s12)
                });
            } else {
                self.raster_sorted([y0, y2, y1], [x0, x2], [s20, s12, s01], |flat| {
                    (!flat && s20 < s01) || (flat && s12 > s01)
                });
            }
        } else if y1 <= y2 {
            if y2 < y0 {
                self.raster_sorted([y1, y2, y0], [x1, x2], [s12, s20, s01], |flat| {
                    !((!flat && s01 < s12) || (flat && s01 > s20))
                });
            } else {
                self.raster_sorted([y1, y0, y2], [x1, x0], [s01, s20, s12], |_| s01 < s12);
            }
        } else if y0 < y1 {
            self.raster_sorted([y2, y0, y1], [x2, x0], [s20, s01, s12], |_| s12 >= s20);
        } else {
            self.raster_sorted([y2, y1, y0], [x2, x1], [s12, s01, s20], |_| s12 < s20);
        }
    }

    /// Fill the triangle with vertices sorted top, middle, bottom (`ys`;
    /// `xs` of the top and middle) from its top-middle, middle-bottom and
    /// top-bottom slopes. `long_right(flat)` says whether the long
    /// top-bottom edge bounds the spans on the right.
    fn raster_sorted(
        &mut self,
        [mut top, mut middle, bottom]: [i32; 3],
        [top_x, middle_x]: [i32; 2],
        [s_top_middle, s_middle_bottom, s_long]: [i32; 3],
        long_right: impl FnOnce(bool) -> bool,
    ) {
        let mut long = top_x << 16;
        let mut short = top_x << 16;
        if top < 0 {
            long -= top * s_long;
            short -= top * s_top_middle;
            top = 0;
        }
        let mut lower = middle_x << 16;
        if middle < 0 {
            lower -= middle * s_middle_bottom;
            middle = 0;
        }
        let right = long_right(top == middle);
        let w = self.width;
        let data = &mut self.data;
        let mut span = |row: i32, edge: i32, long: i32| {
            if right {
                fill_span(data, row, edge >> 16, long >> 16);
            } else {
                fill_span(data, row, long >> 16, edge >> 16);
            }
        };
        let mut row = w * top;
        for _ in top..middle {
            span(row, short, long);
            long += s_long;
            short += s_top_middle;
            row += w;
        }
        for _ in middle..bottom {
            span(row, lower, long);
            long += s_long;
            lower += s_middle_bottom;
            row += w;
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
