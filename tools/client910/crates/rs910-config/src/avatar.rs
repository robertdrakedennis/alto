//! Player-appearance config data the interface model owner reads: identity
//! kits (IDK), the default male kit and the graphics-defaults kit palette
//! tables.
//!
//! # IDK storage
//!
//! Identity kits are group 3 of the `config` archive ([`IDK_BAS_ARCHIVE`]),
//! one file per kit, so the config id is the file id. A kit's models and
//! head models load from the `models` archive, group = model id, file 0.
//!
//! Per-kit recolours and retextures apply per part at bind time; the kit
//! palette ([`AvatarPalette`] indices into the [`GraphicsDefaults`] tables)
//! applies after the parts are merged.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use crate::cache::Pack;
use crate::opcode_table::{
    at, decode_record, payload, span, Entry, Field, Input, Record, Rule, Slot, Table, Unknown,
};

/// Pack archive holding the IDK (group 3) and BAS (group 32) subgroups.
pub const IDK_BAS_ARCHIVE: &str = "config";
/// Group of identity-kit entries inside [`IDK_BAS_ARCHIVE`].
pub const IDK_GROUP: u32 = 3;

/// Sentinel for "no IDK in this slot". The wire uses kit id 0 for an empty
/// slot, but our slots hold RAW idk ids where 0 is a real kit (male hair).
/// `u32::MAX` can never be a real idk id (ids arrive as 31-bit smarts), so a
/// lookup always misses and resolution skips the slot.
pub const NO_IDK: u32 = u32::MAX;

/// Default player BAS id, from the dev-server appearance block.
pub const DEFAULT_BAS: i32 = 2699;

// ---------------------------------------------------------------------------
// Idk: identity kit part
// ---------------------------------------------------------------------------

/// One identity-kit part: a selectable head/hair/beard/body/arms/legs/gloves/
/// boots variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Idk {
    /// Config id (the file id in group 3 of the `config` archive).
    pub id: u32,
    /// Body-part byte. The stock client reads and discards it, because parts
    /// are addressed by id at run time; it is kept so the render track can
    /// tag parts. Male parts are `{0..6, 14}`, female `{7..13, 15}`. Verified
    /// against the pack: 0 hair, 1 beard, 2 body, 3 arms, 4 gloves, 5 legs,
    /// 6 boots (male); 7 hair, 9 body, 10 arms, 11 gloves, 12 legs, 13 boots
    /// (female, no beard part 8); 14/15 head are unused in the data.
    pub bodypart: u8,
    /// Body model ids without the unset (`-1`) entries. Each loads from the
    /// `models` archive, group = id, file 0.
    pub models: Vec<u32>,
    /// Recolour sources.
    pub recol_s: Vec<u16>,
    /// Recolour destinations, parallel to `recol_s`.
    pub recol_d: Vec<u16>,
    /// Retexture sources.
    pub retex_s: Vec<u16>,
    /// Retexture destinations, parallel to `retex_s`.
    pub retex_d: Vec<u16>,
    /// Head-model ids by slot; `-1` when absent. Non-negative entries load
    /// from the `models` archive and merge across all kit slots for the head
    /// mesh, see [`head_part_models`].
    pub heads: [i32; 5],
}

/// Idk opcodes of this revision. The stream may address head slots 5 to 9;
/// those have no field but must still be consumed so later opcodes stay
/// aligned.
static IDK_OPCODES: Table<Idk, anyhow::Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Byte(|k, _, v| k.bodypart = v)),
        Entry::new(at(2), Rule::Custom(read_models)),
        // Accepted and ignored, without payload.
        Entry::new(at(3), Rule::Skip(&[])),
        Entry::new(
            at(40),
            Rule::Custom(|s, k, _| {
                (k.recol_s, k.recol_d) = payload::pairs(s)?;
                Ok(())
            }),
        ),
        Entry::new(
            at(41),
            Rule::Custom(|s, k, _| {
                (k.retex_s, k.retex_d) = payload::pairs(s)?;
                Ok(())
            }),
        ),
        // Palette index tables, dropped: the render track only needs the
        // source and destination tables.
        Entry::new(span(44, 45), Rule::Skip(&[Field::Short])),
        Entry::new(span(60, 69), Rule::Custom(read_head)),
    ],
    Unknown::Reject,
);

fn read_models(source: Input<anyhow::Error>, idk: &mut Idk, _: Slot) -> Result<()> {
    let count = source.byte()?;
    for _ in 0..count {
        let model = source.smart_id()?;
        if model >= 0 {
            idk.models.push(model as u32);
        }
    }
    Ok(())
}

fn read_head(source: Input<anyhow::Error>, idk: &mut Idk, slot: Slot) -> Result<()> {
    let head = source.smart_id()?;
    if let Some(entry) = idk.heads.get_mut(slot) {
        *entry = head;
    }
    Ok(())
}

/// Decode one IDK entry. An opcode outside the table is an error naming the id
/// and the opcode.
pub fn decode_idk(id: u32, data: &[u8]) -> Result<Idk> {
    let blank = Idk {
        id,
        bodypart: 0,
        models: Vec::new(),
        recol_s: Vec::new(),
        recol_d: Vec::new(),
        retex_s: Vec::new(),
        retex_d: Vec::new(),
        heads: [-1, -1, -1, -1, -1],
    };
    let record = Record {
        kind: "idk",
        id: i64::from(id),
    };
    decode_record(&IDK_OPCODES, "avatar", record, data, blank)
}

/// All identity-kit configs, keyed by id (the file id in group 3 of `config`).
#[derive(Clone, Debug, Default)]
pub struct IdkStore {
    entries: BTreeMap<u32, Idk>,
}

impl IdkStore {
    /// Load group 3 of the `config` archive and decode each file as its own id.
    pub fn load(pack: &Pack) -> Result<Self> {
        let mut entries = BTreeMap::new();
        let index = pack.read_archive_index(IDK_BAS_ARCHIVE)?;
        if !index.group_id.contains(&IDK_GROUP) {
            anyhow::bail!("{IDK_BAS_ARCHIVE}: missing IDK group {IDK_GROUP}");
        }
        let files = pack
            .read_group(IDK_BAS_ARCHIVE, IDK_GROUP)
            .with_context(|| format!("{IDK_BAS_ARCHIVE} group {IDK_GROUP}"))?;
        for (file_id, bytes) in &files {
            let entry = decode_idk(*file_id, bytes).with_context(|| format!("idk {file_id}"))?;
            entries.insert(*file_id, entry);
        }
        Ok(Self { entries })
    }

    /// Look up one IDK by id.
    pub fn get(&self, id: u32) -> Option<&Idk> {
        self.entries.get(&id)
    }
}

// ---------------------------------------------------------------------------
// AvatarKit: one player's appearance selection
// ---------------------------------------------------------------------------

/// One player's appearance selection: kit IDK ids per slot plus kit colours
/// and the BAS id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvatarKit {
    /// Head-slot IDK id. The default appearance carries no head kit (the
    /// server writes an empty slot where the eighth kit would go), so this
    /// defaults to [`NO_IDK`], which resolution skips. Head geometry for the
    /// default avatar comes from the hair and beard kits' `heads` entries via
    /// [`head_models`]. The eighth chooser slot is the head slot.
    pub head: u32,
    /// Hair IDK id (server default 0).
    pub hair: u32,
    /// Beard IDK id (server default 10).
    pub beard: u32,
    /// Body IDK id (server default 18).
    pub body: u32,
    /// Arms IDK id (server default 26).
    pub arms: u32,
    /// Legs IDK id (server default 36).
    pub legs: u32,
    /// Boots IDK id (server default 42).
    pub boots: u32,
    /// Gloves IDK id (server default 33).
    pub gloves: u32,
    /// Kit colour indices, applied through the graphics-defaults tables after
    /// the merge. Ten entries, not five: the client reads ten recolour bytes
    /// and the server writes ten.
    pub recolours: [u8; 10],
    /// Kit texture indices; ten entries, like the recolours.
    pub retextures: [u8; 10],
    /// BAS id (server default 2699).
    pub bas: i32,
}

impl AvatarKit {
    /// The server's default (male, bearded) appearance: body 18, arms 26,
    /// legs 36, hair 0, gloves 33, boots 42, beard 10, bas 2699.
    ///
    /// Those block values arrive `| 0x100` on the wire; the client strips the
    /// flag and subtracts the 256 bias (a value of 2048 or more means an item
    /// kit, else `value - 256` is the IDK id), so e.g. `18 | 0x100 = 274` is
    /// already IDK id 18 and no further mapping is needed.
    pub fn default_male() -> Self {
        Self {
            head: NO_IDK,
            hair: 0,
            beard: 10,
            body: 18,
            arms: 26,
            legs: 36,
            boots: 42,
            gloves: 33,
            recolours: [0; 10],
            retextures: [0; 10],
            bas: DEFAULT_BAS,
        }
    }

    /// `(slot name, idk id)` pairs in render-assembly order. The names pin the
    /// [`kit_models`] tuple stream to slots for the mesh track.
    pub fn slots(&self) -> [(&'static str, u32); 8] {
        [
            ("head", self.head),
            ("hair", self.hair),
            ("beard", self.beard),
            ("body", self.body),
            ("arms", self.arms),
            ("legs", self.legs),
            ("boots", self.boots),
            ("gloves", self.gloves),
        ]
    }
}

// ---------------------------------------------------------------------------
// GraphicsDefaults: kit palette tables
// ---------------------------------------------------------------------------

/// Pack archive holding the graphics-defaults file.
pub const DEFAULTS_ARCHIVE: &str = "defaults";
/// File id of the graphics defaults inside [`DEFAULTS_ARCHIVE`].
pub const GRAPHICS_DEFAULTS_GROUP: u32 = 3;

/// Kit palette tables: the 10 x 4 source grids plus the variable-length
/// destination lists the kit colour indices select from.
///
/// The recolour and retexture tables decode from the graphics defaults. Each
/// of the 10 kit slots (the client reads 10 recolour and 10 retexture bytes
/// per player) owns 4 source entries; the kit index picks one destination per
/// entry, on the body and the head node alike:
///
/// ```text
/// for slot in 0..10 { for e in 0..4 {
///     if kit.recolours[slot] < recolour_dst[slot][e].len {
///         recolor(recolour_src[slot][e], recolour_dst[slot][e][kit.recolours[slot]])
/// }}}
/// ```
///
/// `65535` decodes as `-1` (no entry); the bounds check above is the only
/// guard applied. The values come from the pack via
/// [`GraphicsDefaults::load`]; [`AvatarPalette`] resolves a kit through them.
/// Every other defaults opcode (hitmarks, interfaces, spot shadow, ...) is
/// consumed and dropped so the stream stays aligned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphicsDefaults {
    /// Recolour sources `[slot 0..10][entry 0..4]`.
    pub recolour_src: [[i32; 4]; 10],
    /// Recolour destinations `[slot][entry][kit index]`.
    pub recolour_dst: [[Vec<i32>; 4]; 10],
    /// Retexture sources `[slot 0..10][entry 0..4]`.
    pub retexture_src: [[i32; 4]; 10],
    /// Retexture destinations `[slot][entry][kit index]`.
    pub retexture_dst: [[Vec<i32>; 4]; 10],
    /// The default camera state is 3 (cam2) instead of 2 (follow).
    pub cam2_default: bool,
    /// Map element sprite scale percent (default 100).
    pub map_element_scale: i32,
    /// Hint-arrow sprite id (default -1).
    pub hintarrows: i32,
    /// The minimap's on-map hint-arrow sprites.
    pub hintarrow_minimap: i32,
    /// The rotated minimap-edge hint-arrow sprites.
    pub hintarrow_edges: i32,
    pub mapflag: i32,
    /// Draw offsets of the map flag.
    pub mapflag_offset: [i32; 2],
    /// The click cross frames.
    pub cross: i32,
    pub mapdots: i32,
    pub compass: i32,
}

/// Graphics defaults while their opcodes are being read.
struct DefaultsDraft {
    defaults: GraphicsDefaults,
    /// Hitmark count, set by opcode 3 (default 4): the hitmark position
    /// opcode repeats per hitmark.
    marks: usize,
}

/// Graphics defaults opcodes of this revision (only the palette and sprite
/// opcodes are kept).
static DEFAULTS_OPCODES: Table<DefaultsDraft, anyhow::Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Custom(skip_hitmark_positions)),
        Entry::new(at(2), Rule::Skip(&[Field::SmartId])),
        Entry::new(at(3), Rule::Byte(|d, _, v| d.marks = usize::from(v))),
        // Flags without payload.
        Entry::new(at(4), Rule::Skip(&[])),
        Entry::new(span(5, 6), Rule::Skip(&[Field::Medium])),
        Entry::new(
            at(7),
            Rule::Custom(|s, d, _| {
                read_palette(
                    s,
                    &mut d.defaults.recolour_src,
                    &mut d.defaults.recolour_dst,
                )
            }),
        ),
        Entry::new(at(8), Rule::Skip(&[])),
        Entry::new(at(9), Rule::Skip(&[Field::Byte])),
        Entry::new(at(10), Rule::Skip(&[])),
        Entry::new(at(11), Rule::Skip(&[Field::Byte])),
        Entry::new(at(12), Rule::Skip(&[Field::Short, Field::Short])),
        Entry::new(span(13, 15), Rule::Skip(&[Field::Byte])),
        Entry::new(at(16), Rule::Flag(|d, _| d.defaults.cam2_default = true)),
        Entry::new(span(17, 19), Rule::Skip(&[Field::Int])),
        Entry::new(at(20), Rule::Skip(&[Field::Short, Field::Byte])),
        Entry::new(
            at(21),
            Rule::Byte(|d, _, v| d.defaults.map_element_scale = i32::from(v)),
        ),
        Entry::new(at(22), Rule::Custom(read_interface_sprites)),
        Entry::new(
            at(23),
            Rule::Custom(|s, d, _| {
                read_palette(
                    s,
                    &mut d.defaults.retexture_src,
                    &mut d.defaults.retexture_dst,
                )
            }),
        ),
    ],
    Unknown::Reject,
);

/// One signed `(x, y)` pair per hitmark, dropped.
fn skip_hitmark_positions(
    source: Input<anyhow::Error>,
    d: &mut DefaultsDraft,
    _: Slot,
) -> Result<()> {
    for _ in 0..d.marks {
        source.signed_short()?;
        source.signed_short()?;
    }
    Ok(())
}

/// A palette table: for each of 10 slots and 4 entries a source id and a
/// counted list of destination ids.
fn read_palette(
    source: Input<anyhow::Error>,
    src: &mut [[i32; 4]; 10],
    dst: &mut [[Vec<i32>; 4]; 10],
) -> Result<()> {
    for slot in 0..10 {
        for entry in 0..4 {
            src[slot][entry] = read_palette_id(source)?;
            let count = source.short()?;
            let mut list = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                list.push(read_palette_id(source)?);
            }
            dst[slot][entry] = list;
        }
    }
    Ok(())
}

/// The sprite ids of the interface, in stored order; fonts and unrelated
/// sprites are read and dropped.
fn read_interface_sprites(
    source: Input<anyhow::Error>,
    d: &mut DefaultsDraft,
    _: Slot,
) -> Result<()> {
    let out = &mut d.defaults;
    // Three fonts.
    for _ in 0..3 {
        source.smart_id()?;
    }
    out.hintarrows = source.smart_id()?;
    out.hintarrow_minimap = source.smart_id()?;
    out.mapflag = source.smart_id()?;
    out.mapflag_offset = [
        i32::from(source.signed_byte()?),
        i32::from(source.signed_byte()?),
    ];
    out.cross = source.smart_id()?;
    out.mapdots = source.smart_id()?;
    // Font icons and one more sprite, dropped.
    source.smart_id()?;
    source.smart_id()?;
    out.compass = source.smart_id()?;
    // The submenu arrow, dropped.
    source.smart_id()?;
    out.hintarrow_edges = source.smart_id()?;
    Ok(())
}

/// One palette id: a short where `65535` means `-1` (no entry).
fn read_palette_id(source: Input<anyhow::Error>) -> Result<i32> {
    payload::nullable_short(source)
}

impl GraphicsDefaults {
    /// Load file 3 of the `defaults` archive and decode the palette tables.
    pub fn load(pack: &Pack) -> Result<Self> {
        let files = pack
            .read_group(DEFAULTS_ARCHIVE, GRAPHICS_DEFAULTS_GROUP)
            .with_context(|| format!("{DEFAULTS_ARCHIVE} group {GRAPHICS_DEFAULTS_GROUP}"))?;
        // The group carries one file; take file 0 when present, else the
        // lowest file id (both orders coincide on real packs).
        let first = files.keys().min().copied().unwrap_or(0);
        let bytes = files.get(&first).with_context(|| {
            format!("{DEFAULTS_ARCHIVE} group {GRAPHICS_DEFAULTS_GROUP} has no file {first}")
        })?;
        Self::decode(bytes)
    }

    /// Decode one graphics-defaults file. An opcode outside the table is an
    /// error, like the IDK decoder.
    pub fn decode(data: &[u8]) -> Result<Self> {
        let draft = DefaultsDraft {
            defaults: Self {
                map_element_scale: 100,
                hintarrows: -1,
                hintarrow_minimap: -1,
                hintarrow_edges: -1,
                mapflag: -1,
                cross: -1,
                mapdots: -1,
                compass: -1,
                ..Self::default()
            },
            marks: 4,
        };
        let record = Record {
            kind: "graphics defaults",
            id: -1,
        };
        decode_record(&DEFAULTS_OPCODES, "avatar", record, data, draft).map(|d| d.defaults)
    }
}

/// One kit's palette selection: the 10 recolour and 10 retexture indices the
/// client reads per player (server defaults are all zero).
/// [`from_kit`](Self::from_kit) snapshots a kit, and the `*_pairs` resolvers
/// expand the indices through [`GraphicsDefaults`] into upload-ready maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvatarPalette {
    /// Kit recolour indices (10 entries).
    pub recolours: [u8; 10],
    /// Kit retexture indices (10 entries).
    pub retextures: [u8; 10],
}

impl AvatarPalette {
    /// Snapshot the palette selection out of a kit.
    #[must_use]
    pub fn from_kit(kit: &AvatarKit) -> Self {
        Self {
            recolours: kit.recolours,
            retextures: kit.retextures,
        }
    }

    /// Kit-level recolour pairs: `(src, dst)` per slot/entry whose destination
    /// list covers the kit index. `-1` endpoints pass through untouched (the
    /// index-bounds check is the only guard); use
    /// [`recolor_pairs_u16`](Self::recolor_pairs_u16) for upload-ready pairs.
    #[must_use]
    pub fn recolor_pairs(&self, defaults: &GraphicsDefaults) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for slot in 0..10 {
            let index = usize::from(self.recolours[slot]);
            for entry in 0..4 {
                if let Some(&dst) = defaults.recolour_dst[slot][entry].get(index) {
                    out.push((defaults.recolour_src[slot][entry], dst));
                }
            }
        }
        out
    }

    /// Kit-level retexture pairs, resolved like [`recolor_pairs`](Self::recolor_pairs).
    #[must_use]
    pub fn retexture_pairs(&self, defaults: &GraphicsDefaults) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for slot in 0..10 {
            let index = usize::from(self.retextures[slot]);
            for entry in 0..4 {
                if let Some(&dst) = defaults.retexture_dst[slot][entry].get(index) {
                    out.push((defaults.retexture_src[slot][entry], dst));
                }
            }
        }
        out
    }

    /// Upload-ready recolour pairs: [`recolor_pairs`](Self::recolor_pairs)
    /// with negative (`-1` = no entry) endpoints dropped, narrowed to the
    /// `u16` HSL domain for [`crate::model::Mesh::from_raw_recolored`].
    /// Applies post-merge on both the body and the head node.
    #[must_use]
    pub fn recolor_pairs_u16(&self, defaults: &GraphicsDefaults) -> Vec<(u16, u16)> {
        filter_u16_pairs(self.recolor_pairs(defaults))
    }

    /// Upload-ready retexture pairs for [`crate::model::remap_face_materials`].
    /// Applies post-merge on both the body and the head node.
    #[must_use]
    pub fn retexture_pairs_u16(&self, defaults: &GraphicsDefaults) -> Vec<(u16, u16)> {
        filter_u16_pairs(self.retexture_pairs(defaults))
    }
}

/// Drop pairs with a negative endpoint and narrow the rest to `u16` (shared
/// by [`AvatarPalette::recolor_pairs_u16`] and
/// [`AvatarPalette::retexture_pairs_u16`]).
fn filter_u16_pairs(pairs: Vec<(i32, i32)>) -> Vec<(u16, u16)> {
    pairs
        .into_iter()
        .filter_map(|(src, dst)| {
            if src >= 0 && dst >= 0 {
                Some((src as u16, dst as u16))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a non-negative id the way the two-or-four byte smart reads it:
    /// below `0x8000` as 2-byte big-endian, larger as 4 bytes with the high
    /// bit set.
    fn smart2or4(value: u32) -> Vec<u8> {
        if value < 0x8000 {
            vec![(value >> 8) as u8, value as u8]
        } else {
            let raw = value | 0x8000_0000;
            vec![
                (raw >> 24) as u8,
                (raw >> 16) as u8,
                (raw >> 8) as u8,
                raw as u8,
            ]
        }
    }

    #[test]
    fn idk_decodes_bodypart_models_heads_and_recols() {
        // opcode 1: bodypart 2. opcode 2: models [5, dropped -1]. opcode 3:
        // no-op. opcode 40: one recolor. opcode 41: one retexture.
        // opcodes 60/61: heads [7, -1]. opcodes 44/45: index tables.
        let mut data = vec![1, 2, 2, 2];
        data.extend_from_slice(&smart2or4(5));
        data.extend_from_slice(&[0x7F, 0xFF]); // 32767 -> -1 -> dropped
        data.push(3);
        data.extend_from_slice(&[40, 1, 0x12, 0x34, 0x56, 0x78]);
        data.extend_from_slice(&[41, 1, 0x00, 0x01, 0x00, 0x02]);
        data.push(44);
        data.extend_from_slice(&[0x00, 0xFF]);
        data.push(45);
        data.extend_from_slice(&[0x00, 0x01]);
        data.push(60);
        data.extend_from_slice(&smart2or4(7));
        data.push(61);
        data.extend_from_slice(&[0x7F, 0xFF]);
        data.push(0);

        let idk = decode_idk(18, &data).unwrap();
        assert_eq!(idk.id, 18);
        assert_eq!(idk.bodypart, 2);
        assert_eq!(idk.models, vec![5]);
        assert_eq!(idk.recol_s, vec![0x1234]);
        assert_eq!(idk.recol_d, vec![0x5678]);
        assert_eq!(idk.retex_s, vec![1]);
        assert_eq!(idk.retex_d, vec![2]);
        assert_eq!(idk.heads, [7, -1, -1, -1, -1]);
    }

    #[test]
    fn idk_head_opcodes_past_slot_4_stay_aligned() {
        // Opcodes 65-69 address no heads slot (array length 5); the smart
        // must still be consumed so the terminator aligns.
        let mut data = vec![65];
        data.extend_from_slice(&smart2or4(9));
        data.push(0);
        let idk = decode_idk(1, &data).unwrap();
        assert_eq!(idk.heads, [-1, -1, -1, -1, -1]);
    }

    #[test]
    fn idk_unknown_opcode_and_truncation_are_errors() {
        assert!(decode_idk(1, &[77, 0]).is_err());
        let err = decode_idk(1, &[77, 0]).unwrap_err();
        assert!(err.to_string().contains("unknown opcode 77"), "{err}");
        // Missing terminator.
        assert!(decode_idk(1, &[1]).is_err());
        // Opcode 2 claims a model but the smart is cut off.
        assert!(decode_idk(1, &[2, 1, 0]).is_err());
        assert!(decode_idk(1, &[]).is_err());
    }

    /// Synthetic graphics-defaults file: opcode 7 with slot 0 / entry 0 =
    /// src 100 -> dst [200, 201] and every other entry src -1 / no dst;
    /// opcode 23 with slot 1 / entry 2 = src 300 -> dst [301]; terminator.
    fn synthetic_graphics_defaults_bytes() -> Vec<u8> {
        let mut data = vec![7];
        for slot in 0..10 {
            for entry in 0..4 {
                if slot == 0 && entry == 0 {
                    data.extend_from_slice(&[0, 100, 0, 2, 0, 200, 0, 201]);
                } else {
                    // src 0xFFFF (-1, no entry), dst count 0.
                    data.extend_from_slice(&[0xFF, 0xFF, 0, 0]);
                }
            }
        }
        data.push(23);
        for slot in 0..10 {
            for entry in 0..4 {
                if slot == 1 && entry == 2 {
                    data.extend_from_slice(&[1, 44, 0, 1, 1, 45]); // src 300, dst [301]
                } else {
                    data.extend_from_slice(&[0xFF, 0xFF, 0, 0]);
                }
            }
        }
        data.push(0);
        data
    }

    #[test]
    fn graphics_defaults_decode_tables() {
        let defaults = GraphicsDefaults::decode(&synthetic_graphics_defaults_bytes()).unwrap();
        assert_eq!(defaults.recolour_src[0][0], 100);
        assert_eq!(defaults.recolour_dst[0][0], vec![200, 201]);
        assert_eq!(defaults.recolour_src[0][1], -1);
        assert!(defaults.recolour_dst[9][3].is_empty());
        assert_eq!(defaults.retexture_src[1][2], 300);
        assert_eq!(defaults.retexture_dst[1][2], vec![301]);
        // Terminator + unknown-opcode discipline.
        assert!(GraphicsDefaults::decode(&[]).is_err());
        assert!(GraphicsDefaults::decode(&[77, 0]).is_err());
    }

    #[test]
    fn avatar_palette_resolves_kit_indices() {
        let defaults = GraphicsDefaults::decode(&synthetic_graphics_defaults_bytes()).unwrap();
        let mut kit = AvatarKit::default_male();
        kit.recolours[0] = 1; // dst[0][0][1] = 201
        kit.retextures[1] = 0; // dst[1][2][0] = 301
        let palette = AvatarPalette::from_kit(&kit);
        assert_eq!(palette.recolours[0], 1);
        // Slot 0 / entry 0 resolves; other entries in slot 0 have empty dst
        // lists (index 1 out of range -> skipped, the bounds check).
        assert_eq!(palette.recolor_pairs(&defaults), vec![(100, 201)]);
        assert_eq!(palette.recolor_pairs_u16(&defaults), vec![(100, 201)]);
        assert_eq!(palette.retexture_pairs(&defaults), vec![(300, 301)]);
        // Out-of-range kit index resolves to nothing (never panics).
        kit.recolours[0] = 9;
        let palette = AvatarPalette::from_kit(&kit);
        assert!(palette.recolor_pairs(&defaults).is_empty());
        // -1 endpoints never reach the u16 upload pairs.
        let mut neg = GraphicsDefaults::default();
        neg.recolour_src[2][1] = -1;
        neg.recolour_dst[2][1] = vec![-1];
        let palette = AvatarPalette::from_kit(&AvatarKit::default_male());
        assert_eq!(neg.recolour_dst[2][1], vec![-1]);
        assert!(palette.recolor_pairs_u16(&neg).is_empty());
        assert_eq!(
            palette.recolor_pairs(&neg).len(),
            1,
            "raw pairs keep their negative endpoints"
        );
    }
}
