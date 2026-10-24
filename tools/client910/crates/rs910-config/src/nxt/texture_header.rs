//! Headers of the four texture archives: format, dimensions, faces and mip
//! count per texture id, with the container framing checked end to end.
//! No pixels are decoded (the PNG decoder and the raw DXT/ETC passthrough
//! are [`crate::texture`]).
//!
//! Every file starts with a version byte: `1` one image, `6` a cubemap.
//! Then, per face:
//!
//! | archive | payload per face |
//! |---|---|
//! | 52 `textures.dxt` | `u32be len` + DDS (`DXT1`/`DXT5`, full mip chain). Cubemaps are **one** DDS with 6 faces (`caps2 = 0xFE00`) in a version-1 frame. |
//! | 53 `textures.png` | `u32be len` + PNG (no mips). |
//! | 54 `textures.png.mipped` | `u8 mips` + `mips x (u32be len + PNG)`, largest first. |
//! | 55 `textures.etc` | `u32be len` + KTX 1.1 (`0x9278` ETC2 RGBA8/EAC, full mip chain); cubemaps are 6 frames. |
//!
//! 865 reads DXT and PNG (`TextureManager::GetTextureDataDXT` L1060792,
//! `GetTextureDataPNG`) and has no ETC path in the Linux build. The census
//! over the 910 pack (`nxt::tests`) proves each file's bytes are consumed
//! exactly, and finds that archives 52, 54 and 55 store the texture at
//! `64 << k` plus a 32-pixel gutter on each side (192, 320, 576, 1088; 128
//! and 64 for a few), with every mip down to 1x1, while archive 53 has the
//! unguttered source size (64..1024).

use super::Cur;
use crate::cache::Pack;
use crate::texture::{
    TEXTURES_DXT_ARCHIVE, TEXTURES_ETC_ARCHIVE, TEXTURES_PNG_ARCHIVE, TEXTURES_PNG_MIPPED_ARCHIVE,
};

/// One of the four texture archives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TextureArchive {
    /// Archive 52, DDS.
    Dxt,
    /// Archive 53, PNG.
    Png,
    /// Archive 54, PNG mip chains.
    PngMipped,
    /// Archive 55, KTX.
    Etc,
}

impl TextureArchive {
    /// All four, in archive-id order.
    pub const ALL: [Self; 4] = [Self::Dxt, Self::Png, Self::PngMipped, Self::Etc];

    /// The pack file name (`client.<name>.js5`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Dxt => TEXTURES_DXT_ARCHIVE,
            Self::Png => TEXTURES_PNG_ARCHIVE,
            Self::PngMipped => TEXTURES_PNG_MIPPED_ARCHIVE,
            Self::Etc => TEXTURES_ETC_ARCHIVE,
        }
    }
}

/// Pixel or block format of a texture payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// DDS FourCC `DXT1` (BC1).
    Dxt1,
    /// DDS FourCC `DXT5` (BC3).
    Dxt5,
    /// KTX `0x8D64` ETC1 RGB8.
    Etc1Rgb8,
    /// KTX `0x9274` ETC2 RGB8.
    Etc2Rgb8,
    /// KTX `0x9278` ETC2 RGBA8 (EAC alpha).
    Etc2Rgba8,
    /// PNG with this IHDR colour type (0, 2, 3, 4 or 6) at 8 bits.
    Png {
        /// IHDR colour type.
        colour_type: u8,
    },
}

impl PixelFormat {
    /// Bytes per 4x4 block for the block formats, `None` for PNG.
    #[must_use]
    pub fn block_bytes(self) -> Option<usize> {
        match self {
            Self::Dxt1 | Self::Etc1Rgb8 | Self::Etc2Rgb8 => Some(8),
            Self::Dxt5 | Self::Etc2Rgba8 => Some(16),
            Self::Png { .. } => None,
        }
    }
}

/// What a texture file holds, from its headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureHeader {
    /// The frame version byte: 1 single, 6 cubemap (six frames).
    pub version: u8,
    /// Faces: 6 for a cubemap (six frames, or one DDS cubemap), else 1.
    pub faces: u8,
    /// Format of the first payload. Faces agree, except that PNG cube faces
    /// may differ in colour type.
    pub format: PixelFormat,
    /// Width of mip level 0.
    pub width: u32,
    /// Height of mip level 0.
    pub height: u32,
    /// Mip levels per face (1 for archive 53).
    pub mip_levels: u32,
}

/// Size in bytes of a `w x h` level in 4x4 blocks.
fn block_level_bytes(w: u32, h: u32, block: usize) -> usize {
    (w.div_ceil(4).max(1) as usize) * (h.div_ceil(4).max(1) as usize) * block
}

fn le32(bytes: &[u8], at: usize, what: &str) -> anyhow::Result<u32> {
    let word = bytes
        .get(at..at + 4)
        .ok_or_else(|| anyhow::anyhow!("{what}: truncated header at {at}"))?;
    Ok(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
}

/// Parse one DDS payload; the pixel data must be exactly `faces x` the mip
/// chain. Returns `(format, w, h, mips, faces)`.
fn dds(payload: &[u8], what: &str) -> anyhow::Result<(PixelFormat, u32, u32, u32, u8)> {
    anyhow::ensure!(payload.get(..4) == Some(b"DDS "), "{what}: bad DDS magic");
    anyhow::ensure!(
        le32(payload, 4, what)? == 124,
        "{what}: DDS header size != 124"
    );
    let flags = le32(payload, 8, what)?;
    let h = le32(payload, 12, what)?;
    let w = le32(payload, 16, what)?;
    let mips = if flags & 0x2_0000 != 0 {
        le32(payload, 28, what)?.max(1)
    } else {
        1
    };
    let pf_flags = le32(payload, 80, what)?;
    anyhow::ensure!(pf_flags & 0x4 != 0, "{what}: DDS is not FourCC-compressed");
    let format = match &payload[84..88] {
        b"DXT1" => PixelFormat::Dxt1,
        b"DXT5" => PixelFormat::Dxt5,
        other => anyhow::bail!("{what}: unsupported FourCC {other:?}"),
    };
    let caps2 = le32(payload, 112, what)?;
    let faces: u8 = match caps2 & 0xFE00 {
        0 => 1,
        0xFE00 => 6,
        partial => anyhow::bail!("{what}: partial cubemap caps2 {partial:#x}"),
    };
    let block = format.block_bytes().unwrap_or(16);
    let mut chain = 0_usize;
    for level in 0..mips {
        chain += block_level_bytes((w >> level).max(1), (h >> level).max(1), block);
    }
    let expected = 128 + usize::from(faces) * chain;
    anyhow::ensure!(
        payload.len() == expected,
        "{what}: DDS is {} bytes, header implies {expected}",
        payload.len()
    );
    Ok((format, w, h, mips, faces))
}

/// KTX 1.1 identifier.
const KTX_MAGIC: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x31, 0x31, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];

/// Parse one KTX payload, walking every mip level to the end.
fn ktx(payload: &[u8], what: &str) -> anyhow::Result<(PixelFormat, u32, u32, u32, u8)> {
    anyhow::ensure!(
        payload.get(..12) == Some(&KTX_MAGIC[..]),
        "{what}: bad KTX magic"
    );
    anyhow::ensure!(
        le32(payload, 12, what)? == 0x0403_0201,
        "{what}: only little-endian KTX"
    );
    anyhow::ensure!(
        le32(payload, 16, what)? == 0,
        "{what}: KTX glType != 0 (not compressed)"
    );
    let format = match le32(payload, 28, what)? {
        0x8D64 => PixelFormat::Etc1Rgb8,
        0x9274 => PixelFormat::Etc2Rgb8,
        0x9278 => PixelFormat::Etc2Rgba8,
        other => anyhow::bail!("{what}: unsupported KTX internal format {other:#06x}"),
    };
    let w = le32(payload, 36, what)?;
    let h = le32(payload, 40, what)?;
    anyhow::ensure!(le32(payload, 44, what)? == 0, "{what}: KTX depth != 0");
    anyhow::ensure!(le32(payload, 48, what)? == 0, "{what}: KTX array texture");
    let faces = le32(payload, 52, what)?;
    anyhow::ensure!(faces == 1 || faces == 6, "{what}: KTX faces {faces}");
    let mips = le32(payload, 56, what)?.max(1);
    let key_value = le32(payload, 60, what)? as usize;
    let block = format.block_bytes().unwrap_or(16);
    let mut pos = 64 + key_value;
    for level in 0..mips {
        let size = le32(payload, pos, what)? as usize;
        let expected = block_level_bytes((w >> level).max(1), (h >> level).max(1), block);
        anyhow::ensure!(
            size == expected,
            "{what}: KTX level {level} imageSize {size}, expected {expected}"
        );
        // Faces are padded to 4 bytes (cubePadding), then the level
        // (mipPadding); ETC levels are multiples of 8, so both are 0 here.
        pos += 4 + faces as usize * size.next_multiple_of(4);
    }
    anyhow::ensure!(
        pos == payload.len(),
        "{what}: KTX ends at {pos}, payload is {} bytes",
        payload.len()
    );
    Ok((format, w, h, mips, faces as u8))
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Parse one PNG's IHDR and walk its chunks to `IEND`, which must be the
/// last byte. Returns `(format, w, h)`.
fn png(payload: &[u8], what: &str) -> anyhow::Result<(PixelFormat, u32, u32)> {
    anyhow::ensure!(
        payload.get(..8) == Some(&PNG_SIGNATURE[..]),
        "{what}: bad PNG signature"
    );
    let be = |at: usize| -> anyhow::Result<u32> {
        let word = payload
            .get(at..at + 4)
            .ok_or_else(|| anyhow::anyhow!("{what}: truncated PNG at {at}"))?;
        Ok(u32::from_be_bytes([word[0], word[1], word[2], word[3]]))
    };
    anyhow::ensure!(
        payload.get(12..16) == Some(b"IHDR"),
        "{what}: first chunk is not IHDR"
    );
    let w = be(16)?;
    let h = be(20)?;
    let bit_depth = payload[24];
    let colour_type = payload[25];
    anyhow::ensure!(bit_depth == 8, "{what}: PNG bit depth {bit_depth}");
    let mut pos = 8;
    loop {
        let len = be(pos)? as usize;
        let kind = payload
            .get(pos + 4..pos + 8)
            .ok_or_else(|| anyhow::anyhow!("{what}: truncated chunk type at {pos}"))?;
        let end = pos + 12 + len;
        anyhow::ensure!(
            end <= payload.len(),
            "{what}: chunk at {pos} runs past the end"
        );
        let last = kind == b"IEND";
        pos = end;
        if last {
            break;
        }
    }
    anyhow::ensure!(
        pos == payload.len(),
        "{what}: {} bytes after IEND",
        payload.len() - pos
    );
    Ok((PixelFormat::Png { colour_type }, w, h))
}

/// Parse the headers of one texture file of `archive`, checking that its
/// framing and every payload consume the file exactly.
pub fn texture_header(
    archive: TextureArchive,
    id: u32,
    data: &[u8],
) -> anyhow::Result<TextureHeader> {
    let what = format!("{} texture {id}", archive.name());
    let mut c = Cur::new(data, &what);
    let version = c.g1("version")?;
    let frames: u8 = match version {
        1 => 1,
        6 => 6,
        other => anyhow::bail!("{what}: unknown version {other} (only 1 / 6)"),
    };
    let mut first: Option<(PixelFormat, u32, u32, u32, u8)> = None;
    for frame in 0..frames {
        let face = if archive == TextureArchive::PngMipped {
            let mips = c.g1("mip count")?;
            anyhow::ensure!(mips > 0, "{what}: face {frame} has no mips");
            let mut level0 = None;
            for level in 0..u32::from(mips) {
                let len = c.g4("mip length")? as usize;
                let at = c.pos();
                let payload = data
                    .get(at..at + len)
                    .ok_or_else(|| anyhow::anyhow!("{what}: mip {level} runs past the end"))?;
                let (format, w, h) = png(payload, &what)?;
                match level0 {
                    None => level0 = Some((format, w, h)),
                    Some((_, w0, h0)) => anyhow::ensure!(
                        (w, h) == ((w0 >> level).max(1), (h0 >> level).max(1)),
                        "{what}: mip {level} is {w}x{h}, level 0 is {w0}x{h0}"
                    ),
                }
                c.skip(len, "payload")?;
            }
            let (format, w, h) = level0.expect("mips > 0");
            (format, w, h, u32::from(mips), 1)
        } else {
            let len = c.g4("payload length")? as usize;
            let at = c.pos();
            let payload = data
                .get(at..at + len)
                .ok_or_else(|| anyhow::anyhow!("{what}: frame {frame} runs past the end"))?;
            let parsed = match archive {
                TextureArchive::Dxt => dds(payload, &what)?,
                TextureArchive::Etc => ktx(payload, &what)?,
                TextureArchive::Png | TextureArchive::PngMipped => {
                    let (format, w, h) = png(payload, &what)?;
                    (format, w, h, 1, 1)
                }
            };
            c.skip(len, "payload")?;
            parsed
        };
        match first {
            None => first = Some(face),
            // Cubemap faces agree in everything but a PNG's colour type
            // (the encoder picks one per face).
            Some(prev) => anyhow::ensure!(
                (prev.1, prev.2, prev.3, prev.4) == (face.1, face.2, face.3, face.4)
                    && (prev.0 == face.0
                        || matches!(
                            (prev.0, face.0),
                            (PixelFormat::Png { .. }, PixelFormat::Png { .. })
                        )),
                "{what}: frame {frame} {face:?} differs from frame 0 {prev:?}"
            ),
        }
    }
    c.finish()?;
    let (format, width, height, mip_levels, faces) = first.expect("frames > 0");
    Ok(TextureHeader {
        version,
        faces: if frames == 6 { 6 } else { faces },
        format,
        width,
        height,
        mip_levels,
    })
}

/// Headers of texture `id` from `archive` (its group holds one file).
pub fn load_texture_header(
    pack: &Pack,
    archive: TextureArchive,
    id: u32,
) -> anyhow::Result<TextureHeader> {
    let files = pack
        .read_group(archive.name(), id)
        .map_err(|error| anyhow::anyhow!("{} group {id}: {error}", archive.name()))?;
    anyhow::ensure!(
        files.len() == 1,
        "{} group {id} has {} files, expected 1",
        archive.name(),
        files.len()
    );
    let bytes = files.values().next().expect("one file");
    texture_header(archive, id, bytes)
}
