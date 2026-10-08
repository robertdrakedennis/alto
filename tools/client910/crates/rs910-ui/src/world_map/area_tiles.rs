//! Area tile decoding and floor colour blending.

use crate::ui_bytes::Cursor;

use anyhow::{Context, Result};

use std::collections::HashMap;

use super::{Element, TileLocs, UpperTile};

/// The loaded area: the tile arrays and element list.
pub struct Area {
    /// The chunk-aligned display origin.
    pub origin: [i32; 2],
    /// Tiles across and up.
    pub size: [i32; 2],
    /// Underlay ids (released after the blend).
    pub(super) underlay: Vec<i16>,
    /// Overlay ids.
    pub(super) overlay: Vec<i16>,
    /// Overlay shape/rotation bytes.
    pub(super) shape: Vec<i8>,
    /// Ground-level locs.
    pub(super) locs: HashMap<usize, TileLocs>,
    /// The blended ground RGB (0 = none).
    pub(super) colour: Vec<i32>,
    /// Upper-level tiles per `[level - 1][chunk x][chunk z]`, keyed
    /// `(x << 8) + z`.
    pub(super) upper: [Vec<Option<HashMap<u16, UpperTile>>>; 3],
    /// The member-accessible 8x8 blocks, `[x / 8][z / 8]`.
    members: Vec<bool>,
    /// The element list: the loc elements, then the static ones.
    pub elements: Vec<Element>,
}

impl Area {
    /// An empty area of the given chunk-aligned bounds.
    pub fn new(origin: [i32; 2], size: [i32; 2]) -> Self {
        let n = (size[0].max(0) as usize) * (size[1].max(0) as usize);
        let chunks = (size[0].max(0) >> 6) as usize * (size[1].max(0) >> 6) as usize;
        Self {
            origin,
            size,
            underlay: vec![0; n],
            overlay: vec![0; n],
            shape: vec![0; n],
            locs: HashMap::new(),
            colour: vec![0; n],
            upper: std::array::from_fn(|_| vec![None; chunks]),
            members: vec![false; (size[0].max(0) as usize / 8) * (size[1].max(0) as usize / 8)],
            elements: Vec::new(),
        }
    }
    pub(super) fn chunks_z(&self) -> i32 {
        self.size[1] >> 6
    }
    pub(super) fn index(&self, x: i32, z: i32) -> Option<usize> {
        (x >= 0 && z >= 0 && x < self.size[0] && z < self.size[1])
            .then(|| (self.size[0] * z + x) as usize)
    }
    pub(super) fn upper_chunk(
        &self,
        level: usize,
        cx: i32,
        cz: i32,
    ) -> Option<&HashMap<u16, UpperTile>> {
        if cx < 0 || cz < 0 || cx >= self.size[0] >> 6 || cz >= self.chunks_z() {
            return None;
        }
        self.upper[level][(cx * self.chunks_z() + cz) as usize].as_ref()
    }
    pub(super) fn upper_chunk_mut(
        &mut self,
        level: usize,
        cx: i32,
        cz: i32,
    ) -> Option<&mut HashMap<u16, UpperTile>> {
        if cx < 0 || cz < 0 || cx >= self.size[0] >> 6 || cz >= self.chunks_z() {
            return None;
        }
        let at = (cx * self.chunks_z() + cz) as usize;
        Some(self.upper[level][at].get_or_insert_with(HashMap::new))
    }
    /// Whether the 8x8 block holding tile `(x, z)` is member-accessible.
    pub fn member_block(&self, x: i32, z: i32) -> bool {
        let (bx, bz) = (x >> 3, z >> 3);
        let (w, h) = (self.size[0] / 8, self.size[1] / 8);
        bx >= 0 && bz >= 0 && bx < w && bz < h && self.members[(bx * h + bz) as usize]
    }
    pub(super) fn set_member_block(&mut self, bx: i32, bz: i32, value: bool) {
        let (w, h) = (self.size[0] / 8, self.size[1] / 8);
        if bx >= 0 && bz >= 0 && bx < w && bz < h {
            self.members[(bx * h + bz) as usize] = value;
        }
    }
    /// The overlay id and shape byte of a display tile (tests).
    #[cfg(test)]
    pub fn tile_overlay(&self, x: i32, z: i32) -> (i16, i8) {
        self.index(x - self.origin[0], z - self.origin[1])
            .map_or((0, 0), |i| (self.overlay[i], self.shape[i]))
    }

    /// One tile record at local `(x, z)` of region `(region_x, region_z)`.
    /// Out-of-area writes are dropped after the record is consumed.
    pub(super) fn decode_tile(
        &mut self,
        c: &mut Cursor<'_>,
        region: [i32; 2],
        x: i32,
        z: i32,
        underlays: &[i32],
        overlays: &[i32],
    ) -> Result<()> {
        let flags = i32::from(c.g1()?);
        let at = self.index(x, z);
        if flags & 1 == 0 {
            let underlay = flags & 2 == 0;
            let palette = flags >> 2 & 0x3F;
            if palette == 62 {
                return Ok(());
            }
            let value = if palette == 63 {
                c.gsmart1or2()?
            } else if underlay {
                *underlays
                    .get(palette as usize)
                    .context("world-map underlay palette")?
            } else {
                *overlays
                    .get(palette as usize)
                    .context("world-map overlay palette")?
            };
            if underlay {
                if let Some(i) = at {
                    self.underlay[i] = value as i16;
                    self.overlay[i] = 0;
                }
            } else {
                let under = c.gsmart1or2()?;
                if let Some(i) = at {
                    self.overlay[i] = value as i16;
                    self.shape[i] = 0;
                    self.underlay[i] = under as i16;
                }
            }
            return Ok(());
        }
        let levels = (flags >> 1 & 0x3) + 1;
        let has_shape = flags & 0x8 != 0;
        let has_locs = flags & 0x10 != 0;
        for level in 0..levels {
            let underlay = c.gsmart1or2()?;
            let (mut overlay, mut shape) = (0, 0);
            if has_shape {
                overlay = c.gsmart1or2()?;
                shape = i32::from(c.g1()?);
            }
            let loc_count = if has_locs { usize::from(c.g1()?) } else { 0 };
            if level == 0 {
                if let Some(i) = at {
                    self.underlay[i] = underlay as i16;
                    self.overlay[i] = overlay as i16;
                    self.shape[i] = shape as i8;
                }
                if loc_count == 1 {
                    let id = c.gsmart2or4s()?;
                    let angle = c.g1b()?;
                    if let Some(i) = at {
                        self.locs.insert(i, TileLocs::One(id, angle));
                    }
                } else if loc_count > 1 {
                    let mut ids = Vec::with_capacity(loc_count);
                    let mut angles = Vec::with_capacity(loc_count);
                    for _ in 0..loc_count {
                        ids.push(c.gsmart2or4s()?);
                        angles.push(c.g1b()?);
                    }
                    if let Some(i) = at {
                        self.locs.insert(i, TileLocs::Many(ids, angles));
                    }
                }
            } else {
                let locs = if loc_count > 0 {
                    let mut ids = Vec::with_capacity(loc_count);
                    let mut angles = Vec::with_capacity(loc_count);
                    for _ in 0..loc_count {
                        ids.push(c.gsmart2or4s()?);
                        angles.push(c.g1b()?);
                    }
                    Some((ids, angles))
                } else {
                    None
                };
                let (cx, cz) = (
                    region[0] - (self.origin[0] >> 6),
                    region[1] - (self.origin[1] >> 6),
                );
                let key = (((x & 0x3F) << 8) + (z & 0x3F)) as u16;
                if let Some(chunk) = self.upper_chunk_mut(level as usize - 1, cx, cz) {
                    chunk.insert(
                        key,
                        UpperTile {
                            x: (x & 0x3F) as u8,
                            z: (z & 0x3F) as u8,
                            colour: underlay,
                            overlay: overlay as i16,
                            shape: shape as i8,
                            locs,
                        },
                    );
                }
            }
        }
        Ok(())
    }
}

/// The 11x11 underlay blend of `ids` into `out` (24-bit RGB), with the load's
/// hue and lightness jitter. Tiles whose window holds no underlay keep
/// `out`'s previous value: the output array is shared between the levels.
pub fn blend(
    ids: &[i16],
    out: &mut [i32],
    size: [i32; 2],
    underlays: &crate::flo::FloStore,
    hue_offset: i32,
    lightness_offset: i32,
    palette: &[i32],
) {
    let (w, h) = (size[0], size[1]);
    let hn = h.max(0) as usize;
    let mut hue = vec![0i32; hn];
    let mut sat = vec![0i32; hn];
    let mut light = vec![0i32; hn];
    let mut chroma = vec![0i32; hn];
    let mut count = vec![0i32; hn];
    let terms = |id: i16| -> Option<[i32; 4]> {
        // A missing underlay type reads as the defaults.
        let t = underlays.get_underlay(u32::try_from(id - 1).ok()?);
        Some(t.map_or([0; 4], |t| {
            [
                t.hue,
                t.saturation as i32,
                t.lightness as i32,
                t.chroma as i32,
            ]
        }))
    };
    for x in -5..w {
        let add = x + 5;
        let sub = x - 5;
        for z in 0..h as usize {
            if add < w {
                let id = ids[w as usize * z + add as usize];
                if id > 0 {
                    if let Some(t) = terms(id) {
                        hue[z] += t[0];
                        sat[z] += t[1];
                        light[z] += t[2];
                        chroma[z] += t[3];
                        count[z] += 1;
                    }
                }
            }
            if sub >= 0 {
                let id = ids[w as usize * z + sub as usize];
                if id > 0 {
                    if let Some(t) = terms(id) {
                        hue[z] -= t[0];
                        sat[z] -= t[1];
                        light[z] -= t[2];
                        chroma[z] -= t[3];
                        count[z] -= 1;
                    }
                }
            }
        }
        if x < 0 {
            continue;
        }
        let (mut h_sum, mut s_sum, mut l_sum, mut c_sum, mut n) = (0, 0, 0, 0, 0);
        for z in -5..h {
            let add = z + 5;
            if add < h {
                let a = add as usize;
                h_sum += hue[a];
                s_sum += sat[a];
                l_sum += light[a];
                c_sum += chroma[a];
                n += count[a];
            }
            let sub = z - 5;
            if sub >= 0 {
                let s = sub as usize;
                h_sum -= hue[s];
                s_sum -= sat[s];
                l_sum -= light[s];
                c_sum -= chroma[s];
                n -= count[s];
            }
            if z >= 0 && n > 0 {
                let at = (w * z + x) as usize;
                if ids[at] == 0 {
                    out[at] = 0;
                } else {
                    let hsl = if c_sum == 0 {
                        0
                    } else {
                        crate::colour::hsl24to16(
                            h_sum.wrapping_mul(256) / c_sum,
                            s_sum / n,
                            l_sum / n,
                        )
                    };
                    let mut l = (hsl & 0x7F) + lightness_offset;
                    l = l.clamp(0, 127);
                    let adjusted = ((hue_offset + hsl) & 0xFC00) + (hsl & 0x380) + l;
                    let rgb = palette[(crate::colour::renormalise_saturation(
                        crate::colour::mul_hsl(adjusted, 96),
                    ) & 0xFFFF) as usize];
                    out[at] = rgb & 0xFF_FFFF;
                }
            }
        }
    }
}

/// An overlay type's world-map colour.
pub fn overlay_colour(
    flo: &crate::flo::FloStore,
    materials: Option<&crate::texture::MaterialStore>,
    id: u32,
    hue_offset: i32,
    lightness_offset: i32,
    palette: &[i32],
) -> i32 {
    let Some(t) = flo.get_overlay(id) else {
        return 0;
    };
    let mut material = t.texture.map_or(-1, |m| m as i32);
    let material_def =
        materials.and_then(|m| u32::try_from(material).ok().and_then(|id| m.get(id)));
    if material >= 0 && material_def.is_some_and(|m| m.high_detail) {
        material = -1;
    }
    let jitter = |hsl: i32| -> i32 {
        let l = ((hsl & 0x7F) + lightness_offset).clamp(0, 127);
        ((hue_offset + hsl) & 0xFC00) + (hsl & 0x380) + l
    };
    let lookup = |hsl: i32| {
        palette[(crate::colour::renormalise_saturation(crate::colour::adjust_lightness(hsl, 96))
            & 0xFFFF) as usize]
            | 0xFF00_0000u32 as i32
    };
    if t.average_colour >= 0 {
        lookup(jitter(t.average_colour))
    } else if material >= 0 {
        // The material's average colour; a missing material reads the
        // default colour 0.
        lookup(i32::from(
            material_def.map_or(0, |m| m.average_colour as i16),
        ))
    } else if t.rgb == -1 {
        0
    } else {
        lookup(jitter(t.rgb))
    }
}
