//! Map landscape + loc decoding (full tile fidelity).
//!
//! * Landscape: a per-tile opcode read in level/x/z order. Bit `0x1` is an overlay (a `u8`
//!   shape/rotation byte plus a smart overlay id), bit `0x2` the land flags (a signed byte),
//!   bit `0x4` an underlay smart id and bit `0x8` a height byte. A tile without a height byte
//!   falls back to perlin noise on level 0 and to the tile below minus 960 above it. Perlin
//!   inputs are absolute tile coords (the mapsquare origin plus the local coord).
//! * Overlay shape/rotation: shape is the full `shape_byte >> 2` (no `& 0x1F` mask: that mask
//!   is locs-only, see below); rotation is `(region_rot + shape_byte) & 0x3`, the addition
//!   binding tighter than the mask. `region_rot` is 0 for normal loads and the region
//!   rotation for rotated region loads. Shape picks the `TILE_SHAPE_*` geometry tables and
//!   rotation spins the tile points; overlay/underlay ids look up floor definitions at
//!   `id - 1` (config territory, so the raw wire ids are exposed).
//! * Locs: framing is smart loc-id deltas, smart coord deltas packing `level/x/z`, and a `u8`
//!   info byte, with no rotation (the square origin is added straight onto the local coord).
//!   Shape is `info >> 2 & 0x1F`, angle `info & 0x3`, and a scale/rotation/translation block
//!   follows when `info & 0x80`. Rotated region locs need loc footprints (config), rotated
//!   coords and the angle `angle + rotation & 0x3`; the unit helpers
//!   [`rotate_x`]/[`rotate_z`]/[`rotate_loc_angle`] provide that path.
//! * `server/src/lostcity/engine/collision/CollisionManager.ts` is the simplified server
//!   counterpart (land opcode skips, the loc scale/rotation/translation skip table and
//!   coordinate packing).
//!
//! Level-bridging (locs on level 1 above a level-0 roof tile drop a level, see
//! `CollisionManager.ts`) is not part of landscape decoding: the tile loop stores heights at
//! the requested level verbatim. Decode therefore keeps the stored level and leaves bridging
//! to the consumer, which has the flags (`Tile.flags` bit `0x2`) needed to evaluate it.
//!
//! Grey-box scope: heights, flags and overlay/underlay ids decode exactly;
//! locs decode to raw `id + level + tile + shape/angle` (loc width/length
//! lookup is deferred, so the render agent draws unit boxes). The landscape
//! environment trailer after the tile grid is intentionally skipped. Underwater loads are
//! out of scope: Lumbridge squares load through the normal path.
//!
//! The byte helpers below (`g1b`, smart, extended smart) and the bit reader read the wire
//! encodings the packet layer uses, so the opcode loops can be audited against the format.

use thiserror::Error;

use crate::cache::{self, CacheError, Pack};
// The perlin noise lives in rs910-core.
use rs910_core::perlin::perlin;
use rs910_core::reader::{Eof, Reader as CoreReader};

/// Archive holding the map groups (id 5).
pub const MAP_ARCHIVE: &str = "mapsv2";
/// Js5 archive id of [`MAP_ARCHIVE`]. Named in load errors alongside the group id; the
/// client's JS5 owner fetches absent groups (`crate::js5net::Js5System`).
pub const MAP_ARCHIVE_ID: u32 = 5;
/// Lumbridge spawn (`server/.../Player.ts:50`): level 0, x 3222, z 3222.
#[cfg(test)] // test fixture coordinates
pub const LUMBRIDGE_SPAWN: (u8, i32, i32) = (0, 3222, 3222);
/// The 3x3 mapsquare block around Lumbridge: group `mx | mz << 7` for
/// `mx, mz in 49..=51` (mapsquare `(50, 50)` is group 6450).
pub const LUMBRIDGE_GROUPS: [u32; 9] = [6321, 6322, 6323, 6449, 6450, 6451, 6577, 6578, 6579];
/// South-west corner of the merged block: mapsquare 49 base (49 * 64).
pub const LUMBRIDGE_BASE: (i32, i32) = (3136, 3136);
/// Merged tiles per side: 3 mapsquares * 64.
#[cfg(test)] // test fixture extent
pub const LUMBRIDGE_EXTENT: usize = 192;

/// Failures map decoding can produce.
#[derive(Debug, Error)]
pub enum MapError {
    /// A read ran past the end of the buffer.
    #[error("unexpected end of map data reading {what}")]
    Truncated {
        /// What was being read (e.g. `"landscape opcode"`).
        what: &'static str,
    },
    /// The bytes are shaped wrong for the opcode loops.
    #[error("invalid map data: {0}")]
    Invalid(String),
    /// Pack IO / decompression failure.
    #[error(transparent)]
    Cache(#[from] CacheError),
}

// ---------------------------------------------------------------------------
// Byte and bit readers
// ---------------------------------------------------------------------------

/// Byte cursor for the reads the map loops use.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// One `rs910_core::reader` read at `pos` (the wire arithmetic lives there); `pos`
    /// follows it, a failure is `Truncated`.
    fn read<T>(
        &mut self,
        what: &'static str,
        read: impl FnOnce(&mut CoreReader<'a>) -> Result<T, Eof>,
    ) -> Result<T, MapError> {
        let mut r = CoreReader::at(self.data, self.pos);
        let out = read(&mut r);
        self.pos = r.pos();
        out.map_err(|_| MapError::Truncated { what })
    }

    /// One unsigned byte.
    fn g1(&mut self) -> Result<u8, MapError> {
        self.read("u8", CoreReader::g1)
    }

    /// One signed byte (the flags byte).
    fn g1b(&mut self) -> Result<i8, MapError> {
        self.read("u8", CoreReader::g1b)
    }

    /// A big-endian unsigned short.
    fn g2(&mut self) -> Result<u32, MapError> {
        self.read("u8", CoreReader::g2).map(u32::from)
    }

    /// A one-or-two-byte smart. An empty stream fails on the peek
    /// (`smart`), a short 2-byte form on its second byte (`u8`).
    fn gsmart1or2(&mut self) -> Result<i32, MapError> {
        let what = if self.pos < self.data.len() {
            "u8"
        } else {
            "smart"
        };
        self.read(what, CoreReader::gsmart1or2)
    }

    /// A sum of smarts, continuing while each one is 32767.
    fn gextended1or2(&mut self) -> Result<i32, MapError> {
        let mut total: i32 = 0;
        loop {
            let value = self.gsmart1or2()?;
            total = total.checked_add(value).ok_or_else(|| {
                MapError::Invalid("extended smart accumulator overflow".to_string())
            })?;
            if value != 32767 {
                return Ok(total);
            }
        }
    }
}

/// Bit masks for widths `0..=32`.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "bit reader; exercised by tests only")
)]
const BITMASK: [u32; 33] = [
    0x0,
    0x1,
    0x3,
    0x7,
    0xf,
    0x1f,
    0x3f,
    0x7f,
    0xff,
    0x1ff,
    0x3ff,
    0x7ff,
    0xfff,
    0x1fff,
    0x3fff,
    0x7fff,
    0xffff,
    0x1_ffff,
    0x3_ffff,
    0x7_ffff,
    0xf_ffff,
    0x1f_ffff,
    0x3f_ffff,
    0x7f_ffff,
    0xff_ffff,
    0x1ff_ffff,
    0x3ff_ffff,
    0x7ff_ffff,
    0xfff_ffff,
    0x1fff_ffff,
    0x3fff_ffff,
    0x7fff_ffff,
    0xffff_ffff,
];

/// MSB-first bit cursor. The landscape/loc loops are byte-aligned, but this is kept
/// beside the byte reader for the render agent's future packet work.
#[derive(Debug)]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "bit reader; exercised by tests only")
)]
pub struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    /// Wrap `data` with the bit cursor at byte `pos`.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "bit reader; exercised by tests only")
    )]
    pub fn new(data: &'a [u8], pos: usize) -> Result<Self, MapError> {
        let bit_pos = pos
            .checked_mul(8)
            .ok_or_else(|| MapError::Invalid("bit cursor start overflowed".to_string()))?;
        if bit_pos > data.len().saturating_mul(8) {
            return Err(MapError::Truncated { what: "bit start" });
        }
        Ok(Self { data, bit_pos })
    }

    /// Read `bits` (1..=32) MSB-first.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "bit reader; exercised by tests only")
    )]
    pub fn gbit(&mut self, bits: u32) -> Result<u32, MapError> {
        let bits = usize::try_from(bits)
            .map_err(|_| MapError::Invalid(format!("bit width {bits} does not fit memory")))?;
        if bits == 0 || bits > 32 {
            return Err(MapError::Invalid(format!(
                "bit width {bits} outside range 1..=32"
            )));
        }
        let mut need = bits;
        let mut byte = self.bit_pos >> 3;
        let mut avail = 8 - (self.bit_pos & 0x7);
        let mut out: u32 = 0;
        self.bit_pos = self
            .bit_pos
            .checked_add(need)
            .ok_or_else(|| MapError::Invalid("bit cursor advance overflowed".to_string()))?;
        while need > avail {
            let chunk = u32::from(
                *self
                    .data
                    .get(byte)
                    .ok_or(MapError::Truncated { what: "bits" })?,
            ) & BITMASK[avail];
            let shift = need - avail;
            out += chunk
                .checked_shl(shift as u32)
                .ok_or_else(|| MapError::Invalid("bit chunk shift overflowed".to_string()))?;
            byte += 1;
            need -= avail;
            avail = 8;
        }
        let tail = u32::from(
            *self
                .data
                .get(byte)
                .ok_or(MapError::Truncated { what: "bits" })?,
        );
        if need == avail {
            out += tail & BITMASK[avail];
        } else {
            out += (tail >> (avail - need)) & BITMASK[need];
        }
        Ok(out)
    }

    /// Byte position of the cursor, rounding up.
    #[must_use]
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "bit reader; exercised by tests only")
    )]
    pub fn byte_pos(&self) -> usize {
        self.bit_pos.saturating_add(7) / 8
    }
}

// ---------------------------------------------------------------------------
// Landscape
// ---------------------------------------------------------------------------

/// One decoded landscape tile.
///
/// Id semantics: the wire smart *is* the stored id, and floor definitions (colours and
/// textures, owned by the config agent) are looked up at `id - 1`. A clear opcode bit
/// decodes to `None`; a set bit decodes to `Some(wire id)`; wire `0` still means "no
/// overlay" once the floor definition is looked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    /// Render height in client units (multiples of 32; level step 960).
    pub height: i32,
    /// Land flags byte (roof/walk bits live here). Read as a signed byte and stored as the
    /// raw `u8` bit pattern, so `& 0x1 / 0x2 / 0x4` tests behave as in the server decoder.
    pub flags: u8,
    /// Overlay floor id (wire `gSmart1or2`), or `None` when opcode bit `0x1`
    /// is clear.
    pub overlay_id: Option<u32>,
    /// Overlay shape: full `shape_byte >> 2`. No `& 0x1F` mask (that mask is locs-only);
    /// the 6-bit value indexes the `TILE_SHAPE_*` geometry tables.
    pub overlay_shape: u8,
    /// Overlay rotation: `(region_rot + shape_byte) & 0x3`, the addition binding tighter
    /// than the mask. `region_rot` is 0 for normally-loaded mapsquares, so this is
    /// `shape_byte & 0x3` here; rotated region loads add the region rotation. Spins the
    /// tile points at build time.
    pub overlay_rot: u8,
    /// Underlay floor id (wire `gSmart1or2`), or `None` when opcode bit `0x4`
    /// is clear.
    pub underlay_id: Option<u32>,
}

impl Tile {
    const EMPTY: Self = Self {
        height: 0,
        flags: 0,
        overlay_id: None,
        overlay_shape: 0,
        overlay_rot: 0,
        underlay_id: None,
    };
}

/// One decoded 64x64 mapsquare landscape (file 3 of a map group).
#[derive(Clone, Debug)]
pub struct Landscape {
    /// Absolute world X of tile (0, 0).
    #[allow(dead_code, reason = "decoded landscape origin; no reader yet")]
    pub base_x: i32,
    /// Absolute world Z of tile (0, 0).
    #[allow(dead_code, reason = "decoded landscape origin; no reader yet")]
    pub base_z: i32,
    /// `tiles[level][x][z]`: one grid per level.
    pub tiles: Box<[[[Tile; 64]; 64]; 4]>,
}

impl Landscape {
    /// Decode a LAND stream. `base_x`/`base_z` are the mapsquare origin
    /// (`(group & 0x7f) << 6`, `(group >> 7) << 6`); they feed the `perlin`
    /// fallback for tiles without an explicit height byte, as absolute coords.
    ///
    /// Trailing bytes (the environment section) are ignored.
    pub fn decode(land: &[u8], base_x: i32, base_z: i32) -> Result<Self, MapError> {
        let mut reader = Reader::new(land);
        let mut tiles = Box::new([[[Tile::EMPTY; 64]; 64]; 4]);
        for level in 0..4_usize {
            for x in 0..64_usize {
                for z in 0..64_usize {
                    tiles[level][x][z] =
                        Self::decode_tile(&mut reader, &tiles, level, x, z, base_x, base_z)?;
                }
            }
        }
        Ok(Self {
            base_x,
            base_z,
            tiles,
        })
    }

    /// Look up a tile by local coords.
    #[must_use]
    #[cfg(test)] // test-only lookup
    pub fn tile(&self, level: usize, x: usize, z: usize) -> Option<&Tile> {
        self.tiles.get(level)?.get(x)?.get(z)
    }

    /// One tile body for a normally-loaded (non-underwater, unrotated) tile: bit `0x1`
    /// overlay (shape byte plus smart id), bit `0x2` flags, bit `0x4` underlay, bit `0x8`
    /// explicit height with the `1 -> 0` squash and `-h * 8 << 2` / `prev - h * 8 << 2`
    /// forms, else the perlin ground fallback on level 0 and `prev - 960` above.
    /// Out-of-bounds tiles only skip bytes; every group we merge is in-bounds, so no skip
    /// path is needed here.
    fn decode_tile(
        reader: &mut Reader<'_>,
        prev: &[[[Tile; 64]; 64]; 4],
        level: usize,
        x: usize,
        z: usize,
        base_x: i32,
        base_z: i32,
    ) -> Result<Tile, MapError> {
        let opcode = reader.g1().map_err(|_| MapError::Truncated {
            what: "landscape opcode",
        })?;
        let mut tile = Tile::EMPTY;
        if opcode & 0x1 != 0 {
            let shape_byte = reader.g1().map_err(|_| MapError::Truncated {
                what: "overlay shape",
            })?;
            let overlay = reader
                .gsmart1or2()
                .map_err(|_| MapError::Truncated { what: "overlay id" })?;
            tile.overlay_id = Some(u32::try_from(overlay).map_err(|_| {
                MapError::Invalid(format!("overlay id {overlay} does not fit u32"))
            })?);
            tile.overlay_shape = shape_byte >> 2;
            tile.overlay_rot = overlay_rotation(0, shape_byte);
        }
        if opcode & 0x2 != 0 {
            tile.flags = reader
                .g1b()
                .map_err(|_| MapError::Truncated { what: "land flags" })?
                as u8;
        }
        if opcode & 0x4 != 0 {
            let underlay = reader.gsmart1or2().map_err(|_| MapError::Truncated {
                what: "underlay id",
            })?;
            tile.underlay_id = Some(u32::try_from(underlay).map_err(|_| {
                MapError::Invalid(format!("underlay id {underlay} does not fit u32"))
            })?);
        }
        if opcode & 0x8 != 0 {
            let mut h = i32::from(reader.g1().map_err(|_| MapError::Truncated {
                what: "height delta",
            })?);
            if h == 1 {
                h = 0;
            }
            let step = h
                .checked_mul(8)
                .and_then(|v| v.checked_shl(2))
                .ok_or_else(|| MapError::Invalid("height delta overflowed".to_string()))?;
            tile.height = if level == 0 {
                step.checked_neg()
                    .ok_or_else(|| MapError::Invalid("ground height overflowed".to_string()))?
            } else {
                prev[level - 1][x][z]
                    .height
                    .checked_sub(step)
                    .ok_or_else(|| MapError::Invalid("level height overflowed".to_string()))?
            };
        } else if level == 0 {
            let lx = i32::try_from(x)
                .map_err(|_| MapError::Invalid("tile x does not fit i32".to_string()))?;
            let lz = i32::try_from(z)
                .map_err(|_| MapError::Invalid("tile z does not fit i32".to_string()))?;
            let noise = perlin(
                base_x
                    .checked_add(lx)
                    .and_then(|v| v.checked_add(932_731))
                    .ok_or_else(|| MapError::Invalid("perlin x overflowed".to_string()))?,
                base_z
                    .checked_add(lz)
                    .and_then(|v| v.checked_add(556_238))
                    .ok_or_else(|| MapError::Invalid("perlin z overflowed".to_string()))?,
            );
            tile.height = noise
                .checked_neg()
                .and_then(|v| v.checked_mul(8))
                .and_then(|v| v.checked_shl(2))
                .ok_or_else(|| MapError::Invalid("perlin height overflowed".to_string()))?;
        } else {
            tile.height = prev[level - 1][x][z]
                .height
                .checked_sub(960)
                .ok_or_else(|| MapError::Invalid("level height fallback overflowed".to_string()))?;
        }
        Ok(tile)
    }
}

/// Overlay rotation for one tile: `(region_rot + shape_byte) & 0x3`, the addition binding
/// tighter than the mask. Normal landscape loads pass `region_rot = 0`; rotated region
/// loads pass the region rotation.
/// `wrapping_add` is exact here: only the low two bits survive the mask.
#[must_use]
pub fn overlay_rotation(region_rot: u8, shape_byte: u8) -> u8 {
    region_rot.wrapping_add(shape_byte) & 0x3
}

/// Spin a tile-local `0..=7` x coord. Used with [`rotate_z`] by rotated region loads;
/// normal loads pass rotation 0 (identity).
#[must_use]
pub fn rotate_x(x: i32, z: i32, rotation: i32) -> i32 {
    match rotation & 0x3 {
        0 => x,
        1 => z,
        2 => 7 - x,
        _ => 7 - z,
    }
}

/// Spin a tile-local `0..=7` z coord.
/// See [`rotate_x`] for the call sites.
#[must_use]
pub fn rotate_z(x: i32, z: i32, rotation: i32) -> i32 {
    match rotation & 0x3 {
        0 => z,
        1 => 7 - x,
        2 => 7 - z,
        _ => x,
    }
}

/// Loc angle adjustment for rotated region loads: `angle + rotation & 0x3`
/// (the same shape as the overlay rotation). Normal loads pass rotation 0, leaving the
/// wire angle untouched.
#[must_use]
pub fn rotate_loc_angle(angle: u8, rotation: u8) -> u8 {
    angle.wrapping_add(rotation) & 0x3
}

// ---------------------------------------------------------------------------
// Locs
// ---------------------------------------------------------------------------

/// One decoded static loc: raw config id plus its tile footprint. Width,
/// length and collision live in the loc config (deferred — do NOT add a
/// footprint here, the config agent owns it); the render agent draws unit
/// boxes from `x`/`z`. `level` is the stored level: bridge adjustment
/// (see `CollisionManager.ts`) is the consumer's job.
///
/// `shape`/`angle` follow the unrotated decode: shape `info >> 2 & 0x1F`, angle
/// `info & 0x3` — the `0x1F` mask lives here, not on tiles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocSpawn {
    /// Loc config id.
    pub id: u32,
    /// Level (as stored; bridge adjustment deferred).
    pub level: u8,
    /// Absolute world X.
    pub x: i32,
    /// Absolute world Z.
    pub z: i32,
    /// Object shape (`info >> 2 & 0x1F`).
    pub shape: u8,
    /// Object angle (`info & 0x3`).
    pub angle: u8,
    /// The optional scale/rotation/translation block carried when `info & 0x80`.
    pub srt: Option<LocSrt>,
}

/// A loc's scale/rotation/translation block: quaternion, translation, scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocSrt {
    /// `rot` (x, y, z, w), each `g2s / 32768`.
    pub rot: [f32; 4],
    /// `trans`, raw `g2s` values.
    pub trans: [f32; 3],
    /// `scale`, each `g2s / 128` (uniform when flag `0x10`).
    pub scale: [f32; 3],
}

/// Decode a LOC stream (file 0) for normally-loaded mapsquares, following the server's
/// `decodeLocs` in `CollisionManager.ts`.
///
/// No rotation is applied: the normal path adds the square origin straight
/// onto the local coord and keeps the wire angle. Rotated region placement
/// composes [`rotate_x`]/[`rotate_z`] with the loc footprint plus
/// [`rotate_loc_angle`]; that path needs config-owned width/length, so it stays
/// out of decode — use the helpers at the call site when the config agent
/// lands footprints.
pub fn decode_locs(loc: &[u8], base_x: i32, base_z: i32) -> Result<Vec<LocSpawn>, MapError> {
    let mut reader = Reader::new(loc);
    let mut spawns = Vec::new();
    let mut loc_id: i32 = -1;
    loop {
        let id_delta = reader.gextended1or2().map_err(|_| MapError::Truncated {
            what: "loc id delta",
        })?;
        if id_delta == 0 {
            return Ok(spawns);
        }
        loc_id = loc_id
            .checked_add(id_delta)
            .ok_or_else(|| MapError::Invalid("loc id accumulator overflowed".to_string()))?;
        let id = u32::try_from(loc_id)
            .map_err(|_| MapError::Invalid(format!("negative loc id {loc_id}")))?;

        let mut coord: i32 = 0;
        loop {
            let coord_delta = reader.gsmart1or2().map_err(|_| MapError::Truncated {
                what: "loc coord delta",
            })?;
            if coord_delta == 0 {
                break;
            }
            coord = coord
                .checked_add(
                    coord_delta.checked_sub(1).ok_or_else(|| {
                        MapError::Invalid("loc coord delta underflowed".to_string())
                    })?,
                )
                .ok_or_else(|| MapError::Invalid("loc coord accumulator overflowed".to_string()))?;
            if !(0..=0x3FFF).contains(&coord) {
                return Err(MapError::Invalid(format!(
                    "loc coord {coord} outside 14-bit range"
                )));
            }
            // `unpackCoord`: z = packed & 0x3f, x = packed >> 6 & 0x3f,
            // level = packed >> 12 & 0x3.
            let z = coord & 0x3F;
            let x = (coord >> 6) & 0x3F;
            let level = (coord >> 12) & 0x3;

            let info = reader
                .g1()
                .map_err(|_| MapError::Truncated { what: "loc info" })?;
            let srt = if info & 0x80 != 0 {
                Some(read_scale_rot_trans(&mut reader)?)
            } else {
                None
            };

            let level_u8 = u8::try_from(level)
                .map_err(|_| MapError::Invalid(format!("bad loc level {level}")))?;
            let x_abs = base_x
                .checked_add(x)
                .ok_or_else(|| MapError::Invalid("loc absolute x overflowed".to_string()))?;
            let z_abs = base_z
                .checked_add(z)
                .ok_or_else(|| MapError::Invalid("loc absolute z overflowed".to_string()))?;
            spawns.push(LocSpawn {
                id,
                level: level_u8,
                x: x_abs,
                z: z_abs,
                shape: (info >> 2) & 0x1F,
                // Rotation 0: the normal path keeps the wire angle; region loads pass
                // the region rotation.
                angle: rotate_loc_angle(info & 0x3, 0),
                srt,
            });
        }
    }
}

/// Decode the scale/rotation/translation payload after a loc `info` byte with `0x80` set.
fn read_scale_rot_trans(reader: &mut Reader<'_>) -> Result<LocSrt, MapError> {
    let flags = reader.g1().map_err(|_| MapError::Truncated {
        what: "loc scale-rot-trans flags",
    })?;
    let mut g2s = |what: &'static str| -> Result<f32, MapError> {
        let raw = reader.g2().map_err(|_| MapError::Truncated { what })? as i32;
        Ok((if raw > 32767 { raw - 65536 } else { raw }) as f32)
    };
    let mut rot = [0.0_f32, 0.0, 0.0, 1.0];
    if flags & 0x1 != 0 {
        for r in rot.iter_mut() {
            *r = g2s("loc rotation")? / 32768.0;
        }
    }
    let mut trans = [0.0_f32; 3];
    if flags & 0x2 != 0 {
        trans[0] = g2s("loc translation x")?;
    }
    if flags & 0x4 != 0 {
        trans[1] = g2s("loc translation y")?;
    }
    if flags & 0x8 != 0 {
        trans[2] = g2s("loc translation z")?;
    }
    let mut scale = [1.0_f32; 3];
    if flags & 0x10 == 0 {
        if flags & 0x20 != 0 {
            scale[0] = g2s("loc scale x")? / 128.0;
        }
        if flags & 0x40 != 0 {
            scale[1] = g2s("loc scale y")? / 128.0;
        }
        if flags & 0x80 != 0 {
            scale[2] = g2s("loc scale z")? / 128.0;
        }
    } else {
        let s = g2s("loc uniform scale")? / 128.0;
        scale = [s, s, s];
    }
    Ok(LocSrt { rot, trans, scale })
}

// ---------------------------------------------------------------------------
// Merged world
// ---------------------------------------------------------------------------

/// Absolute origin of a map group (`CollisionManager.ts:34-35`).
#[must_use]
pub fn group_base(group: u32) -> (i32, i32) {
    (((group & 0x7F) << 6) as i32, ((group >> 7) << 6) as i32)
}

/// Group ids for the mapsquare block around center `(cx, cz)` plus Chebyshev
/// `radius` in mapsquares: `group = mx | mz << 7` for
/// `mx in cx - radius..=cx + radius` (same for `mz`), ascending, so
/// `region_groups(50, 50, 1)` is exactly [`LUMBRIDGE_GROUPS`].
///
/// This is the inverse of [`group_base`] (`mx = group & 0x7F`,
/// `mz = group >> 7`; `CollisionManager.ts:34-35` threads the same split).
/// Errors when the block leaves the `0..128` mapsquare range — group math is
/// 14-bit (`mx | mz << 7 < 16384`), so out-of-range input is rejected instead
/// of wrapping into a wrong region.
///
/// Key threading: none needed here. Groups are read through
/// `Pack::read_group`, which owns container decrypt (`cache.rs`, `Js5.ts:514-521`),
/// so [`load_groups`]/[`load_region`] below stream locked regions unchanged
/// once `<pack_root>/keys.json` is present.
#[cfg(test)] // test-only helper
pub fn region_groups(cx: u32, cz: u32, radius: u32) -> Result<Vec<u16>, MapError> {
    let lo_x = cx.checked_sub(radius).ok_or_else(|| {
        MapError::Invalid(format!(
            "region center {cx} minus radius {radius} underflowed"
        ))
    })?;
    let hi_x = cx.checked_add(radius).ok_or_else(|| {
        MapError::Invalid(format!(
            "region center {cx} plus radius {radius} overflowed"
        ))
    })?;
    let lo_z = cz.checked_sub(radius).ok_or_else(|| {
        MapError::Invalid(format!(
            "region center {cz} minus radius {radius} underflowed"
        ))
    })?;
    let hi_z = cz.checked_add(radius).ok_or_else(|| {
        MapError::Invalid(format!(
            "region center {cz} plus radius {radius} overflowed"
        ))
    })?;
    for (edge, name) in [(hi_x, "x"), (hi_z, "z")] {
        if edge > 127 {
            return Err(MapError::Invalid(format!(
                "region mapsquare {name} {edge} outside 0..128 (group math is 14-bit)"
            )));
        }
    }
    let side = hi_x
        .checked_sub(lo_x)
        .and_then(|dx| dx.checked_add(1))
        .and_then(|w| {
            hi_z.checked_sub(lo_z)
                .and_then(|dz| dz.checked_add(1))
                .and_then(|h| w.checked_mul(h))
        })
        .ok_or_else(|| MapError::Invalid("region block size overflowed".to_string()))?;
    let mut groups = Vec::with_capacity(side as usize);
    for mz in lo_z..=hi_z {
        for mx in lo_x..=hi_x {
            let group = mx | (mz << 7);
            groups.push(u16::try_from(group).map_err(|_| {
                MapError::Invalid(format!("region group {group} does not fit u16"))
            })?);
        }
    }
    Ok(groups)
}

/// Any set of map groups merged to one absolute tile grid.
#[derive(Clone, Debug)]
pub struct World {
    /// Absolute world X of the south-west corner (bounding-box minimum).
    pub base_x: i32,
    /// Absolute world Z of the south-west corner (bounding-box minimum).
    pub base_z: i32,
    /// Tiles per side: the square covering the groups' bounding box (192 for
    /// the 3x3 Lumbridge block, 64 for a single group). Non-square inputs are
    /// padded to the larger span; tiles outside every group stay
    /// [`Tile::EMPTY`].
    pub extent: usize,
    /// `tiles[(level * extent + lx) * extent + lz]`.
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub tiles: Vec<Tile>,
    /// Every static loc in the block, in absolute coords.
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub locs: Vec<LocSpawn>,
    /// Groups merged into this world, sorted ascending and deduplicated.
    pub groups: Vec<u16>,
}

impl World {
    /// Look up a tile by absolute coords.
    #[must_use]
    #[cfg(any(test, feature = "test-hooks"))] // test-only lookup
    pub fn tile(&self, level: u8, x: i32, z: i32) -> Option<&Tile> {
        if level >= 4 {
            return None;
        }
        let lx = x.checked_sub(self.base_x)?;
        let lz = z.checked_sub(self.base_z)?;
        if lx < 0 || lz < 0 {
            return None;
        }
        let (lx, lz) = (lx as usize, lz as usize);
        if lx >= self.extent || lz >= self.extent {
            return None;
        }
        self.tiles
            .get((usize::from(level) * self.extent + lx) * self.extent + lz)
    }
}

/// Scene level for a loc after bridge resolution.
///
/// Ground truth: bridging is a per-column property driven by the level-1
/// tile flags (bit `0x2` set shifts the whole column down one level).
/// The server TS mirrors it for locs as
/// `bridged = (level === 1 ? lands[coord] & 0x2
/// : lands[packCoord(x, z, 1)] & 0x2) === 2;
/// actual = bridged ? level - 1 : level`
/// (`CollisionManager.ts:128` land collision, `:185-189` locs) — i.e. the
/// flag is ALWAYS read at level 1 (for wire level 1 that is the tile itself,
/// for every other wire level it is the level-1 tile of the same column,
/// the HIGHER tile for level 0 and the reference plane for 2-3).
///
/// Rule implemented here (decode keeps the wire level intact; consumers call
/// this): `level == 0 -> 0` (a bridged level-0 loc would drop to -1 in TS and
/// is skipped there; the renderer clamps it to 0 instead of dropping);
/// `level > 0` with `world.tile(1, x, z).flags & 0x2 != 0 -> level - 1`,
/// else the wire level. Missing tiles (`None`, outside the merged block)
/// keep the wire level. Never panics; `saturating_sub` guards level 0.
#[must_use]
#[cfg(test)] // test-only helper
pub fn resolved_level(world: &World, loc: &LocSpawn) -> u8 {
    if loc.level == 0 {
        return 0;
    }
    match world.tile(1, loc.x, loc.z) {
        Some(tile) if tile.flags & 0x2 != 0 => loc.level.saturating_sub(1),
        _ => loc.level,
    }
}

/// Load and merge any set of map groups from the pack into one [`World`].
///
/// Each group contributes its 64x64 mapsquare at [`group_base`]; the world
/// spans the bounding box of all groups. A group that is absent on disk (or
/// otherwise unreadable) is a hard error naming archive [`MAP_ARCHIVE_ID`]
/// and the group id — the caller waits until the groups are ready
/// (`crate::js5net::Js5System::map_groups_missing`) before loading. A group
/// without a landscape stream ([`cache::LAND_FILE`]: a mapsquare the map
/// leaves empty, such as the one east of the Abyss) contributes empty tiles,
/// as the client's own loader skips land data it does not have; a missing loc
/// stream contributes no locs.
pub fn load_groups(pack: &Pack, groups: &[u16]) -> Result<World, MapError> {
    if groups.is_empty() {
        return Err(MapError::Invalid(
            "load_groups: no groups requested".to_string(),
        ));
    }
    let mut sorted: Vec<u16> = groups.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut min_x = i32::MAX;
    let mut min_z = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_z = i32::MIN;
    for group in &sorted {
        let (gx, gz) = group_base(u32::from(*group));
        min_x = min_x.min(gx);
        min_z = min_z.min(gz);
        max_x = max_x.max(
            gx.checked_add(64)
                .ok_or_else(|| MapError::Invalid(format!("group {group} east edge overflowed")))?,
        );
        max_z = max_z
            .max(gz.checked_add(64).ok_or_else(|| {
                MapError::Invalid(format!("group {group} north edge overflowed"))
            })?);
    }
    let span_x = max_x
        .checked_sub(min_x)
        .ok_or_else(|| MapError::Invalid("group bounding box underflowed".to_string()))?;
    let span_z = max_z
        .checked_sub(min_z)
        .ok_or_else(|| MapError::Invalid("group bounding box underflowed".to_string()))?;
    let extent = usize::try_from(span_x.max(span_z)).map_err(|_| {
        MapError::Invalid(format!("world span {span_x}x{span_z} does not fit memory"))
    })?;
    let tile_count = 4_usize
        .checked_mul(extent)
        .and_then(|v| v.checked_mul(extent))
        .ok_or_else(|| MapError::Invalid(format!("world extent {extent} overflowed")))?;
    let mut tiles = vec![Tile::EMPTY; tile_count];
    let mut locs = Vec::new();

    for group in &sorted {
        let gid = u32::from(*group);
        let files = pack.read_group(MAP_ARCHIVE, gid).map_err(|err| {
            MapError::Invalid(format!(
                "archive {} ({MAP_ARCHIVE}) group {gid} unavailable: {err:#}; \
                 wait for mapsJs5.isGroupReady (the JS5 client) then retry",
                MAP_ARCHIVE_ID,
            ))
        })?;
        let (land, loc) = cache::map_group(&files);
        let (gx, gz) = group_base(gid);
        if let Some(loc) = loc {
            locs.extend(decode_locs(loc, gx, gz)?);
        }
        let Some(land) = land else {
            continue;
        };
        let square = Landscape::decode(land, gx, gz)?;
        for level in 0..4_usize {
            for x in 0..64_usize {
                for z in 0..64_usize {
                    let wx = gx + x as i32;
                    let wz = gz + z as i32;
                    let lx = wx - min_x;
                    let lz = wz - min_z;
                    if lx < 0 || lz < 0 {
                        continue;
                    }
                    let (lx, lz) = (lx as usize, lz as usize);
                    if lx >= extent || lz >= extent {
                        continue;
                    }
                    if let Some(slot) = tiles.get_mut((level * extent + lx) * extent + lz) {
                        *slot = square.tiles[level][x][z];
                    }
                }
            }
        }
    }

    Ok(World {
        base_x: min_x,
        base_z: min_z,
        extent,
        tiles,
        locs,
        groups: sorted,
    })
}

/// Load the 3x3 Lumbridge block from the pack into one merged [`World`].
/// Delegates to [`load_groups`]; every id fits `u16` (all are `< 2^14`).
pub fn load_lumbridge(pack: &Pack) -> Result<World, MapError> {
    let groups: Vec<u16> = LUMBRIDGE_GROUPS.iter().map(|group| *group as u16).collect();
    load_groups(pack, &groups)
}

#[cfg(test)]
mod tests;
