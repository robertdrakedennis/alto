//! Map file 5 (named `NXT_LOC` in the 910 cache index): despite that name, the NXT
//! **terrain** of one mapsquare, not locs.
//!
//! Evidence:
//! - 865 reads file 5 only in the terrain builder: `BuildTerrainMain::
//!   RunFirst` (L1029487, L1029847) prescans it for the underlay and
//!   overlay ids to load (floor overlay type lookups from L1030038), and
//!   `BuildTerrainWorker::DecodeTerrain` (L952027-952361) decodes it into
//!   `DecodedTerrainTile`s (overlay shapes resolve to
//!   `TerrainOverlayShape::*`).
//! - The layout below consumes every file of the 910 pack exactly (4,414
//!   non-empty files; the other 2,817 squares have a zero-length file 5).
//! - Against the 910 LAND file (3) of the same square, on the inner 64x64
//!   tiles, the per-tile settings flags are equal everywhere, overlays and
//!   shapes agree where both have one, and every 910-only overlay is the
//!   water overlay that NXT moves into the tile's water layer
//!   (`nxt::tests::map_squares_decode_and_match_the_910_land_files` checks
//!   every tile).
//!
//! Structure: level blocks until the end of the file, each `u8 level` then
//! 66x66 tiles (the 64x64 square plus a one-tile border), X outer, Z inner
//! (the border offset `(1, 1)` and X-outer order are the alignment that
//! matches 910 LAND).

use super::Cur;

/// Tiles per side of a level block (64 plus a one-tile border).
pub const TERRAIN_SIDE: usize = 66;

/// One terrain tile. A tile whose flag byte is 0 carries only a height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NxtTerrainTile {
    /// The raw flag byte. 0: height only. Otherwise bit `0x1` is set, bits
    /// 1-3 and 5-6 are the 910 tile settings (see
    /// [`NxtTerrainTile::settings`]) and bit `0x10` marks a water tile.
    pub flags: u8,
    /// Height code (865's `DecodeHeightCode`, L186137: `32 c` fine units
    /// over the level below, `1` none, `0` 960): the 910 LAND byte (the
    /// level-0 noise stored explicitly). At a water tile, the depth of the
    /// bed below the surface in the same steps: the 910 `UNDERWATER_LAND`
    /// (file 4) byte. Proven over the pack
    /// (`map_squares_decode_and_match_the_910_land_files`).
    pub height: u8,
    /// Bit `0x10`: the water surface's height code (865 `+3`), the 910
    /// LAND height of the tile.
    pub water_height: Option<u8>,
    /// Underlay id (floor underlay type), smart `- 1`; at a water tile the
    /// bed's (file 4's).
    pub underlay: Option<u16>,
    /// The `u16` after an underlay (865 `+8`, `AssignUnderlay` L500790):
    /// the tile's colour, HSL16, the 910 client's 10 x 10 floor-colour blend
    /// (the ground builder) over file 5's underlay ids, precomputed
    /// (865 skips the underlay type's own colour).
    pub underlay_u16: Option<u16>,
    /// Overlay id (floor overlay type), smart `- 1`; at a water tile the
    /// bed's (file 4's).
    pub overlay: Option<u16>,
    /// Bit `0x10`: the water overlay id (865 `+24`, prescanned with the
    /// overlays): the 910 LAND overlay (111 in most squares).
    pub water_overlay: Option<u16>,
    /// The shape byte of an overlay: `shape << 2 | rotation`
    /// (`TerrainOverlayShape`, 865 `+32`/`+16`).
    pub overlay_shape: Option<u8>,
    /// Bit `0x10` with an overlay: the 910 LAND underlay (865 `+20`,
    /// prescanned with the underlays); 865 draws it as the overlay part of
    /// a shaped water tile (`BuildTerrain` L861300-861650).
    pub water_underlay: Option<u16>,
}

impl NxtTerrainTile {
    /// The 910 LAND settings flags (`MapLoader` tile flags), as 865 packs
    /// them: `(flags >> 2) & 0x18 | (flags >> 1) & 7` (L952037-952038).
    /// Equal to the 910 LAND value on every sampled tile.
    #[must_use]
    pub fn settings(&self) -> u8 {
        ((self.flags >> 2) & 0x18) | ((self.flags >> 1) & 7)
    }

    /// Bit `0x10`.
    #[must_use]
    pub fn is_water(&self) -> bool {
        self.flags & 0x10 != 0
    }
}

/// One level block: 66x66 tiles, index `x * 66 + z`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NxtTerrainLevel {
    /// Level byte (0..=3 in the pack).
    pub level: u8,
    /// `TERRAIN_SIDE * TERRAIN_SIDE` tiles, X outer.
    pub tiles: Vec<NxtTerrainTile>,
}

impl NxtTerrainLevel {
    /// Tile at border-inclusive `(x, z)`, each in `0..66`.
    #[must_use]
    pub fn tile(&self, x: usize, z: usize) -> Option<&NxtTerrainTile> {
        if x < TERRAIN_SIDE && z < TERRAIN_SIDE {
            self.tiles.get(x * TERRAIN_SIDE + z)
        } else {
            None
        }
    }
}

fn id(raw: i32, field: &str, what: &str) -> anyhow::Result<Option<u16>> {
    if raw == -1 {
        return Ok(None);
    }
    u16::try_from(raw)
        .map(Some)
        .map_err(|_| anyhow::anyhow!("{what}: {field} {raw} is out of range"))
}

/// Decode one file-5 payload (an empty file gives no levels).
pub fn decode_terrain(group: u32, data: &[u8]) -> anyhow::Result<Vec<NxtTerrainLevel>> {
    let what = format!("map {group} file 5 (terrain)");
    let mut c = Cur::new(data, &what);
    let mut levels = Vec::new();
    while c.remaining() > 0 {
        let level = c.g1("level")?;
        let mut tiles = Vec::with_capacity(TERRAIN_SIDE * TERRAIN_SIDE);
        for _ in 0..TERRAIN_SIDE * TERRAIN_SIDE {
            // DecodeTerrain L952028-952361.
            let flags = c.g1("tile flags")?;
            let height = c.g1("height")?;
            let mut tile = NxtTerrainTile {
                flags,
                height,
                ..NxtTerrainTile::default()
            };
            if flags != 0 {
                let water = flags & 0x10 != 0;
                if water {
                    tile.water_height = Some(c.g1("water height")?);
                }
                tile.underlay = id(c.gsmart_null("underlay")?, "underlay", &what)?;
                if tile.underlay.is_some() {
                    tile.underlay_u16 = Some(c.g2("underlay u16")?);
                }
                tile.overlay = id(c.gsmart_null("overlay")?, "overlay", &what)?;
                if water {
                    tile.water_overlay =
                        id(c.gsmart_null("water overlay")?, "water overlay", &what)?;
                }
                if tile.overlay.is_some() {
                    tile.overlay_shape = Some(c.g1("overlay shape")?);
                    if water {
                        tile.water_underlay =
                            id(c.gsmart_null("water underlay")?, "water underlay", &what)?;
                    }
                }
            }
            tiles.push(tile);
        }
        levels.push(NxtTerrainLevel { level, tiles });
    }
    c.finish()?;
    Ok(levels)
}
