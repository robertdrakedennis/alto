//! Archive-8 sprite groups decoded into a [`SpriteSheet`]
//! (paletted or full-colour). Split out of client910's `iface`
//! with its byte reader and tests, so the font atlas, the toolkit's
//! `Font` and the UI sprite loaders read it below the UI layer.

use rs910_core::reader::{Eof, Reader as CoreReader};

/// Minimal big-endian reader for the sprite decoder: `g1/g2/g3`.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// One `rs910_core::reader` read of `count` bytes at `pos`, all or
    /// nothing (the arithmetic lives there).
    fn read<T>(
        &mut self,
        count: usize,
        what: &'static str,
        read: impl FnOnce(&mut CoreReader<'a>) -> Result<T, Eof>,
    ) -> anyhow::Result<T> {
        let mut r = CoreReader::at(self.data, self.pos);
        match r.atomic(read) {
            Ok(value) => {
                self.pos = r.pos();
                Ok(value)
            }
            Err(_) if self.pos.checked_add(count).is_none() => Err(anyhow::anyhow!(
                "iface decode: {what} overflows cursor at {}",
                self.pos
            )),
            Err(_) => Err(anyhow::anyhow!(
                "iface decode: truncated {what} at {} (need {count}, have {})",
                self.pos,
                self.remaining()
            )),
        }
    }

    fn take(&mut self, count: usize, what: &'static str) -> anyhow::Result<&'a [u8]> {
        self.read(count, what, |r| r.take(count))
    }

    fn g1(&mut self) -> anyhow::Result<u8> {
        self.read(1, "u8", CoreReader::g1)
    }

    fn g2(&mut self) -> anyhow::Result<u16> {
        self.read(2, "u16", CoreReader::g2)
    }

    fn g3(&mut self) -> anyhow::Result<u32> {
        self.read(3, "u24", CoreReader::g3)
    }
}

// ---------------------------------------------------------------------------
// Sprites: archive-8 decode
// ---------------------------------------------------------------------------

/// One paletted sprite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PalettedSprite {
    /// Left padding.
    pub pad_left: u16,
    /// Top padding.
    pub pad_top: u16,
    /// Glyph width.
    pub width: u16,
    /// Glyph height.
    pub height: u16,
    /// Wire flags (bit 0 = column-major, bit 1 = alpha plane present).
    pub flags: u8,
    /// Palette indices, row-major (`colour`).
    pub colour: Vec<u8>,
    /// Alpha plane, if any.
    pub alpha: Option<Vec<u8>>,
}

/// One full-colour sprite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullSprite {
    /// RGB bytes, row-major.
    pub rgb: Vec<u8>,
    /// Alpha bytes, if any.
    pub alpha: Option<Vec<u8>>,
}

/// One decoded sprite group: paletted or full-colour. Canvas + palette are shared across the sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpriteSheet {
    /// Paletted sheet (index 0 is transparent; `0` palette entries read as 1).
    Paletted {
        /// Canvas width.
        canvas_w: u16,
        /// Canvas height.
        canvas_h: u16,
        /// Palette (index 0 implicit).
        palette: Vec<u32>,
        /// Sprites in file order.
        sprites: Vec<PalettedSprite>,
    },
    /// Full-colour sheet.
    Full {
        /// Width.
        width: u16,
        /// Height.
        height: u16,
        /// Alpha present.
        has_alpha: bool,
        /// Sprites in file order.
        sprites: Vec<FullSprite>,
    },
}

impl SpriteSheet {
    /// Decode one sprite group.
    pub fn decode(data: &[u8]) -> anyhow::Result<Self> {
        if data.len() < 2 {
            anyhow::bail!("sprite: truncated trailer");
        }
        let tail = u16::from_be_bytes([data[data.len() - 2], data[data.len() - 1]]);
        let count = usize::from(tail & 0x7FFF);
        if count == 0 {
            anyhow::bail!("sprite: sheet declares zero sprites");
        }
        if tail >> 15 == 0 {
            Self::decode_paletted(data, count)
        } else {
            Self::decode_full(data, count)
        }
    }

    fn decode_paletted(data: &[u8], count: usize) -> anyhow::Result<Self> {
        // Dims block `7 + count*8` bytes before the end.
        let dims_len = count
            .checked_mul(8)
            .and_then(|planes| planes.checked_add(7))
            .ok_or_else(|| anyhow::anyhow!("sprite: count overflow"))?;
        let dim_pos = data
            .len()
            .checked_sub(dims_len)
            .ok_or_else(|| anyhow::anyhow!("sprite: paletted dims underflow"))?;
        let mut dims = Cursor::new(&data[dim_pos..]);
        let canvas_w = dims.g2()?;
        let canvas_h = dims.g2()?;
        let palette_count = usize::from(dims.g1()?) + 1;
        let mut pad_left = Vec::with_capacity(count);
        let mut pad_top = Vec::with_capacity(count);
        let mut widths = Vec::with_capacity(count);
        let mut heights = Vec::with_capacity(count);
        for _ in 0..count {
            pad_left.push(dims.g2()?);
        }
        for _ in 0..count {
            pad_top.push(dims.g2()?);
        }
        for _ in 0..count {
            widths.push(dims.g2()?);
        }
        for _ in 0..count {
            heights.push(dims.g2()?);
        }
        // Palette `(count-1)*3` bytes immediately before the dims block.
        let palette_len = (palette_count - 1)
            .checked_mul(3)
            .ok_or_else(|| anyhow::anyhow!("sprite: palette size overflow"))?;
        let palette_pos = dim_pos
            .checked_sub(palette_len)
            .ok_or_else(|| anyhow::anyhow!("sprite: paletted palette underflow"))?;
        let mut raw_palette = Cursor::new(&data[palette_pos..palette_pos + palette_len]);
        let mut palette = vec![0_u32; palette_count];
        for slot in palette.iter_mut().skip(1) {
            let rgb = raw_palette.g3()?;
            *slot = if rgb == 0 { 1 } else { rgb };
        }
        // Pixel planes from offset 0, abutting the palette.
        let mut pixels = Cursor::new(data);
        let mut sprites = Vec::with_capacity(count);
        for index in 0..count {
            let (w, h) = (widths[index], heights[index]);
            let flags = pixels.g1()?;
            let column_major = flags & 0x1 != 0;
            let area = usize::from(w)
                .checked_mul(usize::from(h))
                .ok_or_else(|| anyhow::anyhow!("sprite {index}: dimensions overflow"))?;
            let colour = read_plane(&mut pixels, w, h, column_major)?;
            if colour.len() != area {
                anyhow::bail!("sprite {index}: colour plane truncated");
            }
            for (offset, entry) in colour.iter().enumerate() {
                if usize::from(*entry) >= palette_count {
                    anyhow::bail!(
                        "sprite {index} pixel {offset}: palette index {entry} past {palette_count}"
                    );
                }
            }
            let alpha = if flags & 0x2 != 0 {
                Some(read_plane(&mut pixels, w, h, column_major)?)
            } else {
                None
            };
            sprites.push(PalettedSprite {
                pad_left: pad_left[index],
                pad_top: pad_top[index],
                width: w,
                height: h,
                flags,
                colour,
                alpha,
            });
        }
        if pixels.pos != palette_pos {
            anyhow::bail!(
                "sprite: pixel data ends at {} but palette starts at {palette_pos}",
                pixels.pos
            );
        }
        Ok(Self::Paletted {
            canvas_w,
            canvas_h,
            palette,
            sprites,
        })
    }

    fn decode_full(data: &[u8], count: usize) -> anyhow::Result<Self> {
        // Sub-format 1 is rejected, as in the original client.
        let mut header = Cursor::new(data);
        let sub = header.g1()?;
        if sub != 0 {
            anyhow::bail!("sprite: unsupported full-colour sub-format {sub}");
        }
        let has_alpha = match header.g1()? {
            0 => false,
            1 => true,
            other => {
                anyhow::bail!("sprite: full-colour alpha flag must be 0/1, got {other}");
            }
        };
        let width = header.g2()?;
        let height = header.g2()?;
        let area = usize::from(width)
            .checked_mul(usize::from(height))
            .ok_or_else(|| anyhow::anyhow!("sprite: dimensions overflow"))?;
        let rgb_len = area
            .checked_mul(3)
            .ok_or_else(|| anyhow::anyhow!("sprite: dimensions overflow"))?;
        let mut sprites = Vec::with_capacity(count);
        for _ in 0..count {
            let rgb = header.take(rgb_len, "full rgb")?.to_vec();
            let alpha = if has_alpha {
                Some(header.take(area, "full alpha")?.to_vec())
            } else {
                None
            };
            sprites.push(FullSprite { rgb, alpha });
        }
        if header.pos != data.len() - 2 {
            anyhow::bail!(
                "sprite: pixel data ends at {} but trailer starts at {}",
                header.pos,
                data.len() - 2
            );
        }
        Ok(Self::Full {
            width,
            height,
            has_alpha,
            sprites,
        })
    }

    #[cfg(test)]
    /// First sprite's RGBA bytes row-major (palette resolved, magenta
    /// `0xFF00FF` transparent like the full-colour sprite data).
    #[must_use]
    pub fn first_rgba(&self) -> Option<(u16, u16, Vec<u8>)> {
        match self {
            Self::Paletted {
                palette, sprites, ..
            } => {
                let sprite = sprites.first()?;
                let mut px =
                    Vec::with_capacity(usize::from(sprite.width) * usize::from(sprite.height) * 4);
                for (i, index) in sprite.colour.iter().enumerate() {
                    if *index == 0 {
                        px.extend_from_slice(&[0, 0, 0, 0]);
                        continue;
                    }
                    let rgb = palette.get(usize::from(*index)).copied().unwrap_or(1);
                    let a = sprite
                        .alpha
                        .as_ref()
                        .map(|plane| plane[i] as i8)
                        .unwrap_or(-1);
                    if a == 0 {
                        px.extend_from_slice(&[0, 0, 0, 0]);
                    } else {
                        px.extend_from_slice(&[
                            ((rgb >> 16) & 0xFF) as u8,
                            ((rgb >> 8) & 0xFF) as u8,
                            (rgb & 0xFF) as u8,
                            255,
                        ]);
                    }
                }
                Some((sprite.width, sprite.height, px))
            }
            Self::Full {
                width,
                height,
                sprites,
                ..
            } => {
                let sprite = sprites.first()?;
                let mut px = Vec::with_capacity(usize::from(*width) * usize::from(*height) * 4);
                for (i, triple) in sprite.rgb.chunks_exact(3).enumerate() {
                    let argb = (u32::from(triple[0]) << 16)
                        | (u32::from(triple[1]) << 8)
                        | u32::from(triple[2]);
                    if argb == 0xFF_00FF {
                        px.extend_from_slice(&[0, 0, 0, 0]);
                        continue;
                    }
                    let a = sprite.alpha.as_ref().map(|p| p[i]).unwrap_or(255);
                    px.extend_from_slice(&[triple[0], triple[1], triple[2], a]);
                }
                Some((*width, *height, px))
            }
        }
    }
}

/// Read one `w*h` byte plane, normalising column-major wire order to
/// row-major.
fn read_plane(
    pixels: &mut Cursor<'_>,
    w: u16,
    h: u16,
    column_major: bool,
) -> anyhow::Result<Vec<u8>> {
    let size = usize::from(w)
        .checked_mul(usize::from(h))
        .ok_or_else(|| anyhow::anyhow!("sprite: plane dimensions overflow"))?;
    if !column_major {
        return Ok(pixels.take(size, "sprite plane")?.to_vec());
    }
    let mut plane = vec![0_u8; size];
    let (w_usize, h_usize) = (usize::from(w), usize::from(h));
    for x in 0..w_usize {
        for y in 0..h_usize {
            plane[w_usize * y + x] = pixels.g1()?;
        }
    }
    Ok(plane)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprite decode goldens from hand-built paletted bytes.
    #[test]
    fn sprite_paletted_goldens() {
        // 1 sprite, 2x1, palette [transparent, red], row-major colours [1, 0].
        // Layout: [pixels][palette][dims][trailer].
        let mut bytes = Vec::new();
        bytes.push(0); // flags: row-major, no alpha
        bytes.extend_from_slice(&[1, 0]); // colour plane
        bytes.extend_from_slice(&[0xFF, 0x00, 0x00]); // palette[1] = red
        bytes.extend_from_slice(&[0, 2, 0, 1]); // canvas 2x1
        bytes.push(1); // palette count byte (1 => 2 entries incl. transparent)
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 2, 0, 1]); // padL, padT, w, h
        bytes.extend_from_slice(&[0, 1]); // trailer: fmt 0, count 1
        let sheet = SpriteSheet::decode(&bytes).unwrap();
        match &sheet {
            SpriteSheet::Paletted {
                canvas_w,
                canvas_h,
                palette,
                sprites,
            } => {
                assert_eq!((*canvas_w, *canvas_h), (2, 1));
                assert_eq!(palette.len(), 2);
                assert_eq!(sprites.len(), 1);
                assert_eq!(sprites[0].colour, vec![1, 0]);
            }
            SpriteSheet::Full { .. } => panic!("want paletted"),
        }
        let (w, h, px) = sheet.first_rgba().unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(px.len(), 2 * 4);
        // First pixel red opaque, second transparent (index 0).
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(&px[4..8], &[0, 0, 0, 0]);
    }
}
