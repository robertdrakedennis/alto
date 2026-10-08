//! 910 sprite-sheet codec: binary ⇄ [`SpriteSheet`].
//!
//! Pack map: `server/data/pack/client.sprites.js5` — 27,266 single-file
//! groups; the group id is the sprite id carried by interfaces
//! ([`crate::interface::ComponentBody::Graphic::graphic`]), cursors,
//! map elements and fonts, and the file id within a group is always 0.
//! One group holds one sheet: either a paletted (indexed-colour) sheet or
//! a full-colour (24-bit RGB) sheet, selected by the high bit of the
//! trailing 2-byte `fmt/count` word.
//!
//! A faithful port of the 910 client's sprite decoder: the paletted branch
//! reads the end-anchored dims block (shared canvas, palette size, then one
//! planar column each of padding-left, padding-top, width, height), the
//! shared palette just before it (index 0 is the implicit transparent slot),
//! and the per-sprite flags byte + colour/alpha planes from offset 0; the
//! full-colour branch reads the shared header (`subFormat`, `hasAlpha`,
//! width, height) plus one planar RGB (+ optional alpha) image per sprite.
//!
//! Deliberate model choices (all byte-exact, all gate-proven):
//!
//! * Stored bytes only — no rasterization, no `image` crate. Palette entries
//!   stay raw `u24`s, colour planes stay index bytes, full-colour planes stay
//!   raw RGB/alpha bytes. In particular the client's render mappings are NOT
//!   applied: a zero palette entry is kept as zero (the client forces it to
//!   1 when resolving pixels), and an opaque-magenta full-colour pixel is
//!   kept as magenta (the client keys it to transparent).
//! * The paletted flags byte is stored raw. Bit 0 selects column-major plane
//!   order and bit 1 carries the alpha plane; the client masks those two
//!   bits and ignores the rest, and the corpus does set high bits on a few
//!   sprites — so the byte round-trips untouched instead of being validated.
//! * An all-opaque alpha plane is kept (`Some`), even though the client drops
//!   it to null: the flags bit proves the plane was on the wire, and
//!   re-encoding must emit it to stay byte-identical.
//! * Pixel planes must abut the palette exactly (paletted) and pixel data
//!   must consume the payload up to the trailer (full-colour); anything else
//!   is invalid data rather than a shifted-but-decodable read.

use crate::error::{NativeError, Result};
use crate::packet::{ByteWriter, Packet};

/// One decoded 910 sprite sheet: the payload of a single group (file 0) of
/// `client.sprites.js5`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpriteSheet {
    /// Indexed-colour sheet: shared palette plus per-sprite index planes.
    Paletted(PalettedSheet),
    /// Full-colour sheet: shared dimensions plus per-sprite RGB planes.
    Full(FullSheet),
}

impl SpriteSheet {
    /// Number of sprites in the sheet (the trailer count; always ≥ 1).
    #[must_use]
    pub fn sprite_count(&self) -> usize {
        match self {
            Self::Paletted(sheet) => sheet.sprites.len(),
            Self::Full(sheet) => sheet.sprites.len(),
        }
    }

    /// Whether this is a paletted (indexed-colour) sheet.
    #[must_use]
    pub fn is_paletted(&self) -> bool {
        matches!(self, Self::Paletted(_))
    }
}

/// A paletted (fmt-0) sprite sheet: one shared canvas and palette, plus the
/// per-sprite sub-images placed within the canvas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PalettedSheet {
    /// Shared canvas width every sprite is placed within.
    pub canvas_width: u16,
    /// Shared canvas height every sprite is placed within.
    pub canvas_height: u16,
    /// Shared palette as stored: index 0 is the implicit transparent slot
    /// (always zero, carrying no bytes); entries `1..` are raw `u24` RGB
    /// values exactly as carried, including zeros (no 0→1 forcing).
    pub palette: Vec<u32>,
    /// The sprites, in sheet order. Never empty.
    pub sprites: Vec<PalettedSprite>,
}

/// One indexed-colour sprite: padding plus a `width × height` index plane
/// (and optional alpha plane) in row-major order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PalettedSprite {
    /// Canvas offset of the sub-image's left edge.
    pub padding_left: u16,
    /// Canvas offset of the sub-image's top edge.
    pub padding_top: u16,
    /// Sub-image width in pixels.
    pub width: u16,
    /// Sub-image height in pixels.
    pub height: u16,
    /// Raw flags byte: bit 0 = column-major plane order, bit 1 = an alpha
    /// plane follows the colour plane. Higher bits are preserved untouched
    /// (the client masks them away); they occur rarely in the corpus.
    pub flags: u8,
    /// Palette indices, `width × height` bytes in row-major order.
    pub colour: Vec<u8>,
    /// Alpha bytes, `width × height` in row-major order. `Some` exactly when
    /// the flags alpha bit is set — even when every byte is opaque.
    pub alpha: Option<Vec<u8>>,
}

impl PalettedSprite {
    /// Whether the planes are stored column-major on the wire.
    #[must_use]
    pub fn column_major(&self) -> bool {
        self.flags & 0x1 != 0
    }

    /// Whether an alpha plane is carried (the flags bit), regardless of its
    /// contents.
    #[must_use]
    pub fn has_alpha(&self) -> bool {
        self.flags & 0x2 != 0
    }

    /// Derived right padding: `canvas - width - padding_left` (an `i32`
    /// because the subtraction is client-exact; the corpus keeps it ≥ 0).
    #[must_use]
    pub fn padding_right(&self, canvas_width: u16) -> i32 {
        i32::from(canvas_width) - i32::from(self.width) - i32::from(self.padding_left)
    }

    /// Derived bottom padding: `canvas - height - padding_top`.
    #[must_use]
    pub fn padding_bottom(&self, canvas_height: u16) -> i32 {
        i32::from(canvas_height) - i32::from(self.height) - i32::from(self.padding_top)
    }
}

/// A full-colour (fmt-1) sprite sheet: one shared size, plus one planar RGB
/// (+ optional alpha) image per sprite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullSheet {
    /// Image width shared by every sprite in the sheet.
    pub width: u16,
    /// Image height shared by every sprite in the sheet.
    pub height: u16,
    /// Whether every sprite carries an alpha plane after its RGB plane.
    pub has_alpha: bool,
    /// The sprites, in sheet order. Never empty.
    pub sprites: Vec<FullSprite>,
}

/// One full-colour sprite: raw RGB triplets plus optional raw alpha bytes,
/// all row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullSprite {
    /// Raw RGB bytes, `width × height × 3` (magenta kept as magenta: the
    /// client's transparent keying is a render mapping, not storage).
    pub rgb: Vec<u8>,
    /// Raw alpha bytes, `width × height`, present exactly when the sheet's
    /// `has_alpha` is set.
    pub alpha: Option<Vec<u8>>,
}

fn invalid(what: &str) -> NativeError {
    NativeError::Invalid(format!("bad sprite ({what})"))
}

/// Decode one sprite-sheet payload (a single group file of
/// `client.sprites.js5`) into a [`SpriteSheet`].
pub fn decode_sprite(data: &[u8]) -> Result<SpriteSheet> {
    if data.len() < 2 {
        return Err(NativeError::Truncated {
            what: "sprite trailer",
        });
    }
    let mut tail = Packet::with_pos(data, data.len() - 2)?;
    let last = tail.g2()?;
    let count = usize::from(last & 0x7FFF);
    if count == 0 {
        return Err(invalid("sheet declares zero sprites"));
    }
    if last >> 15 == 0 {
        decode_paletted(data, count)
    } else {
        decode_full(data, count)
    }
}

/// Decode the fmt-0 paletted branch of the sprite decoder.
fn decode_paletted(data: &[u8], count: usize) -> Result<SpriteSheet> {
    // The dims block (canvas, palette size, four planar columns) sits
    // `7 + count * 8` bytes before the end; the 7 already includes the
    // 2-byte trailer.
    let dims_len = count
        .checked_mul(8)
        .and_then(|planes| planes.checked_add(7))
        .ok_or_else(|| invalid("sprite count overflow"))?;
    let dim_pos = data
        .len()
        .checked_sub(dims_len)
        .ok_or_else(|| invalid("paletted sprite dims underflow"))?;
    let mut dims = Packet::with_pos(data, dim_pos)?;
    let canvas_width = dims.g2()?;
    let canvas_height = dims.g2()?;
    let palette_count = usize::from(dims.g1()?) + 1;
    let mut padding_left = Vec::with_capacity(count);
    let mut padding_top = Vec::with_capacity(count);
    let mut widths = Vec::with_capacity(count);
    let mut heights = Vec::with_capacity(count);
    for _ in 0..count {
        padding_left.push(dims.g2()?);
    }
    for _ in 0..count {
        padding_top.push(dims.g2()?);
    }
    for _ in 0..count {
        widths.push(dims.g2()?);
    }
    for _ in 0..count {
        heights.push(dims.g2()?);
    }

    // The palette's `(count - 1)` 3-byte entries sit immediately before the
    // dims block; index 0 is the implicit transparent slot with no bytes.
    let palette_len = (palette_count - 1)
        .checked_mul(3)
        .ok_or_else(|| invalid("palette size overflow"))?;
    let palette_pos = dim_pos
        .checked_sub(palette_len)
        .ok_or_else(|| invalid("paletted sprite palette underflow"))?;
    let mut raw_palette = Packet::with_pos(data, palette_pos)?;
    let mut palette = vec![0_u32; palette_count];
    for slot in palette.iter_mut().skip(1) {
        *slot = raw_palette.g3()?;
    }

    // Pixel planes run from offset 0 and must abut the palette exactly.
    let mut pixels = Packet::new(data);
    let mut sprites = Vec::with_capacity(count);
    for index in 0..count {
        let width = widths[index];
        let height = heights[index];
        let flags = pixels.g1()?;
        let column_major = flags & 0x1 != 0;
        let colour = read_plane(
            &mut pixels,
            width,
            height,
            column_major,
            "paletted colour plane",
        )?;
        for (offset, &entry) in colour.iter().enumerate() {
            if usize::from(entry) >= palette_count {
                return Err(invalid(&format!(
                    "sprite {index} pixel {offset}: palette index {entry} out of range {palette_count}"
                )));
            }
        }
        let alpha = if flags & 0x2 != 0 {
            Some(read_plane(
                &mut pixels,
                width,
                height,
                column_major,
                "paletted alpha plane",
            )?)
        } else {
            None
        };
        sprites.push(PalettedSprite {
            padding_left: padding_left[index],
            padding_top: padding_top[index],
            width,
            height,
            flags,
            colour,
            alpha,
        });
    }
    if pixels.pos() != palette_pos {
        return Err(invalid(&format!(
            "pixel data ends at {} but the palette starts at {palette_pos}",
            pixels.pos()
        )));
    }

    Ok(SpriteSheet::Paletted(PalettedSheet {
        canvas_width,
        canvas_height,
        palette,
        sprites,
    }))
}

/// Read one `width × height` byte plane, normalising column-major wire order
/// to row-major exactly like the client's nested read loops.
fn read_plane(
    pixels: &mut Packet<'_>,
    width: u16,
    height: u16,
    column_major: bool,
    what: &'static str,
) -> Result<Vec<u8>> {
    let size = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or_else(|| invalid("sprite dimensions overflow"))?;
    if pixels.remaining() < size {
        return Err(NativeError::Truncated { what });
    }
    if !column_major {
        return pixels.gdata(size, what);
    }
    let width = usize::from(width);
    let height = usize::from(height);
    let mut plane = vec![0_u8; size];
    for x in 0..width {
        for y in 0..height {
            plane[width * y + x] = pixels.g1()?;
        }
    }
    Ok(plane)
}

/// Decode the fmt-1 full-colour branch of the sprite decoder.
/// `subFormat` 1 is rejected: the client itself refuses it as unsupported, so
/// it is never valid 910 input.
fn decode_full(data: &[u8], count: usize) -> Result<SpriteSheet> {
    let mut header = Packet::new(data);
    let sub_format = header.g1()?;
    if sub_format != 0 {
        return Err(invalid(&format!(
            "unsupported full-colour sprite sub-format {sub_format}"
        )));
    }
    let has_alpha = match header.g1()? {
        0 => false,
        1 => true,
        other => {
            return Err(invalid(&format!(
                "full-colour sprite alpha flag must be 0 or 1, got {other}"
            )));
        }
    };
    let width = header.g2()?;
    let height = header.g2()?;
    let area = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or_else(|| invalid("sprite dimensions overflow"))?;
    let rgb_len = area
        .checked_mul(3)
        .ok_or_else(|| invalid("sprite dimensions overflow"))?;

    let mut sprites = Vec::with_capacity(count);
    for _ in 0..count {
        let rgb = header.gdata(rgb_len, "full-colour rgb plane")?;
        let alpha = if has_alpha {
            Some(header.gdata(area, "full-colour alpha plane")?)
        } else {
            None
        };
        sprites.push(FullSprite { rgb, alpha });
    }
    if header.pos() != data.len() - 2 {
        return Err(invalid(&format!(
            "pixel data ends at {} but the trailer starts at {}",
            header.pos(),
            data.len() - 2
        )));
    }

    Ok(SpriteSheet::Full(FullSheet {
        width,
        height,
        has_alpha,
        sprites,
    }))
}

/// Encode a [`SpriteSheet`] back to its 910 binary form. The output is
/// byte-identical to the input that [`decode_sprite`] read whenever the
/// model came from the decoder; hand-built models are validated strictly
/// (plane sizes, alpha presence, palette shape) and rejected loudly.
pub fn encode_sprite(sheet: &SpriteSheet) -> Result<Vec<u8>> {
    match sheet {
        SpriteSheet::Paletted(sheet) => encode_paletted(sheet),
        SpriteSheet::Full(sheet) => encode_full(sheet),
    }
}

fn encode_paletted(sheet: &PalettedSheet) -> Result<Vec<u8>> {
    let count = sheet.sprites.len();
    if count == 0 || count > 0x7FFF {
        return Err(invalid(&format!(
            "paletted sheet sprite count {count} out of range 1..=32767"
        )));
    }
    let palette_count = sheet.palette.len();
    if palette_count == 0 || palette_count > 256 {
        return Err(invalid(&format!(
            "paletted sheet palette size {palette_count} out of range 1..=256"
        )));
    }
    if sheet.palette[0] != 0 {
        return Err(invalid("palette index 0 is the implicit transparent slot"));
    }

    let mut writer = ByteWriter::default();
    for (index, sprite) in sheet.sprites.iter().enumerate() {
        let area = usize::from(sprite.width)
            .checked_mul(usize::from(sprite.height))
            .ok_or_else(|| invalid("sprite dimensions overflow"))?;
        if sprite.colour.len() != area {
            return Err(invalid(&format!(
                "sprite {index} colour plane holds {} bytes for a {}x{} image",
                sprite.colour.len(),
                sprite.width,
                sprite.height
            )));
        }
        for (offset, &entry) in sprite.colour.iter().enumerate() {
            if usize::from(entry) >= palette_count {
                return Err(invalid(&format!(
                    "sprite {index} pixel {offset}: palette index {entry} out of range {palette_count}"
                )));
            }
        }
        match &sprite.alpha {
            Some(alpha) if sprite.has_alpha() => {
                if alpha.len() != area {
                    return Err(invalid(&format!(
                        "sprite {index} alpha plane holds {} bytes for a {}x{} image",
                        alpha.len(),
                        sprite.width,
                        sprite.height
                    )));
                }
            }
            None if !sprite.has_alpha() => {}
            _ => {
                return Err(invalid(&format!(
                    "sprite {index} alpha plane disagrees with its flags byte {:#04X}",
                    sprite.flags
                )));
            }
        }
        writer.p1(sprite.flags);
        write_plane(
            &mut writer,
            &sprite.colour,
            sprite.width,
            sprite.height,
            sprite.column_major(),
        );
        if let Some(alpha) = &sprite.alpha {
            write_plane(
                &mut writer,
                alpha,
                sprite.width,
                sprite.height,
                sprite.column_major(),
            );
        }
    }

    for entry in sheet.palette.iter().skip(1) {
        writer.p3(*entry)?;
    }
    writer.p2(sheet.canvas_width);
    writer.p2(sheet.canvas_height);
    writer.p1((palette_count - 1) as u8);
    for sprite in &sheet.sprites {
        writer.p2(sprite.padding_left);
    }
    for sprite in &sheet.sprites {
        writer.p2(sprite.padding_top);
    }
    for sprite in &sheet.sprites {
        writer.p2(sprite.width);
    }
    for sprite in &sheet.sprites {
        writer.p2(sprite.height);
    }
    writer.p2(count as u16);
    Ok(writer.data)
}

/// Encode one fmt-1 sheet: the shared header, then the planar RGB (+ alpha)
/// images, then the trailer with the fmt bit set.
fn encode_full(sheet: &FullSheet) -> Result<Vec<u8>> {
    let count = sheet.sprites.len();
    if count == 0 || count > 0x7FFF {
        return Err(invalid(&format!(
            "full-colour sheet sprite count {count} out of range 1..=32767"
        )));
    }
    let area = usize::from(sheet.width)
        .checked_mul(usize::from(sheet.height))
        .ok_or_else(|| invalid("sprite dimensions overflow"))?;
    let rgb_len = area
        .checked_mul(3)
        .ok_or_else(|| invalid("sprite dimensions overflow"))?;

    let mut writer = ByteWriter::default();
    writer.p1(0);
    writer.p1(u8::from(sheet.has_alpha));
    writer.p2(sheet.width);
    writer.p2(sheet.height);
    for (index, sprite) in sheet.sprites.iter().enumerate() {
        if sprite.rgb.len() != rgb_len {
            return Err(invalid(&format!(
                "sprite {index} rgb plane holds {} bytes for a {}x{} image",
                sprite.rgb.len(),
                sheet.width,
                sheet.height
            )));
        }
        match (&sprite.alpha, sheet.has_alpha) {
            (Some(alpha), true) if alpha.len() == area => {}
            (None, false) => {}
            _ => {
                return Err(invalid(&format!(
                    "sprite {index} alpha plane disagrees with has_alpha {}",
                    sheet.has_alpha
                )));
            }
        }
        writer.pdata(&sprite.rgb);
        if let Some(alpha) = &sprite.alpha {
            writer.pdata(alpha);
        }
    }
    writer.p2(count as u16 | 0x8000);
    Ok(writer.data)
}

/// Write one byte plane, shuffling row-major storage back to column-major
/// wire order when the flags byte selects it.
fn write_plane(writer: &mut ByteWriter, plane: &[u8], width: u16, height: u16, column_major: bool) {
    if !column_major {
        writer.pdata(plane);
        return;
    }
    let width = usize::from(width);
    let height = usize::from(height);
    for x in 0..width {
        for y in 0..height {
            writer.p1(plane[width * y + x]);
        }
    }
}
