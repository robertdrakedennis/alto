//! Texture + material decoding for the 910 GPU path (std-only, no image crates).
//!
//! # Archive survey (verified against `server/data/pack`, rev 910)
//!
//! | archive id | file | bytes | groups | notes |
//! |---|---|---|---|---|
//! | 26 | `client.materials.js5` | 59 271 | 1 (`[0]`, 11 791 files) | material defs; see [`MaterialStore`] |
//! | 31 | `client.shaders.js5` | 18 247 | 2 (`[1, 3]`) | GLSL blobs; **not ported** (reference only) |
//! | 52 | `client.textures.dxt.js5` | 823 700 665 | 14 305 (ids from 5442) | ver + `u32be len` + DDS (DXT1/DXT5); raw only |
//! | 53 | `client.textures.png.js5` | 682 143 494 | 14 305 (ids from 5442) | ver + PNGs; **the 910 GPU path** |
//! | 54 | `client.textures.png.mipped.js5` | 2 174 915 616 | 14 253 (ids from 5442) | ver + mip chains of PNGs; RGBA8 |
//! | 55 | `client.textures.etc.js5` | 674 463 519 | 14 305 (ids from 5442) | ver + `u32be len` + KTX (ETC2); raw only |
//!
//! Group counts come from `Pack::read_archive_index`; byte sizes from the
//! on-disk files. The `#[ignore]` real-pack test below re-checks the
//! group counts and prints the live numbers.
//!
//! # Which archive the 910 GPU path uses
//!
//! The 910 client opens only the PNG textures archive (53) and builds its
//! material list from the materials archive (26) plus a texture list over the
//! PNG archive. The DXT, mipped-PNG and ETC archives are declared but never
//! opened. So the GPU path is **PNG**; DXT/ETC/MIPPED are not on it.
//!
//! # File framing
//!
//! Every texture file starts with a version byte selecting the texture kind
//! (`6` cubemap, `1` diffuse, `4` aux):
//!
//! * ver `1` (diffuse/aux): `01 | u32be png_len | PNG`.
//! * ver `6` (cubemap): `06 | 6 x (u32be png_len | PNG)`.
//!
//! The mipped archive (54) uses the same versions but prefixes every face with
//! a mip count: `ver | faces x (u8 mips | mips x (u32be len | PNG))` with 1
//! face for ver 1 and 6 for ver 6 (byte-exact consumption verified on real
//! groups 5443 and 5531). DXT/ETC reuse the ver-1/ver-6 framing with DDS/KTX
//! payloads instead of PNG.
//!
//! # PNG vs DXT vs ETC choice for wgpu
//!
//! PNG (+ mipped) decodes to [`RgbaImage`] (RGBA8) with the std-only decoder
//! in this module. Census over 877 real PNGs (every 17th group of archive 53):
//! bit depth always 8, interlace always 0, color types `{0 gray, 2 truecolor,
//! 3 indexed, 4 gray+alpha, 6 truecolor+alpha}`, chunk set always within
//! `{IHDR, PLTE, tRNS, IDAT, IEND}`, always a single IDAT, chunk CRCs 100%
//! valid, dimensions in `{64, 128, 256, 512}` squared. Anything outside that
//! envelope is a hard error naming the offending value.
//!
//! DXT (DDS `DXT1`/`DXT5`, mipmapped) and ETC (KTX `GL_COMPRESSED_RGBA8_ETC2_EAC`
//! `0x9278`, mipmapped) stay [`RawTexture`] blocks: there is no BC/ETC
//! transcoder in std, and wgpu's compressed formats (`Bc1RgbaUnorm` etc.)
//! consume exactly such blocks later. [`TextureFormat`] carries their tags.
//!
//! # Materials
//!
//! Archive 26 holds one group (0) with one file per material id (11 791
//! files; versions `{0: 11 167 RT5, 1: 624 RT7}`). The first byte selects the
//! RT5 or RT7 decoder; this module accepts `0`/`1` and errors on any other
//! version. The original client sizes its material array from the group
//! capacity and stores `null` for missing/undecodable files; [`MaterialStore`]
//! keeps the `None` slots but propagates decode errors (unknown version/enum is
//! an error, never a silent skip).
//!
//! Render note: materials carry no combine selector (the combine mode is
//! fixed-function shader state, not per-material data). The diffuse+aux
//! combine is `rgb * (aux/255*31+1)`, alpha passthrough.
//! [`Material::texture_ids`] gives `[diffuse?, aux?]` in that order.
//! `average_colour` is an opaque `u16` consumed through the client's HSL colour
//! table; it is kept raw.

use crate::cache::Pack;
use rs910_core::reader::{Eof, Reader as CoreReader};

// ---------------------------------------------------------------------------
// Archive names.
// ---------------------------------------------------------------------------

/// Js5 archive holding material defs (id 26).
pub const MATERIALS_ARCHIVE: &str = "materials";
/// Js5 archive the 910 GPU path samples (id 53).
pub const TEXTURES_PNG_ARCHIVE: &str = "textures.png";
/// Pre-mipped PNG variants (id 54).
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "Js5Archive id; production samples textures.png")
)]
pub const TEXTURES_PNG_MIPPED_ARCHIVE: &str = "textures.png.mipped";
/// S3TC/DDS variants (id 52). Raw only.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "Js5Archive id; production samples textures.png")
)]
pub const TEXTURES_DXT_ARCHIVE: &str = "textures.dxt";
/// ETC/KTX variants (id 55). Raw only.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "Js5Archive id; production samples textures.png")
)]
pub const TEXTURES_ETC_ARCHIVE: &str = "textures.etc";

/// Materials live in group 0 (file id = material id).
const MATERIALS_GROUP: u32 = 0;

/// Hard bound for decoded PNG dimensions (real data peaks at 512).
const MAX_DIM: u32 = 4096;
/// Hard bound for decoded pixels (stops zip-bombs before allocation).
const MAX_PIXELS: u64 = 16_777_216;

// ---------------------------------------------------------------------------
// Decoded images.
// ---------------------------------------------------------------------------

/// One RGBA8 image: row-major `w*h*4` bytes, top row first.
///
/// This is what the render agent uploads (`wgpu::TextureFormat::Rgba8Unorm`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
    /// RGBA bytes, `w*h*4` long.
    pub px: Vec<u8>,
}

impl RgbaImage {
    /// Pixel at `(x, y)` as `[r, g, b, a]`, or `None` when out of bounds.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code, reason = "exercised by tests only"))]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.w || y >= self.h {
            return None;
        }
        let i = (y as usize)
            .checked_mul(self.w as usize)?
            .checked_add(x as usize)?
            .checked_mul(4)?;
        let px = self.px.get(i..i + 4)?;
        Some([px[0], px[1], px[2], px[3]])
    }
}

/// One texture id: either a single diffuse/aux image or a 6-face cubemap.
///
/// Mirrors the ver-1 / ver-6 framing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Texture {
    /// ver `1`: a diffuse or aux image.
    Single(RgbaImage),
    /// ver `6`: a cubemap, faces in file order.
    Cube([RgbaImage; 6]),
}

/// Mip chains from `textures.png.mipped`: one entry per face (1 for ver 1, 6
/// for ver 6), each ordered largest mip first.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "textures.png.mipped loader; production samples textures.png (tested)"
    )
)]
pub struct MippedTexture {
    /// `faces[face][level]`; `level 0` is the full-size image.
    pub faces: Vec<Vec<RgbaImage>>,
}

// ---------------------------------------------------------------------------
// GPU block formats.
// ---------------------------------------------------------------------------

/// Block/pixel format tag for [`RawTexture`]. `index` is the format's tag
/// index (kept so wgpu mappings stay reviewable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code, reason = "complete texture format vocabulary")]
pub enum TextureFormat {
    /// Depth, index 0.
    Depth,
    /// `COMPRESSED_RGBA_S3TC_DXT1`, index 1 (DDS `DXT1`).
    Dxt1,
    /// RGB, index 2.
    Rgb,
    /// Alpha + luminance, index 3.
    AlphaLuminance,
    /// RGBA, index 4.
    Rgba,
    /// Index 5. Meaning unknown; kept for completeness.
    Format5,
    /// Alpha, index 6.
    Alpha,
    /// Luminance, index 7.
    Luminance,
    /// `COMPRESSED_RGBA_S3TC_DXT5`, index 8 (DDS `DXT5`).
    Dxt5,
    /// Index 9. Meaning unknown; kept for completeness.
    Format9,
    /// Extension (no original index): KTX `0x8D64` ETC1 RGB8.
    Etc1Rgb8,
    /// Extension (no original index): KTX `0x9274` ETC2 RGB8.
    Etc2Rgb8,
    /// Extension (no original index): KTX `0x9278` ETC2 RGBA8 (observed).
    Etc2Rgba8,
}

impl TextureFormat {
    /// Tag index; `None` for the ETC extensions (no original index).
    #[must_use]
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "TextureFormat accessors; exercised by tests only")
    )]
    pub fn index(self) -> Option<u32> {
        match self {
            Self::Depth => Some(0),
            Self::Dxt1 => Some(1),
            Self::Rgb => Some(2),
            Self::AlphaLuminance => Some(3),
            Self::Rgba => Some(4),
            Self::Format5 => Some(5),
            Self::Alpha => Some(6),
            Self::Luminance => Some(7),
            Self::Dxt5 => Some(8),
            Self::Format9 => Some(9),
            Self::Etc1Rgb8 | Self::Etc2Rgb8 | Self::Etc2Rgba8 => None,
        }
    }
}

/// Undecoded GPU block texture: dimensions + format tag + container bytes.
///
/// `bytes` is the whole payload after the 5-byte js5 framing (a complete DDS
/// file for DXT, a complete KTX file for ETC, headers included) so a future
/// transcoder gets everything `ImageIO`/DDS/KTX readers see. Round-trip rule:
/// `parse(format(bytes).bytes) == parse(bytes)`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "DXT/ETC passthrough; production samples textures.png (tested)"
    )
)]
pub struct RawTexture {
    /// Width in pixels, from the DDS/KTX header.
    pub w: u32,
    /// Height in pixels, from the DDS/KTX header.
    pub h: u32,
    /// Block format, from the DDS FourCC / KTX internal format.
    pub format: TextureFormat,
    /// Complete container bytes (header + mipmapped blocks).
    pub bytes: Vec<u8>,
}

/// Fixed-function combiner vocabulary (modes 0-4).
///
/// Vocabulary only: no combine selector is stored per material or texture
/// (see module docs). The render agent maps these to wgpu blend state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code, reason = "complete texture combine mode vocabulary")]
pub enum CombineMode {
    /// Mode 0.
    Mode0,
    /// Mode 1.
    Mode1,
    /// Mode 2.
    Mode2,
    /// Mode 3.
    Mode3,
    /// Mode 4.
    Mode4,
}

// ---------------------------------------------------------------------------
// Checksums (std-only; needed by the PNG + zlib paths).
// ---------------------------------------------------------------------------

// CRC-32 and Adler-32: `rs910_core::checksum`.
use rs910_core::checksum::{adler32, crc32};

// ---------------------------------------------------------------------------
// Minimal zlib inflate (RFC 1950 header/trailer + RFC 1951 deflate).
// ---------------------------------------------------------------------------

/// LSB-first bit reader over the deflate stream.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u32,
    n: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            acc: 0,
            n: 0,
        }
    }

    fn take(&mut self, count: u32) -> anyhow::Result<u32> {
        if count > 16 {
            return Err(anyhow::anyhow!(
                "deflate: bit read of {count} exceeds the 16-bit limit"
            ));
        }
        while self.n < count {
            let byte = *self
                .data
                .get(self.pos)
                .ok_or_else(|| anyhow::anyhow!("deflate: truncated bit stream"))?;
            self.pos += 1;
            self.acc |= u32::from(byte) << self.n;
            self.n += 8;
        }
        let mask = if count == 32 {
            u32::MAX
        } else {
            (1_u32 << count) - 1
        };
        let value = self.acc & mask;
        self.acc >>= count;
        self.n -= count;
        Ok(value)
    }

    /// Drop to the next byte boundary (stored blocks).
    fn align(&mut self) {
        let skip = self.n % 8;
        self.acc >>= skip;
        self.n -= skip;
    }

    /// Bytes consumed so far. `acc` holds the next `n` stream bits, already
    /// read from the first `n/8` bytes at `pos - n/8`, so integer division
    /// gives the exact resume point at any time.
    fn byte_pos(&self) -> usize {
        self.pos - (self.n / 8) as usize
    }
}

/// Canonical Huffman decode tree built from code lengths.
struct Huff {
    left: Vec<i32>,
    right: Vec<i32>,
    sym: Vec<i32>,
}

impl Huff {
    fn build(lengths: &[u8]) -> anyhow::Result<Self> {
        let max = lengths.iter().copied().max().unwrap_or(0);
        if max > 15 {
            return Err(anyhow::anyhow!(
                "deflate: Huffman code length {max} exceeds 15"
            ));
        }
        // Reject over-subscribed sets (Kraft); incomplete sets are legal.
        let mut counts = [0_i32; 16];
        for &len in lengths {
            counts[len as usize] += 1;
        }
        let mut left: i32 = 1;
        for &count in &counts[1..=usize::from(max)] {
            left <<= 1;
            left -= count;
            if left < 0 {
                return Err(anyhow::anyhow!("deflate: over-subscribed Huffman set"));
            }
        }
        // Canonical code assignment (RFC 1951 §3.2.2).
        let mut next_code = [0_u32; 16];
        let mut code = 0_u32;
        for len in 1..=usize::from(max) {
            code = (code + counts[len - 1] as u32) << 1;
            next_code[len] = code;
        }
        let mut tree = Self {
            left: vec![-1],
            right: vec![-1],
            sym: vec![-1],
        };
        for (symbol, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let code = next_code[len as usize];
            next_code[len as usize] += 1;
            let mut node = 0_usize;
            for bit in (0..len).rev() {
                let edge = ((code >> u32::from(bit)) & 1) as usize;
                let next = if edge == 0 {
                    tree.left[node]
                } else {
                    tree.right[node]
                };
                let next = if next < 0 {
                    let fresh = tree.left.len();
                    if fresh > 600 {
                        return Err(anyhow::anyhow!("deflate: Huffman tree overflow"));
                    }
                    tree.left.push(-1);
                    tree.right.push(-1);
                    tree.sym.push(-1);
                    if edge == 0 {
                        tree.left[node] = fresh as i32;
                    } else {
                        tree.right[node] = fresh as i32;
                    }
                    fresh
                } else {
                    next as usize
                };
                if tree.sym[next] >= 0 {
                    return Err(anyhow::anyhow!("deflate: Huffman code conflict"));
                }
                node = next;
            }
            if tree.left[node] >= 0 || tree.right[node] >= 0 {
                return Err(anyhow::anyhow!(
                    "deflate: Huffman code is a prefix of another"
                ));
            }
            tree.sym[node] = symbol as i32;
            let _ = code;
        }
        Ok(tree)
    }

    fn decode(&self, bits: &mut Bits<'_>) -> anyhow::Result<usize> {
        let mut node = 0_usize;
        loop {
            let leaf = self.sym[node];
            if leaf >= 0 {
                return Ok(leaf as usize);
            }
            let bit = bits.take(1)?;
            let next = if bit == 0 {
                self.left[node]
            } else {
                self.right[node]
            };
            if next < 0 {
                return Err(anyhow::anyhow!("deflate: invalid Huffman code"));
            }
            node = next as usize;
        }
    }
}

/// Fixed literal/length + distance tables (RFC 1951 §3.2.6).
fn fixed_tables() -> anyhow::Result<(Huff, Huff)> {
    let mut lit = [0_u8; 288];
    for (i, slot) in lit.iter_mut().enumerate() {
        *slot = if i <= 143 {
            8
        } else if i <= 255 {
            9
        } else if i <= 279 {
            7
        } else {
            8
        };
    }
    let dist = [5_u8; 32];
    Ok((Huff::build(&lit)?, Huff::build(&dist)?))
}

/// Code-length alphabet order (RFC 1951 §3.2.7).
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Dynamic literal/length + distance tables.
fn dynamic_tables(bits: &mut Bits<'_>) -> anyhow::Result<(Huff, Huff)> {
    let hlit = bits.take(5)? as usize + 257;
    let hdist = bits.take(5)? as usize + 1;
    let hclen = bits.take(4)? as usize + 4;
    let mut cl_len = [0_u8; 19];
    for &slot in CL_ORDER.iter().take(hclen) {
        cl_len[slot] = bits.take(3)? as u8;
    }
    let cl = Huff::build(&cl_len)?;
    let total = hlit
        .checked_add(hdist)
        .ok_or_else(|| anyhow::anyhow!("deflate: dynamic table size overflow"))?;
    if total > 320 {
        return Err(anyhow::anyhow!(
            "deflate: dynamic table declares {total} lengths past the 320 limit"
        ));
    }
    let mut lens: Vec<u8> = Vec::with_capacity(total);
    while lens.len() < total {
        let sym = cl.decode(bits)?;
        match sym {
            0..=15 => lens.push(sym as u8),
            16 => {
                let prev = *lens
                    .last()
                    .ok_or_else(|| anyhow::anyhow!("deflate: repeat with no previous length"))?;
                let count = bits.take(2)? as usize + 3;
                for _ in 0..count {
                    lens.push(prev);
                }
            }
            17 => {
                let count = bits.take(3)? as usize + 3;
                lens.extend(std::iter::repeat_n(0, count));
            }
            18 => {
                let count = bits.take(7)? as usize + 11;
                lens.extend(std::iter::repeat_n(0, count));
            }
            _ => {
                return Err(anyhow::anyhow!("deflate: bad code-length symbol {sym}"));
            }
        }
    }
    if lens.len() != total {
        return Err(anyhow::anyhow!("deflate: length run past the table end"));
    }
    Ok((Huff::build(&lens[..hlit])?, Huff::build(&lens[hlit..])?))
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// One Huffman-coded block; shared by fixed and dynamic blocks.
fn huffman_block(
    bits: &mut Bits<'_>,
    lit: &Huff,
    dist: &Huff,
    out: &mut Vec<u8>,
    limit: usize,
) -> anyhow::Result<()> {
    loop {
        let sym = lit.decode(bits)?;
        if sym < 256 {
            if out.len() >= limit {
                return Err(anyhow::anyhow!(
                    "deflate: output past the {}-byte limit",
                    limit
                ));
            }
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else if sym <= 285 {
            let i = sym - 257;
            let len = usize::from(LEN_BASE[i]) + bits.take(u32::from(LEN_EXTRA[i]))? as usize;
            let dsym = dist.decode(bits)?;
            if dsym > 29 {
                return Err(anyhow::anyhow!("deflate: reserved distance code {dsym}"));
            }
            let d = usize::from(DIST_BASE[dsym]) + bits.take(u32::from(DIST_EXTRA[dsym]))? as usize;
            if d == 0 || d > out.len() {
                return Err(anyhow::anyhow!(
                    "deflate: distance {d} past the {} decoded bytes",
                    out.len()
                ));
            }
            if out.len().checked_add(len).is_none_or(|end| end > limit) {
                return Err(anyhow::anyhow!(
                    "deflate: match past the {limit}-byte limit"
                ));
            }
            for _ in 0..len {
                let byte = out[out.len() - d];
                out.push(byte);
            }
        } else {
            return Err(anyhow::anyhow!("deflate: bad literal symbol {sym}"));
        }
    }
}

/// Raw deflate decode with an exact output cap.
pub fn inflate_raw(data: &[u8], limit: usize) -> anyhow::Result<(Vec<u8>, usize)> {
    let mut bits = Bits::new(data);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = bits.take(1)? != 0;
        let kind = bits.take(2)?;
        match kind {
            0 => {
                bits.align();
                let pos = bits.byte_pos();
                let header = data
                    .get(pos..pos + 4)
                    .ok_or_else(|| anyhow::anyhow!("deflate: truncated stored header"))?;
                let len = u16::from_le_bytes([header[0], header[1]]) as usize;
                let nlen = u16::from_le_bytes([header[2], header[3]]);
                if nlen != !(len as u16) {
                    return Err(anyhow::anyhow!("deflate: stored LEN/NLEN mismatch"));
                }
                let end = pos
                    .checked_add(4)
                    .and_then(|p| p.checked_add(len))
                    .ok_or_else(|| anyhow::anyhow!("deflate: stored block size overflow"))?;
                let body = data
                    .get(pos + 4..end)
                    .ok_or_else(|| anyhow::anyhow!("deflate: truncated stored block"))?;
                if out.len().checked_add(len).is_none_or(|e| e > limit) {
                    return Err(anyhow::anyhow!(
                        "deflate: stored block past the {limit}-byte limit"
                    ));
                }
                out.extend_from_slice(body);
                bits.pos = end;
            }
            1 => {
                let (lit, dist) = fixed_tables()?;
                huffman_block(&mut bits, &lit, &dist, &mut out, limit)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut bits)?;
                huffman_block(&mut bits, &lit, &dist, &mut out, limit)?;
            }
            _ => return Err(anyhow::anyhow!("deflate: reserved block type 3")),
        }
        if last {
            return Ok((out, bits.byte_pos()));
        }
    }
}

/// zlib (RFC 1950) decode: header checks + deflate + Adler-32.
///
/// `expected` is the exact inflated size; the output must match it byte for
/// byte (PNG scanlines have a known size, so this doubles as a zip-bomb cap).
fn inflate_zlib(data: &[u8], expected: usize) -> anyhow::Result<Vec<u8>> {
    if expected as u64 > MAX_PIXELS * 5 {
        return Err(anyhow::anyhow!(
            "zlib: {expected}-byte output past the decoder bound"
        ));
    }
    let head = data
        .get(..2)
        .ok_or_else(|| anyhow::anyhow!("zlib: truncated header"))?;
    let (cmf, flg) = (head[0], head[1]);
    if cmf & 0x0F != 8 {
        return Err(anyhow::anyhow!(
            "zlib: unsupported compression method {}",
            cmf & 0x0F
        ));
    }
    if (u16::from(cmf) * 256 + u16::from(flg)) % 31 != 0 {
        return Err(anyhow::anyhow!("zlib: bad header check bits"));
    }
    if flg & 0x20 != 0 {
        return Err(anyhow::anyhow!("zlib: preset dictionaries unsupported"));
    }
    let (out, pos) = inflate_raw(&data[2..], expected)?;
    if out.len() != expected {
        return Err(anyhow::anyhow!(
            "zlib: inflated {} bytes, expected {expected}",
            out.len()
        ));
    }
    let tail = data
        .get(2 + pos..2 + pos + 4)
        .ok_or_else(|| anyhow::anyhow!("zlib: truncated Adler-32 trailer"))?;
    let want = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]);
    let got = adler32(&out);
    if want != got {
        return Err(anyhow::anyhow!(
            "zlib: Adler-32 mismatch (stream {want:#010x} != computed {got:#010x})"
        ));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// PNG decode (8-bit, non-interlaced; color types 0/2/3/4/6).
// ---------------------------------------------------------------------------

/// PNG magic.
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn be_u32_at(data: &[u8], pos: usize, what: &str) -> anyhow::Result<u32> {
    let bytes = data
        .get(pos..pos + 4)
        .ok_or_else(|| anyhow::anyhow!("png: truncated {what} at offset {pos}"))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Bytes per pixel for an 8-bit color type.
fn png_bpp(color_type: u8) -> anyhow::Result<usize> {
    match color_type {
        0 | 3 => Ok(1),
        4 => Ok(2),
        2 => Ok(3),
        6 => Ok(4),
        other => Err(anyhow::anyhow!(
            "png: unsupported color type {other} (only 0/2/3/4/6 at 8-bit decode)"
        )),
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (i32::from(a), i32::from(b), i32::from(c));
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    (if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }) as u8
}

/// Undo PNG filtering; `filtered` holds `h` rows of `1 + stride` bytes.
fn unfilter(
    filtered: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    bpp: usize,
) -> anyhow::Result<Vec<u8>> {
    let row_len = stride
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("png: scanline size overflow for {w}x{h}"))?;
    if filtered.len()
        != row_len
            .checked_mul(h)
            .ok_or_else(|| anyhow::anyhow!("png: scanline size overflow for {w}x{h}"))?
    {
        return Err(anyhow::anyhow!(
            "png: filtered size {} != scanlines {row_len}x{h}",
            filtered.len()
        ));
    }
    let mut raw = vec![
        0_u8;
        stride.checked_mul(h).ok_or_else(|| {
            anyhow::anyhow!("png: image size overflow for {w}x{h}")
        })?
    ];
    let mut prior = vec![0_u8; stride];
    for row in 0..h {
        let line = &filtered[row * row_len..(row + 1) * row_len];
        let filter = line[0];
        if filter > 4 {
            return Err(anyhow::anyhow!(
                "png: unknown filter type {filter} on row {row}"
            ));
        }
        let out = &mut raw[row * stride..(row + 1) * stride];
        for i in 0..stride {
            let a = if i >= bpp { out[i - bpp] } else { 0 };
            let b = prior[i];
            let c = if i >= bpp { prior[i - bpp] } else { 0 };
            let f = line[1 + i];
            out[i] = match filter {
                0 => f,
                1 => f.wrapping_add(a),
                2 => f.wrapping_add(b),
                3 => f.wrapping_add(((u16::from(a) + u16::from(b)) / 2) as u8),
                _ => f.wrapping_add(paeth(a, b, c)),
            };
        }
        prior.copy_from_slice(out);
    }
    Ok(raw)
}

/// Decode one complete PNG file to RGBA8.
///
/// Accepts 8-bit depth, non-interlaced, color types 0/2/3/4/6 with
/// PLTE/tRNS as applicable; chunk CRCs are verified. Anything else errors
/// naming the color type, bit depth, or interlace value.
pub fn decode_png(data: &[u8]) -> anyhow::Result<RgbaImage> {
    if data.get(..8) != Some(&PNG_MAGIC[..]) {
        return Err(anyhow::anyhow!("png: bad 8-byte magic"));
    }
    let mut pos = 8_usize;
    let mut first = true;
    let mut seen_idat = false;
    let mut ended = false;
    let mut w = 0_u32;
    let mut h = 0_u32;
    let mut depth = 0_u8;
    let mut color_type = 0_u8;
    let mut interlace = 0_u8;
    let mut palette: Option<Vec<u8>> = None;
    let mut transparency: Option<Vec<u8>> = None;
    let mut idat: Vec<u8> = Vec::new();

    while !ended {
        let len = be_u32_at(data, pos, "chunk length")? as usize;
        let typ = data
            .get(pos + 4..pos + 8)
            .ok_or_else(|| anyhow::anyhow!("png: truncated chunk type at {pos}"))?;
        let body = data.get(pos + 8..pos + 8 + len).ok_or_else(|| {
            anyhow::anyhow!(
                "png: chunk {} declares {len} bytes past offset {}",
                String::from_utf8_lossy(typ),
                pos + 8
            )
        })?;
        let crc = be_u32_at(data, pos + 8 + len, "chunk CRC")?;
        let mut check = Vec::with_capacity(4 + len);
        check.extend_from_slice(typ);
        check.extend_from_slice(body);
        if crc32(&check) != crc {
            return Err(anyhow::anyhow!(
                "png: CRC mismatch in {} chunk at offset {pos}",
                String::from_utf8_lossy(typ)
            ));
        }
        match typ {
            b"IHDR" => {
                if !first {
                    return Err(anyhow::anyhow!("png: duplicate IHDR"));
                }
                if len != 13 {
                    return Err(anyhow::anyhow!("png: IHDR length {len} != 13"));
                }
                w = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                h = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
                depth = body[8];
                color_type = body[9];
                interlace = body[12];
            }
            b"PLTE" => {
                if palette.is_none() {
                    palette = Some(body.to_vec());
                }
            }
            b"tRNS" => {
                if transparency.is_none() {
                    transparency = Some(body.to_vec());
                }
            }
            b"IDAT" => {
                seen_idat = true;
                idat.extend_from_slice(body);
            }
            b"IEND" => {
                if len != 0 {
                    return Err(anyhow::anyhow!("png: IEND length {len} != 0"));
                }
                ended = true;
            }
            _ => {
                // Critical chunks (uppercase first letter) must be understood.
                if typ[0] & 0x20 == 0 {
                    return Err(anyhow::anyhow!(
                        "png: unsupported critical chunk {}",
                        String::from_utf8_lossy(typ)
                    ));
                }
            }
        }
        first = false;
        pos = (pos + 8 + len)
            .checked_add(4)
            .ok_or_else(|| anyhow::anyhow!("png: chunk offset overflow"))?;
    }

    if w == 0 || h == 0 {
        return Err(anyhow::anyhow!("png: missing IHDR"));
    }
    if !seen_idat {
        return Err(anyhow::anyhow!("png: no IDAT data"));
    }
    if depth != 8 {
        return Err(anyhow::anyhow!(
            "png: unsupported bit depth {depth} (only 8-bit decodes)"
        ));
    }
    let interlace_method = interlace;
    if interlace_method != 0 {
        return Err(anyhow::anyhow!(
            "png: unsupported interlace method {interlace_method} (only 0 decodes)"
        ));
    }
    let bpp = png_bpp(color_type)?;
    if w > MAX_DIM || h > MAX_DIM {
        return Err(anyhow::anyhow!(
            "png: dimensions {w}x{h} past the {MAX_DIM}px bound"
        ));
    }
    let pixels = u64::from(w) * u64::from(h);
    if pixels > MAX_PIXELS {
        return Err(anyhow::anyhow!(
            "png: {pixels} pixels past the {MAX_PIXELS} bound"
        ));
    }
    let (w_usize, h_usize) = (w as usize, h as usize);
    let stride = w_usize
        .checked_mul(bpp)
        .ok_or_else(|| anyhow::anyhow!("png: scanline size overflow for {w}x{h}"))?;

    if color_type == 3 {
        let palette = palette
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("png: indexed image without PLTE"))?;
        if palette.is_empty() || palette.len() % 3 != 0 || palette.len() > 768 {
            return Err(anyhow::anyhow!(
                "png: malformed PLTE ({} bytes)",
                palette.len()
            ));
        }
        if let Some(trns) = &transparency {
            if trns.len() > palette.len() / 3 {
                return Err(anyhow::anyhow!(
                    "png: tRNS ({} bytes) longer than the palette ({} entries)",
                    trns.len(),
                    palette.len() / 3
                ));
            }
        }
    } else if color_type == 0 || color_type == 2 {
        if let Some(trns) = &transparency {
            let want = if color_type == 0 { 2 } else { 6 };
            if trns.len() != want {
                return Err(anyhow::anyhow!(
                    "png: tRNS length {} != {want} for color type {color_type}",
                    trns.len()
                ));
            }
        }
    } else if transparency.is_some() {
        return Err(anyhow::anyhow!(
            "png: tRNS with color type {color_type} (alpha already present)"
        ));
    }

    let expected = h_usize
        .checked_mul(stride + 1)
        .ok_or_else(|| anyhow::anyhow!("png: filtered size overflow for {w}x{h}"))?;
    let filtered = inflate_zlib(&idat, expected)?;
    let raw = unfilter(&filtered, w_usize, h_usize, stride, bpp)?;

    let mut px = Vec::with_capacity(w_usize * h_usize * 4);
    match color_type {
        0 => {
            let key = transparency
                .as_ref()
                .map(|t| u16::from_be_bytes([t[0], t[1]]));
            for &g in &raw {
                let a = match key {
                    Some(k) if u16::from(g) == k => 0,
                    _ => 255,
                };
                px.extend_from_slice(&[g, g, g, a]);
            }
        }
        2 => {
            let key = transparency
                .as_ref()
                .map(|t| (t[0], t[1], t[2], t[3], t[4], t[5]));
            for triple in raw.chunks_exact(3) {
                let a = match key {
                    Some((r0, r1, g0, g1, b0, b1))
                        if u16::from_be_bytes([r0, r1]) == u16::from(triple[0])
                            && u16::from_be_bytes([g0, g1]) == u16::from(triple[1])
                            && u16::from_be_bytes([b0, b1]) == u16::from(triple[2]) =>
                    {
                        0
                    }
                    _ => 255,
                };
                px.extend_from_slice(&[triple[0], triple[1], triple[2], a]);
            }
        }
        3 => {
            // Validated above (indexed without PLTE is an error); re-check
            // here so the invariant holds even if validation moves.
            let palette = palette
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("png: indexed image without PLTE"))?;
            let entries = palette.len() / 3;
            let trns = transparency.as_deref().unwrap_or(&[]);
            for &index in &raw {
                if usize::from(index) >= entries {
                    return Err(anyhow::anyhow!(
                        "png: palette index {index} past {entries} entries"
                    ));
                }
                let i = usize::from(index) * 3;
                let a = trns.get(usize::from(index)).copied().unwrap_or(255);
                px.extend_from_slice(&[palette[i], palette[i + 1], palette[i + 2], a]);
            }
        }
        4 => {
            for pair in raw.chunks_exact(2) {
                px.extend_from_slice(&[pair[0], pair[0], pair[0], pair[1]]);
            }
        }
        6 => px.extend_from_slice(&raw),
        other => {
            return Err(anyhow::anyhow!(
                "png: unsupported color type {other} (only 0/2/3/4/6 at 8-bit decode)"
            ));
        }
    }

    Ok(RgbaImage { w, h, px })
}

// ---------------------------------------------------------------------------
// Texture file framing + pack loaders.
// ---------------------------------------------------------------------------

/// Split one `textures.png` file into its PNG payloads (ver 1 -> one, ver 6
/// -> six). Length prefixes must consume the file exactly.
fn split_png_file(data: &[u8]) -> anyhow::Result<Vec<&[u8]>> {
    let ver = *data
        .first()
        .ok_or_else(|| anyhow::anyhow!("texture: empty file"))?;
    let faces = match ver {
        1 => 1,
        6 => 6,
        other => {
            return Err(anyhow::anyhow!(
                "texture: unknown version {other} (only ver 1 single / ver 6 cube decode)"
            ));
        }
    };
    let mut out = Vec::with_capacity(faces);
    let mut pos = 1_usize;
    for face in 0..faces {
        let len = be_u32_at(data, pos, "face length")? as usize;
        pos += 4;
        let png = data.get(pos..pos + len).ok_or_else(|| {
            anyhow::anyhow!("texture: face {face} declares {len} bytes past the file end")
        })?;
        pos += len;
        out.push(png);
    }
    if pos != data.len() {
        return Err(anyhow::anyhow!(
            "texture: {ver}-face file has {} trailing bytes",
            data.len() - pos
        ));
    }
    Ok(out)
}

/// Read one file's bytes from a texture archive.
fn texture_file_bytes(
    pack: &Pack,
    archive: &str,
    group: u32,
    file: u32,
) -> anyhow::Result<Vec<u8>> {
    let files = pack
        .read_group(archive, group)
        .map_err(|error| anyhow::anyhow!("texture: group {group} in {archive:?}: {error}"))?;
    files
        .get(&file)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("texture: group {group} in {archive:?} has no file {file}"))
}

/// Decode the single-PNG texture `(group, file)` from `textures.png`.
///
/// Errors on cubemap (ver 6) files: use [`load_cube`] or [`load_texture`].
pub fn load_png(pack: &Pack, group: u32, file: u32) -> anyhow::Result<RgbaImage> {
    let bytes = texture_file_bytes(pack, TEXTURES_PNG_ARCHIVE, group, file)?;
    let faces = split_png_file(&bytes)?;
    if faces.len() != 1 {
        return Err(anyhow::anyhow!(
            "texture: group {group} file {file} is a {}-face cubemap, not a single PNG (use load_cube)",
            faces.len()
        ));
    }
    decode_png(faces[0])
}

/// Decode the 6-face cubemap `(group, file)` from `textures.png`, in file order.
pub fn load_cube(pack: &Pack, group: u32, file: u32) -> anyhow::Result<[RgbaImage; 6]> {
    let bytes = texture_file_bytes(pack, TEXTURES_PNG_ARCHIVE, group, file)?;
    let faces = split_png_file(&bytes)?;
    if faces.len() != 6 {
        return Err(anyhow::anyhow!(
            "texture: group {group} file {file} is a single PNG, not a cubemap (use load_png)"
        ));
    }
    let mut images = Vec::with_capacity(6);
    for face in &faces {
        images.push(decode_png(face)?);
    }
    let images: Vec<RgbaImage> = images;
    // Length is checked above; indexing is exact.
    Ok([
        images[0].clone(),
        images[1].clone(),
        images[2].clone(),
        images[3].clone(),
        images[4].clone(),
        images[5].clone(),
    ])
}

/// Decode texture `id` (== group id, file 0) from `textures.png`,
/// dispatching on the version byte.
pub fn load_texture(pack: &Pack, id: u32) -> anyhow::Result<Texture> {
    let bytes = texture_file_bytes(pack, TEXTURES_PNG_ARCHIVE, id, 0)?;
    let ver = *bytes
        .first()
        .ok_or_else(|| anyhow::anyhow!("texture: group {id} file 0 is empty"))?;
    match ver {
        1 => Ok(Texture::Single(load_png(pack, id, 0)?)),
        6 => Ok(Texture::Cube(load_cube(pack, id, 0)?)),
        other => Err(anyhow::anyhow!(
            "texture: group {id} has unknown version {other} (only ver 1 single / ver 6 cube decode)"
        )),
    }
}

/// Decode the mip chain(s) of `(group, file)` from `textures.png.mipped`.
///
/// Framing: `ver | faces x (u8 mips | mips x (u32be len | PNG))`, 1 face for
/// ver 1, 6 for ver 6. Level 0 of each face is the full-size image.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "textures.png.mipped loader; production samples textures.png (tested)"
    )
)]
pub fn load_mipped(pack: &Pack, group: u32, file: u32) -> anyhow::Result<MippedTexture> {
    let bytes = texture_file_bytes(pack, TEXTURES_PNG_MIPPED_ARCHIVE, group, file)?;
    let ver = *bytes
        .first()
        .ok_or_else(|| anyhow::anyhow!("texture: mipped group {group} file {file} is empty"))?;
    let faces = match ver {
        1 => 1,
        6 => 6,
        other => {
            return Err(anyhow::anyhow!(
                "texture: mipped group {group} has unknown version {other}"
            ));
        }
    };
    let mut pos = 1_usize;
    let mut out: Vec<Vec<RgbaImage>> = Vec::with_capacity(faces);
    for face in 0..faces {
        let mips = *bytes.get(pos).ok_or_else(|| {
            anyhow::anyhow!("texture: mipped group {group} face {face} truncates the mip count")
        })? as usize;
        pos += 1;
        let mut levels = Vec::with_capacity(mips);
        for level in 0..mips {
            let len = be_u32_at(&bytes, pos, "mip length")? as usize;
            pos += 4;
            let png = bytes.get(pos..pos + len).ok_or_else(|| {
                anyhow::anyhow!(
                    "texture: mipped group {group} face {face} level {level} declares {len} bytes past the file end"
                )
            })?;
            pos += len;
            levels.push(decode_png(png)?);
        }
        out.push(levels);
    }
    if pos != bytes.len() {
        return Err(anyhow::anyhow!(
            "texture: mipped group {group} file {file} has {} trailing bytes",
            bytes.len() - pos
        ));
    }
    Ok(MippedTexture { faces: out })
}

// ---------------------------------------------------------------------------
// DXT / ETC raw passthrough.
// ---------------------------------------------------------------------------

/// Parse one DDS payload (after the js5 framing) into a [`RawTexture`].
/// Only `DXT1`/`DXT5` FourCCs map; `bytes` keeps the whole DDS file.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "DXT passthrough; production samples textures.png (tested)"
    )
)]
pub fn parse_dxt(data: &[u8]) -> anyhow::Result<RawTexture> {
    if data.get(..4) != Some(b"DDS ".as_slice()) {
        return Err(anyhow::anyhow!("dxt: bad DDS magic"));
    }
    let size = u32::from_le_bytes(
        data.get(4..8)
            .ok_or_else(|| anyhow::anyhow!("dxt: truncated DDS header"))?
            .try_into()
            .map_err(|_| anyhow::anyhow!("dxt: truncated DDS header"))?,
    );
    if size != 124 {
        return Err(anyhow::anyhow!("dxt: DDS header size {size} != 124"));
    }
    let word = |at: usize, what: &str| -> anyhow::Result<u32> {
        data.get(at..at + 4)
            .ok_or_else(|| anyhow::anyhow!("dxt: truncated DDS {what}"))?
            .try_into()
            .map(|w: [u8; 4]| u32::from_le_bytes(w))
            .map_err(|_| anyhow::anyhow!("dxt: truncated DDS {what}"))
    };
    let h = word(12, "height")?;
    let w = word(16, "width")?;
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err(anyhow::anyhow!("dxt: bad dimensions {w}x{h}"));
    }
    let flags = word(80, "pixel-format flags")?;
    if flags & 0x4 == 0 {
        return Err(anyhow::anyhow!(
            "dxt: DDS pixel format {flags:#x} is not FourCC-compressed"
        ));
    }
    let fourcc = word(84, "FourCC")?;
    let format = match &fourcc.to_le_bytes() {
        b"DXT1" => TextureFormat::Dxt1,
        b"DXT5" => TextureFormat::Dxt5,
        _ => {
            return Err(anyhow::anyhow!(
                "dxt: unsupported FourCC {:?} (only DXT1/DXT5 map)",
                String::from_utf8_lossy(&fourcc.to_le_bytes())
            ));
        }
    };
    Ok(RawTexture {
        w,
        h,
        format,
        bytes: data.to_vec(),
    })
}

/// KTX magic (`AB 4B 54 58 20 31 31 BB 0D 0A 1A 0A`).
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "KTX passthrough; production samples textures.png (tested)"
    )
)]
const KTX_MAGIC: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x31, 0x31, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];

/// Parse one KTX payload (after the js5 framing) into a [`RawTexture`].
/// Only little-endian compressed ETC textures map; `bytes` keeps the whole
/// KTX file.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "ETC passthrough; production samples textures.png (tested)"
    )
)]
pub fn parse_etc(data: &[u8]) -> anyhow::Result<RawTexture> {
    if data.get(..12) != Some(&KTX_MAGIC[..]) {
        return Err(anyhow::anyhow!("etc: bad KTX magic"));
    }
    let word = |at: usize, what: &str| -> anyhow::Result<u32> {
        data.get(at..at + 4)
            .ok_or_else(|| anyhow::anyhow!("etc: truncated KTX {what}"))?
            .try_into()
            .map(|w: [u8; 4]| u32::from_le_bytes(w))
            .map_err(|_| anyhow::anyhow!("etc: truncated KTX {what}"))
    };
    if word(12, "endianness")? != 0x0403_0201 {
        return Err(anyhow::anyhow!("etc: only little-endian KTX decodes"));
    }
    if word(16, "glType")? != 0 {
        return Err(anyhow::anyhow!("etc: only compressed (glType 0) KTX maps"));
    }
    let format = match word(28, "internal format")? {
        0x8D64 => TextureFormat::Etc1Rgb8,
        0x9274 => TextureFormat::Etc2Rgb8,
        0x9278 => TextureFormat::Etc2Rgba8,
        other => {
            return Err(anyhow::anyhow!(
                "etc: unsupported KTX internal format {other:#06x}"
            ));
        }
    };
    let w = word(36, "width")?;
    let h = word(40, "height")?;
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err(anyhow::anyhow!("etc: bad dimensions {w}x{h}"));
    }
    Ok(RawTexture {
        w,
        h,
        format,
        bytes: data.to_vec(),
    })
}

/// Split one DXT/ETC file into payloads (ver 1 -> one, ver 6 -> six).
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "DXT/ETC passthrough; production samples textures.png (tested)"
    )
)]
fn split_block_file<'a>(data: &'a [u8], archive: &str) -> anyhow::Result<Vec<&'a [u8]>> {
    let ver = *data
        .first()
        .ok_or_else(|| anyhow::anyhow!("{archive}: empty file"))?;
    let faces = match ver {
        1 => 1,
        6 => 6,
        other => {
            return Err(anyhow::anyhow!(
                "{archive}: unknown version {other} (only ver 1 / ver 6 split)"
            ));
        }
    };
    let mut out = Vec::with_capacity(faces);
    let mut pos = 1_usize;
    for face in 0..faces {
        let len = be_u32_at(data, pos, "face length")? as usize;
        pos += 4;
        let body = data.get(pos..pos + len).ok_or_else(|| {
            anyhow::anyhow!("{archive}: face {face} declares {len} bytes past the file end")
        })?;
        pos += len;
        out.push(body);
    }
    if pos != data.len() {
        return Err(anyhow::anyhow!(
            "{archive}: file has {} trailing bytes",
            data.len() - pos
        ));
    }
    Ok(out)
}

/// Raw DXT blocks of `(group, file)` from `textures.dxt` (one per face).
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "DXT passthrough; production samples textures.png (tested)"
    )
)]
pub fn load_dxt(pack: &Pack, group: u32, file: u32) -> anyhow::Result<Vec<RawTexture>> {
    let bytes = texture_file_bytes(pack, TEXTURES_DXT_ARCHIVE, group, file)?;
    split_block_file(&bytes, TEXTURES_DXT_ARCHIVE)?
        .iter()
        .map(|payload| parse_dxt(payload))
        .collect()
}

/// Raw ETC blocks of `(group, file)` from `textures.etc` (one per face).
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "ETC passthrough; production samples textures.png (tested)"
    )
)]
pub fn load_etc(pack: &Pack, group: u32, file: u32) -> anyhow::Result<Vec<RawTexture>> {
    let bytes = texture_file_bytes(pack, TEXTURES_ETC_ARCHIVE, group, file)?;
    split_block_file(&bytes, TEXTURES_ETC_ARCHIVE)?
        .iter()
        .map(|payload| parse_etc(payload))
        .collect()
}

// ---------------------------------------------------------------------------
// Materials.
// ---------------------------------------------------------------------------

/// Alpha selector (`NONE = 0`, `ALPHA_TESTED = 1`, `MULTIPLY = 2`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaMode {
    /// Opaque.
    None,
    /// Alpha-tested against [`Material::alpha_threshold`].
    AlphaTested,
    /// Multiplied blending.
    Multiply,
}

impl AlphaMode {
    fn decode(id: u8) -> anyhow::Result<Self> {
        match id {
            0 => Ok(Self::None),
            1 => Ok(Self::AlphaTested),
            2 => Ok(Self::Multiply),
            other => Err(anyhow::anyhow!("material: unknown alpha mode {other}")),
        }
    }
}

/// One material: texture mapping + sampling parameters for the render agent.
///
/// Holds the fields the texture combine and the toolkits read:
/// [`Material::texture_ids`] is `[diffuse?, aux?]` (the diffuse map gated by
/// flags `0x1|0x10`, the aux map gated by `0x2|0x8` in RT5; `0x20`-gated
/// diffuse in RT7). `size` is the tile size (`Some(64|128|256|512|1024)`,
/// `None` for an unknown code); `average_colour` is the opaque HSL colour
/// table key kept raw.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Material id (== file id in group 0 of the materials archive).
    pub id: u32,
    /// Texture ids in combine order: diffuse first, aux second.
    pub texture_ids: Vec<u32>,
    /// Diffuse map texture id (`-1`/none becomes `None`).
    pub diffuse_texture: Option<u32>,
    /// The aux (height/spec) map id.
    pub aux_texture: Option<u32>,
    /// Tile size; `None` for an unknown code.
    pub size: Option<u32>,
    /// Opaque average-colour key (HSL colour table domain).
    pub average_colour: u16,
    /// Alpha selector.
    pub alpha: AlphaMode,
    /// Alpha-test threshold (a signed byte, default `-1`); raw bits kept.
    pub alpha_threshold: u8,
    /// Repeat modes S / T (3 bits each).
    pub repeat_s: u8,
    /// See [`Material::repeat_s`].
    pub repeat_t: u8,
    /// Scroll speeds (`g2s * 127 / 32767 / 64`, f32, in that operation order).
    pub speed_u: f32,
    /// See [`Material::speed_u`].
    pub speed_v: f32,
    /// HD flag (RT7 quality mode HD).
    pub high_detail: bool,
    /// Environment cubemap flag and mip mode.
    pub environment_cube: bool,
    pub mip_mode: u8,
    /// LD flag (RT7 quality mode LD).
    pub low_detail: bool,
    /// Effect (RT5 effect block only; RT7 leaves 0): 1 reflective, 2 water,
    /// 4/8/9 sea, 6 alpha, 7 env-mapped. A signed byte, raw bits kept.
    pub effect: u8,
    /// Effect parameter: wave-speed exponent / env arg.
    pub effect_param: u8,
    /// Brightness boost, used as `(boost & 0xFF) + 256` per channel by the
    /// floor and model material passes.
    pub brightness_boost: u8,
    /// Blend-toward-grey amount (`& 0xFF`).
    pub grey_blend: u8,
    /// RT5's second flag word (the 910 client reads only its `0x800`/`0x10`
    /// bits; RT7 leaves 0). NXT 950-1 keeps the whole word: bit `0x2` is the
    /// RT5 environment-mapping flag (lane Q-AOENV,
    /// `rs910_render_modern::nxt_rt5env950`).
    pub flags2: u32,
}

/// Big-endian cursor over one material file.
struct Cursor<'a> {
    id: u32,
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(id: u32, data: &'a [u8]) -> Self {
        Self { id, data, pos: 0 }
    }

    /// One `rs910_core::reader` read of `count` bytes at `pos`, all or
    /// nothing (the arithmetic lives there).
    fn read<T>(
        &mut self,
        count: usize,
        what: &str,
        read: impl FnOnce(&mut CoreReader<'a>) -> Result<T, Eof>,
    ) -> anyhow::Result<T> {
        let mut r = CoreReader::at(self.data, self.pos);
        match r.atomic(read) {
            Ok(value) => {
                self.pos = r.pos();
                Ok(value)
            }
            Err(_) if self.pos.checked_add(count).is_none() => Err(anyhow::anyhow!(
                "material {}: offset overflow reading {what}",
                self.id
            )),
            Err(_) => Err(anyhow::anyhow!(
                "material {}: truncated {what} at offset {} (len {})",
                self.id,
                self.pos,
                self.data.len()
            )),
        }
    }

    fn g1(&mut self) -> anyhow::Result<u8> {
        self.read(1, "u8", CoreReader::g1)
    }

    fn g2(&mut self) -> anyhow::Result<u16> {
        self.read(2, "u16", CoreReader::g2)
    }

    fn g2s(&mut self) -> anyhow::Result<i16> {
        self.read(2, "i16", CoreReader::g2s)
    }

    fn g4s(&mut self) -> anyhow::Result<i32> {
        self.read(4, "i32", CoreReader::g4s)
    }

    fn gfloat(&mut self) -> anyhow::Result<f32> {
        self.read(4, "f32", CoreReader::gfloat)
    }
}

/// Size code -> tile size (`None` for an unknown code).
fn material_size(code: u8) -> Option<u32> {
    match code {
        0 => Some(64),
        1 => Some(128),
        2 => Some(256),
        3 => Some(512),
        4 => Some(1024),
        _ => None,
    }
}

/// Working state shared by the RT5/RT7 decoders (field order == wire order).
#[derive(Default)]
struct RawFields {
    diffuse: Option<u32>,
    aux: Option<u32>,
    size: Option<u32>,
    average_colour: u16,
    alpha: Option<AlphaMode>,
    alpha_threshold: u8,
    repeat_s: u8,
    repeat_t: u8,
    speed_u: f32,
    speed_v: f32,
    high_detail: bool,
    environment_cube: bool,
    mip_mode: u8,
    low_detail: bool,
    effect: u8,
    effect_param: u8,
    brightness_boost: u8,
    grey_blend: u8,
    flags2: u32,
}

impl RawFields {
    fn finish(self, id: u32) -> anyhow::Result<Material> {
        let alpha = self
            .alpha
            .ok_or_else(|| anyhow::anyhow!("material {id}: missing alpha mode (truncated file)"))?;
        let mut texture_ids = Vec::with_capacity(2);
        if let Some(t) = self.diffuse {
            texture_ids.push(t);
        }
        if let Some(t) = self.aux {
            texture_ids.push(t);
        }
        Ok(Material {
            id,
            texture_ids,
            diffuse_texture: self.diffuse,
            aux_texture: self.aux,
            size: self.size,
            average_colour: self.average_colour,
            alpha,
            alpha_threshold: self.alpha_threshold,
            repeat_s: self.repeat_s,
            repeat_t: self.repeat_t,
            speed_u: self.speed_u,
            speed_v: self.speed_v,
            high_detail: self.high_detail,
            environment_cube: self.environment_cube,
            mip_mode: self.mip_mode,
            low_detail: self.low_detail,
            effect: self.effect,
            effect_param: self.effect_param,
            brightness_boost: self.brightness_boost,
            grey_blend: self.grey_blend,
            flags2: self.flags2,
        })
    }
}

fn opt_texture_id(raw: i32) -> Option<u32> {
    u32::try_from(raw).ok()
}

/// Decode an RT5 material, field order kept.
fn decode_rt5(id: u32, cursor: &mut Cursor<'_>) -> anyhow::Result<Material> {
    let mut f = RawFields {
        alpha_threshold: 0xFF,
        // The grey blend starts at -1 even when the RT5 effect block is absent.
        grey_blend: 0xFF,
        ..RawFields::default()
    };
    let _skipped = cursor.g1()?;
    let size_code = cursor.g1()?;
    f.size = material_size(size_code);
    let flags = cursor.g4s()?;
    let has_diffuse_alpha = flags & 0x1 != 0;
    let has_aux_a = flags & 0x2 != 0;
    let has_aux_b = flags & 0x8 != 0;
    let has_diffuse_b = flags & 0x10 != 0;
    f.environment_cube = has_diffuse_b;
    if has_diffuse_alpha || has_diffuse_b {
        f.diffuse = opt_texture_id(cursor.g4s()?);
    }
    if has_aux_b || has_aux_a {
        f.aux = opt_texture_id(cursor.g4s()?);
    }
    let repeat = cursor.g1()?;
    f.repeat_s = repeat & 0x7;
    f.repeat_t = (repeat >> 3) & 0x7;
    let second_flags = cursor.g4s()?;
    f.flags2 = second_flags as u32;
    let specular = second_flags & 0x800 != 0;
    if second_flags & 0x10 != 0 {
        let _ = cursor.gfloat()?;
        let _ = cursor.gfloat()?;
    }
    if has_aux_a {
        let _ = cursor.gfloat()?;
    }
    let _scroll = cursor.g1()? == 1;
    let _facet = facet_id(cursor.g1()?)?;
    f.alpha = Some(AlphaMode::decode(cursor.g1()?)?);
    if f.alpha == Some(AlphaMode::AlphaTested) {
        f.alpha_threshold = cursor.g1()?;
    }
    if specular {
        let _ = cursor.gfloat()?;
        let _ = cursor.gfloat()?;
        let _ = cursor.gfloat()?;
    }
    let speed_flags = cursor.g1()?;
    if speed_flags & 0x1 != 0 {
        f.speed_u = cursor.g2s()? as f32 * 127.0 / 32767.0 / 64.0;
    }
    if speed_flags & 0x2 != 0 {
        f.speed_v = cursor.g2s()? as f32 * 127.0 / 32767.0 / 64.0;
    }
    if cursor.g1()? != 1 {
        return f.finish(id);
    }
    f.effect = cursor.g1()?;
    f.effect_param = cursor.g1()?;
    let _ignored_word = cursor.g4s()?;
    let _ignored_flag = cursor.g1()?;
    let _skipped = cursor.g1()?;
    f.mip_mode = cursor.g1()?;
    f.low_detail = cursor.g1()? == 1;
    f.high_detail = cursor.g1()? == 1;
    f.brightness_boost = cursor.g1()?;
    f.grey_blend = cursor.g1()?;
    f.average_colour = cursor.g2()?;
    f.finish(id)
}

/// Facet ids (0-5); the value is discarded by both decoders but unknown ids
/// are still an error.
fn facet_id(raw: u8) -> anyhow::Result<u8> {
    if raw <= 5 {
        Ok(raw)
    } else {
        Err(anyhow::anyhow!("material: unknown facet mode {raw}"))
    }
}

/// Decode an RT7 material, field order kept.
fn decode_rt7(id: u32, cursor: &mut Cursor<'_>) -> anyhow::Result<Material> {
    let mut f = RawFields {
        alpha_threshold: 0xFF,
        // RT7 keeps the default signed -1; the model material pass consumes it as
        // the unsigned grey blend 255.
        grey_blend: 0xFF,
        ..RawFields::default()
    };
    let flags = cursor.g4s()?;
    let specular = flags & 0x800 != 0;
    if flags & 0x20 != 0 {
        f.diffuse = {
            let _skipped = cursor.g1()?;
            opt_texture_id(cursor.g4s()?)
        };
    }
    if flags & 0x40 != 0 {
        let _skipped = cursor.g1()?;
        let _skipped = cursor.g4s()?;
    }
    if flags & 0x80 != 0 {
        let _skipped = cursor.g1()?;
        let _skipped = cursor.g4s()?;
    }
    if flags & 0x1000 != 0 {
        let _ = cursor.gfloat()?;
    }
    if flags & 0x2000 != 0 {
        let _ = cursor.gfloat()?;
    }
    if flags & 0x4000 != 0 {
        let _ = cursor.gfloat()?;
    }
    if flags & 0x8000 != 0 {
        let _skipped = cursor.g4s()?;
    }
    if flags & 0x40 != 0 {
        let _ = cursor.gfloat()?;
    }
    if specular {
        let _ = cursor.gfloat()?;
        let _ = cursor.gfloat()?;
        let _ = cursor.gfloat()?;
    }
    if flags & 0x10000 != 0 {
        let _ = cursor.gfloat()?;
    }
    if flags & 0x20000 != 0 {
        let _ = cursor.gfloat()?;
    }
    if flags & 0x100 != 0 {
        f.speed_u = cursor.g2s()? as f32 * 127.0 / 32767.0 / 64.0;
    }
    if flags & 0x200 != 0 {
        f.speed_v = cursor.g2s()? as f32 * 127.0 / 32767.0 / 64.0;
    }
    let repeat = cursor.g1()?;
    f.repeat_s = repeat & 0x7;
    f.repeat_t = (repeat >> 3) & 0x7;
    let _facet = facet_id(cursor.g1()?)?;
    match cursor.g1()? {
        0 => {}
        1 => f.high_detail = true,
        2 => f.low_detail = true,
        other => {
            return Err(anyhow::anyhow!(
                "material {id}: unknown quality mode {other}"
            ));
        }
    }
    f.alpha = Some(AlphaMode::decode(cursor.g1()?)?);
    if f.alpha == Some(AlphaMode::AlphaTested) {
        f.alpha_threshold = cursor.g1()?;
    }
    f.average_colour = cursor.g2()?;
    f.size = material_size(cursor.g1()?);
    f.finish(id)
}

/// Decode one material file (the first byte selects RT5 or RT7).
fn decode_material(id: u32, data: &[u8]) -> anyhow::Result<Material> {
    let mut cursor = Cursor::new(id, data);
    match cursor.g1()? {
        0 => decode_rt5(id, &mut cursor),
        1 => decode_rt7(id, &mut cursor),
        other => Err(anyhow::anyhow!(
            "material {id}: unknown version {other} (only 0 RT5 / 1 RT7 decode)"
        )),
    }
}

/// All materials from group 0 of the materials archive.
///
/// Slots are indexed by material id: `None` for file ids with no bytes.
/// Unlike the original client (which nulls undecodable entries), [`MaterialStore::load`]
/// fails on undecodable bytes so corrupt defs surface instead of vanishing.
#[derive(Clone, Debug, Default)]
pub struct MaterialStore {
    materials: Vec<Option<Material>>,
}

impl MaterialStore {
    /// Load every material (strict decode).
    pub fn load(pack: &Pack) -> anyhow::Result<Self> {
        rs910_core::profile::scope!("load materials");
        let files = pack
            .read_group(MATERIALS_ARCHIVE, MATERIALS_GROUP)
            .map_err(|error| anyhow::anyhow!("materials: group {MATERIALS_GROUP}: {error}"))?;
        let capacity = files.keys().last().map_or(0, |max| max + 1);
        let capacity = usize::try_from(capacity)
            .map_err(|_| anyhow::anyhow!("materials: group capacity overflow"))?;
        let mut materials: Vec<Option<Material>> = vec![None; capacity];
        for (&id, bytes) in &files {
            let index = usize::try_from(id)
                .map_err(|_| anyhow::anyhow!("materials: file id {id} overflow"))?;
            materials[index] = Some(decode_material(id, bytes)?);
        }
        Ok(Self { materials })
    }

    /// Slot count (`getGroupCapacity(0)` + 1 semantics: max id + 1).
    #[must_use]
    #[allow(
        clippy::len_without_is_empty,
        reason = "a slot capacity (max id + 1), not an entry count"
    )]
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    /// Material `id` (`None` when out of range or the file is absent).
    #[must_use]
    pub fn get(&self, id: u32) -> Option<&Material> {
        self.materials
            .get(usize::try_from(id).ok()?)
            .and_then(|m| m.as_ref())
    }

    /// Iterate `(id, material)` over present slots.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &Material)> {
        self.materials
            .iter()
            .enumerate()
            .filter_map(|(i, m)| m.as_ref().map(|m| (i as u32, m)))
    }
}

// ---------------------------------------------------------------------------
// HSL colour table + diffuse x aux combine.
// ---------------------------------------------------------------------------

/// Size of the HSL -> RGB colour table (65536 entries).
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "HSL colour table without a production caller yet; tested"
    )
)]
pub const COLOUR_TABLE_SIZE: usize = 65536;

/// One HSL colour table entry, computed procedurally.
///
/// Index layout is the packed HSL short itself: the outer index `0..512` is
/// `(hue6 << 3) | sat3` and the inner `0..128` is `light7`, so
/// `index = outer * 128 + inner = hue6 << 10 | sat3 << 7 | light7`. Hue in
/// degrees is `((outer>>3)/64+0.0078125)*360`, saturation is
/// `(outer&7)/8+0.0625`, lightness is `inner/128`. The hue sector is
/// `hue/60`, then come the three blends, gamma `pow(c, 0.7)`, then
/// `(int)(c*256)` truncate + `(g<<8)+(r<<16)-16777216+b`.
/// Cache archive/group: pure procedural (no pack data); the floor average
/// colours that index it come from `client.config.js5` groups 1/4
/// (`flo.rs` `FLO_ARCHIVE`).
///
/// Float fidelity: hue, saturation, lightness, the sector fraction and the
/// blends are `f32`, `pow(double, double)` runs in `f64` then narrows to `f32`
/// before the `*256` truncate. Positive-only inputs make Rust `as i32`
/// truncation identical to the original's `(int)` cast.
///
/// Sentinels: index `0` (packed black `hue/sat/light 0`) -> opaque black
/// `0xFF000000`; the overlay `-1` no-colour sentinel never indexes the table
/// (callers map it to `None`, cf. `flo.rs` `NO_COLOUR`).
#[must_use]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "HSL colour table without a production caller yet; tested"
    )
)]
pub fn colour_table_entry(index: u16) -> u32 {
    let idx = u32::from(index);
    let hue_sat = (idx >> 7) & 0x1FF;
    let light = idx & 0x7F;
    let hue_degrees = ((hue_sat >> 3) as f32 / 64.0 + 0.007_812_5) * 360.0;
    let saturation = (hue_sat & 0x7) as f32 / 8.0 + 0.0625;
    let lightness = light as f32 / 128.0;
    let sector_pos = hue_degrees / 60.0;
    let sector_floor = sector_pos as i32;
    // `sector_floor` is `0..=5` (hue `2.8..357` deg), so `% 6` stays `0..=5`.
    let sector = sector_floor % 6;
    let sector_frac = sector_pos - sector_floor as f32;
    let low = (1.0 - saturation) * lightness;
    let falling = (1.0 - saturation * sector_frac) * lightness;
    let rising = (1.0 - (1.0 - sector_frac) * saturation) * lightness;
    let (red, green, blue) = match sector {
        0 => (lightness, rising, low),
        1 => (falling, lightness, low),
        2 => (low, lightness, rising),
        3 => (low, falling, lightness),
        4 => (rising, low, lightness),
        _ => (lightness, low, falling),
    };
    // Gamma 0.7 per channel, computed in f64 and narrowed to f32.
    let red_gamma = f64::from(red).powf(0.7) as f32;
    let green_gamma = f64::from(green).powf(0.7) as f32;
    let blue_gamma = f64::from(blue).powf(0.7) as f32;
    let red_byte = (red_gamma * 256.0) as i32;
    let green_byte = (green_gamma * 256.0) as i32;
    let blue_byte = (blue_gamma * 256.0) as i32;
    // `(green_byte << 8) + (red_byte << 16) + -16777216 + blue_byte`. The
    // three bytes stay `0..=255` in practice (`lightness < 1`), so this ORs
    // opaque alpha `0xFF000000` with the bytes; `wrapping_*` keeps the
    // bit pattern identical even on the `256` edge.
    (red_byte as u32)
        .wrapping_mul(0x1_0000)
        .wrapping_add((green_byte as u32).wrapping_mul(0x100))
        .wrapping_add(0xFF00_0000)
        .wrapping_add(blue_byte as u32)
}

/// Full 64K HSL colour table, procedurally generated via
/// [`colour_table_entry`]. `Vec<u32>` of
/// `0xFF000000 | r<<16 | g<<8 | b` entries, index = packed HSL short.
#[must_use]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "HSL colour table without a production caller yet; tested"
    )
)]
pub fn build_colour_table() -> Vec<u32> {
    let mut table = Vec::with_capacity(COLOUR_TABLE_SIZE);
    for index in 0..COLOUR_TABLE_SIZE {
        // `index < 65536` always fits `u16`; the conversion is exact.
        let entry = colour_table_entry(index as u16);
        table.push(entry);
    }
    table
}

/// Decode one table entry to linear `0..1` `f32` RGB (`channel / 255.0`,
/// alpha dropped). Used for floor blend bases + tests; the atlas path keeps
/// `u8` bytes directly.
#[must_use]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "HSL colour table without a production caller yet; tested"
    )
)]
pub fn colour_table_linear(index: u16) -> [f32; 3] {
    let entry = colour_table_entry(index);
    [
        ((entry >> 16) & 0xFF) as f32 / 255.0,
        ((entry >> 8) & 0xFF) as f32 / 255.0,
        (entry & 0xFF) as f32 / 255.0,
    ]
}

/// Diffuse x aux brightness factor `(aux/255*31+1)`.
///
/// Computed as `aux * 31.0 / 255.0 + 1.0`, where `aux` is the aux red byte
/// (`>> 16 & 0xFF`). `0 -> 1.0` (no-op),
/// `255 -> 32.0` (full HDR boost). Cache: aux bytes come from
/// `client.textures.png.js5` (archive 53, `TEXTURES_PNG_ARCHIVE`) file 0 of
/// the material's aux texture id.
#[must_use]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "diffuse/aux combine without a production caller yet; tested"
    )
)]
pub fn diffuse_aux_factor(aux: u8) -> f32 {
    aux as f32 * 31.0 / 255.0 + 1.0
}

/// Exact per-texel combine as floats:
/// `rgb_out = diffuse * factor / 255`, `alpha_out = diffuse_alpha / 255`
/// (passthrough). Returns `([r,g,b], a)` in `0..32` / `0..1` (HDR, unclamped).
#[must_use]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "diffuse/aux combine without a production caller yet; tested"
    )
)]
pub fn combine_diffuse_aux_float(diffuse: [u8; 4], aux: u8) -> ([f32; 3], f32) {
    let factor = diffuse_aux_factor(aux);
    (
        [
            diffuse[0] as f32 * factor / 255.0,
            diffuse[1] as f32 * factor / 255.0,
            diffuse[2] as f32 * factor / 255.0,
        ],
        diffuse[3] as f32 / 255.0,
    )
}

/// Atlas-bake (`u8`) form of [`combine_diffuse_aux_float`]: `rgb = min(255,
/// diffuse * factor)`, alpha passthrough. The `Rgba8Unorm` atlas cannot hold
/// the `0..32` HDR floats, so values saturate at 255 (documented
/// approximation; the factor-`1.0` (aux `0`/absent) path is bit-identical).
/// Shaders stay unchanged (they sample the pre-combined bytes).
#[must_use]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "diffuse/aux combine without a production caller yet; tested"
    )
)]
pub fn combine_diffuse_aux_byte(diffuse: [u8; 4], aux: u8) -> [u8; 4] {
    let factor = diffuse_aux_factor(aux);
    let scale = |channel: u8| -> u8 {
        let scaled = channel as f32 * factor;
        if scaled >= 255.0 {
            255
        } else if scaled <= 0.0 {
            0
        } else {
            // Truncate, like an integer cast, for positive values.
            scaled as u8
        }
    };
    [
        scale(diffuse[0]),
        scale(diffuse[1]),
        scale(diffuse[2]),
        diffuse[3],
    ]
}

/// Combine a whole diffuse image with an optional aux image, texel by texel,
/// for atlas baking (so shaders keep sampling plain bytes).
///
/// The aux red channel drives [`diffuse_aux_factor`]. `None` (or empty) aux =
/// factor `1.0` everywhere (the aux plane is zero-filled), i.e. the diffuse
/// clone. Mismatched dims tile the aux (`x % aux.w`, `y % aux.h`, documented
/// grey-box rule - the original client assumes equal sizes; real census dims are squared `{64,128,256,512}` per the module
/// docs, so equal-size is the common case).
#[must_use]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "diffuse/aux combine without a production caller yet; tested"
    )
)]
pub fn combine_diffuse_aux_image(diffuse: &RgbaImage, aux: Option<&RgbaImage>) -> RgbaImage {
    let w = diffuse.w;
    let h = diffuse.h;
    let aux_image = match aux {
        Some(image) if image.w > 0 && image.h > 0 && !image.px.is_empty() => Some(image),
        _ => None,
    };
    let mut px = Vec::with_capacity(diffuse.px.len());
    // `w*h*4` bytes; short inputs yield zeroed texels, never a panic.
    for y in 0..h {
        for x in 0..w {
            let src_off = (y as usize)
                .saturating_mul(w as usize)
                .saturating_add(x as usize)
                .saturating_mul(4);
            let src = diffuse
                .px
                .get(src_off..src_off + 4)
                .unwrap_or(&[0, 0, 0, 0]);
            let diffuse_px = [src[0], src[1], src[2], src[3]];
            let aux_byte = match aux_image {
                Some(image) => {
                    let ax = (x % image.w.max(1)) as usize;
                    let ay = (y % image.h.max(1)) as usize;
                    let aux_off = ay
                        .saturating_mul(image.w as usize)
                        .saturating_add(ax)
                        .saturating_mul(4);
                    image.px.get(aux_off).copied().unwrap_or(0)
                }
                None => 0,
            };
            let combined = combine_diffuse_aux_byte(diffuse_px, aux_byte);
            px.extend_from_slice(&combined);
        }
    }
    RgbaImage { w, h, px }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
