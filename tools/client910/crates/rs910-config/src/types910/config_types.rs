//! CPU-side item and NPC configs: the fields entity code needs (names, worn
//! models, palettes, morph list, ambient sound), decoded from complete opcode
//! tables so no byte is misaligned. Fields not needed by CPU entities stay
//! available in `raw`; this is not the renderer's config schema. An opcode
//! outside the table is an error naming the id and the opcode.
use super::{appearance, npc, npc_custom, zone, Error, Packet, Result};
use crate::opcode_table::{at, span, Entry, Field, Input, Record, Rule, Slot, Table, Unknown};
use crate::types910::customisation::Customisation;
use std::collections::{BTreeMap, BTreeSet};

/// One item type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: i32,
    pub name: String,
    pub cost: i32,
    pub stackable: i32,
    pub members: bool,
    pub team: i32,
    pub wear: [i32; 3],
    pub custom: Customisation,
    pub recolour_source: Option<Vec<i16>>,
    pub retexture_source: Option<Vec<i16>>,
    pub man_offset: [i32; 3],
    pub woman_offset: [i32; 3],
    /// Noted, lent, bought and shard kinds, each `[template, link]`; the raw
    /// 16-bit values are kept, so an absent id reads 65535.
    pub derived: [[i32; 2]; 4],
    pub shardcount: i32,
    pub shardname: String,
    pub consumed: usize,
    pub raw: Vec<u8>,
}

impl Item {
    pub fn empty(id: i32) -> Self {
        Self {
            id,
            name: "null".into(),
            cost: 1,
            stackable: 0,
            members: false,
            team: 0,
            wear: [-1; 3],
            custom: Customisation {
                man: [-1; 3],
                woman: [-1; 3],
                man_head: [-1; 2],
                woman_head: [-1; 2],
                recolour: None,
                retexture: None,
            },
            recolour_source: None,
            retexture_source: None,
            man_offset: [0; 3],
            woman_offset: [0; 3],
            derived: [[-1; 2]; 4],
            shardcount: 0,
            shardname: "null".into(),
            consumed: 0,
            raw: vec![],
        }
    }

    pub fn decode(id: i32, bytes: &[u8]) -> Result<Self> {
        let mut item = Self::empty(id);
        item.raw = bytes.to_vec();
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "item",
            id: i64::from(id),
        };
        ITEM_OPCODES.run(&mut packet, &mut item, record)?;
        item.consumed = packet.pos;
        Ok(item)
    }

    pub fn appearance(&self) -> appearance::Item {
        appearance::Item {
            team: self.team,
            defaults: self.custom.clone(),
        }
    }

    pub fn ground_type(&self) -> zone::ObjectType {
        zone::ObjectType {
            cost: self.cost,
            stackable: self.stackable,
        }
    }
}

/// Item opcodes of this revision.
static ITEM_OPCODES: Table<Item, Error> = Table::new(
    &[
        // World model, dropped.
        Entry::new(at(1), Rule::Skip(&[Field::SmartId])),
        Entry::new(at(2), Rule::Text(|t, _, v| t.name = v)),
        // Inventory render parameters, dropped.
        Entry::new(span(4, 8), Rule::Skip(&[Field::Short])),
        Entry::new(at(11), Rule::Flag(|t, _| t.stackable = 1)),
        Entry::new(at(12), Rule::Int(|t, _, v| t.cost = v)),
        Entry::new(
            span(13, 14),
            Rule::Byte(|t, slot, v| t.wear[slot] = i32::from(v)),
        ),
        Entry::new(at(15), Rule::Skip(&[])),
        Entry::new(at(16), Rule::Flag(|t, _| t.members = true)),
        // Worn models: man, man, woman, woman.
        Entry::new(
            span(23, 26),
            Rule::SmartId(|t, slot, v| {
                if slot < 2 {
                    t.custom.man[slot] = v;
                } else {
                    t.custom.woman[slot - 2] = v;
                }
            }),
        ),
        Entry::new(at(27), Rule::Byte(|t, _, v| t.wear[2] = i32::from(v))),
        // Menu operations, dropped.
        Entry::new(span(30, 39), Rule::Skip(&[Field::Text])),
        Entry::new(
            at(40),
            Rule::Custom(|s, t, _| {
                let (src, dst) = read_pairs(s)?;
                t.recolour_source = Some(src);
                t.custom.recolour = Some(dst);
                Ok(())
            }),
        ),
        Entry::new(
            at(41),
            Rule::Custom(|s, t, _| {
                let (src, dst) = read_pairs(s)?;
                t.retexture_source = Some(src);
                t.custom.retexture = Some(dst);
                Ok(())
            }),
        ),
        Entry::new(at(42), Rule::Custom(skip_counted_bytes)),
        Entry::new(at(43), Rule::Skip(&[Field::Int])),
        Entry::new(span(44, 45), Rule::Skip(&[Field::Short])),
        Entry::new(at(65), Rule::Skip(&[])),
        Entry::new(at(78), Rule::SmartId(|t, _, v| t.custom.man[2] = v)),
        Entry::new(at(79), Rule::SmartId(|t, _, v| t.custom.woman[2] = v)),
        // Head models: man, woman, man, woman.
        Entry::new(
            span(90, 93),
            Rule::SmartId(|t, slot, v| {
                if slot % 2 == 0 {
                    t.custom.man_head[slot / 2] = v;
                } else {
                    t.custom.woman_head[slot / 2] = v;
                }
            }),
        ),
        Entry::new(span(94, 95), Rule::Skip(&[Field::Short])),
        Entry::new(at(96), Rule::Skip(&[Field::Byte])),
        // Derived kinds store the link first, then the template.
        Entry::new(
            span(97, 98),
            Rule::Short(|t, slot, v| t.derived[0][1 - slot] = i32::from(v)),
        ),
        Entry::new(span(100, 109), Rule::Skip(&[Field::Short, Field::Short])),
        Entry::new(span(110, 112), Rule::Skip(&[Field::Short])),
        Entry::new(span(113, 114), Rule::Skip(&[Field::Byte])),
        Entry::new(at(115), Rule::Byte(|t, _, v| t.team = i32::from(v))),
        Entry::new(
            span(121, 122),
            Rule::Short(|t, slot, v| t.derived[1][1 - slot] = i32::from(v)),
        ),
        Entry::new(span(125, 126), Rule::Custom(read_wear_offsets)),
        Entry::new(span(127, 130), Rule::Skip(&[Field::Byte, Field::Short])),
        Entry::new(at(132), Rule::Custom(skip_counted_shorts)),
        Entry::new(at(134), Rule::Skip(&[Field::Byte])),
        Entry::new(
            span(139, 140),
            Rule::Short(|t, slot, v| t.derived[2][1 - slot] = i32::from(v)),
        ),
        Entry::new(span(142, 146), Rule::Skip(&[Field::Short])),
        Entry::new(span(150, 154), Rule::Skip(&[Field::Short])),
        Entry::new(span(156, 157), Rule::Skip(&[])),
        Entry::new(
            span(161, 162),
            Rule::Short(|t, slot, v| t.derived[3][1 - slot] = i32::from(v)),
        ),
        Entry::new(at(163), Rule::Short(|t, _, v| t.shardcount = i32::from(v))),
        Entry::new(at(164), Rule::Text(|t, _, v| t.shardname = v)),
        Entry::new(at(165), Rule::Flag(|t, _| t.stackable = 2)),
        Entry::new(span(167, 168), Rule::Skip(&[])),
        Entry::new(at(249), Rule::Custom(skip_params)),
    ],
    Unknown::Reject,
);

/// A count byte, then that many bytes.
fn skip_counted_bytes(source: Input<Error>, _: &mut Item, _: Slot) -> Result<()> {
    for _ in 0..source.byte()? {
        source.byte()?;
    }
    Ok(())
}

/// A count byte, then that many 16-bit values.
fn skip_counted_shorts(source: Input<Error>, _: &mut Item, _: Slot) -> Result<()> {
    for _ in 0..source.byte()? {
        source.short()?;
    }
    Ok(())
}

/// Wear offsets for the man (first opcode) or the woman: three signed bytes,
/// scaled by four.
fn read_wear_offsets(source: Input<Error>, item: &mut Item, slot: Slot) -> Result<()> {
    let offsets = if slot == 0 {
        &mut item.man_offset
    } else {
        &mut item.woman_offset
    };
    for value in offsets {
        *value = i32::from(source.signed_byte()?) << 2;
    }
    Ok(())
}

/// A parameter table, read and dropped.
fn skip_params<T>(source: Input<Error>, _: &mut T, _: Slot) -> Result<()> {
    crate::config::read_params(source, &mut Vec::new())
}

/// A count byte, then that many `(source, destination)` pairs of 16-bit
/// values, as signed shorts.
fn read_pairs(source: Input<Error>) -> Result<(Vec<i16>, Vec<i16>)> {
    let count = source.byte()?;
    let mut src = vec![];
    let mut dst = vec![];
    for _ in 0..count {
        src.push(source.short()? as i16);
        dst.push(source.short()? as i16);
    }
    Ok((src, dst))
}

/// A count byte, then that many `(source, destination)` pairs of which only
/// the destinations are kept.
fn read_destinations(source: Input<Error>) -> Result<Vec<i16>> {
    let count = source.byte()?;
    let mut out = vec![];
    for _ in 0..count {
        source.short()?;
        out.push(source.short()? as i16);
    }
    Ok(out)
}

/// A count byte, then that many 16-bit-or-wider ids.
fn read_id_list(source: Input<Error>) -> Result<Vec<i32>> {
    let count = source.byte()?;
    (0..count).map(|_| source.smart_id()).collect()
}

/// A 16-bit id where 65535 means "none" (`-1`).
fn read_nullable(source: Input<Error>) -> Result<i32> {
    crate::opcode_table::payload::nullable_short(source)
}

/// Resolve derived items (noted, lent, bought, shard): copy the fields their
/// kind takes from the template and the link, and apply the members filter.
/// Missing files resolve to the default item, not a render placeholder.
/// A derivation cycle, or a chain deeper than 128, is an error.
pub fn resolve_items(
    raw: &BTreeMap<i32, Item>,
    ids: impl IntoIterator<Item = i32>,
    members: bool,
) -> Result<BTreeMap<i32, Item>> {
    fn get(
        id: i32,
        raw: &BTreeMap<i32, Item>,
        done: &mut BTreeMap<i32, Item>,
        active: &mut BTreeSet<i32>,
        members: bool,
    ) -> Result<Item> {
        if let Some(t) = done.get(&id) {
            return Ok(t.clone());
        }
        if active.len() >= 128 || !active.insert(id) {
            return Err(Error::UnsupportedContext("item derivation cycle/depth"));
        }
        let mut t = raw.get(&id).cloned().unwrap_or_else(|| Item::empty(id));
        if let Some(kind) = (0..4).find(|&k| t.derived[k][0] != -1) {
            let template = get(t.derived[kind][0], raw, done, active, members)?;
            let link = get(t.derived[kind][1], raw, done, active, members)?;
            let colour = if kind == 0 { &template } else { &link };
            t.recolour_source = colour.recolour_source.clone();
            t.retexture_source = colour.retexture_source.clone();
            t.custom.recolour = colour.custom.recolour.clone();
            t.custom.retexture = colour.custom.retexture.clone();
            t.name = link.name.clone();
            t.members = link.members;
            match kind {
                0 => {
                    t.cost = link.cost;
                    t.stackable = 1;
                }
                3 => {
                    if link.shardcount == 0 {
                        return Err(Error::Invalid("zero shard count"));
                    }
                    t.name = link.shardname;
                    t.cost = link.cost.wrapping_div(link.shardcount);
                    t.stackable = 1;
                }
                _ => {
                    t.cost = 0;
                    t.stackable = link.stackable;
                    t.wear = link.wear;
                    t.custom = link.custom;
                    t.man_offset = link.man_offset;
                    t.woman_offset = link.woman_offset;
                    t.team = link.team;
                }
            }
        }
        if !members && t.members {
            t.team = 0;
        }
        active.remove(&id);
        done.insert(id, t.clone());
        Ok(t)
    }
    let mut done = BTreeMap::new();
    let mut active = BTreeSet::new();
    for id in ids {
        get(id, raw, &mut done, &mut active, members)?;
    }
    Ok(done)
}
/// One NPC type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Npc {
    pub id: i32,
    pub name: String,
    pub size: i32,
    pub turnspeed: i32,
    pub vislevel: i32,
    pub bas: i32,
    pub models: Vec<i32>,
    pub heads: Option<Vec<i32>>,
    pub modeloffset: Option<Vec<Option<[i32; 3]>>>,
    pub recolour: Option<Vec<i16>>,
    pub retexture: Option<Vec<i16>>,
    pub headicons: Option<Vec<(i32, i16)>>,
    pub covermarker: i32,
    pub multivarbit: i32,
    pub multivarp: i32,
    pub multinpc: Option<Vec<i32>>,
    pub sounds: [i32; 4],
    pub sound_range: i32,
    pub sound_dropoff: i32,
    pub sound_volume: i32,
    pub sound_rates: [i32; 2],
    pub active: bool,
    pub walksmoothing: bool,
    pub respawndir: Option<i32>,
    /// Bit 0 lets the client stand the NPC on a map square's spawn list.
    pub walkflags: i32,
    pub fade: i32,
    pub tint: [i8; 4],
    pub consumed: usize,
    pub raw: Vec<u8>,
}

/// An NPC while its opcodes are being read.
struct NpcDraft {
    npc: Npc,
    /// Set by the model list opcode; per-model offsets need it first.
    models_present: bool,
}

impl Npc {
    pub fn empty(id: i32) -> Self {
        Self {
            id,
            name: "null".into(),
            size: 1,
            turnspeed: 32,
            vislevel: -1,
            bas: -1,
            models: vec![],
            heads: None,
            modeloffset: None,
            recolour: None,
            retexture: None,
            headicons: None,
            covermarker: -1,
            multivarbit: -1,
            multivarp: -1,
            multinpc: None,
            sounds: [-1; 4],
            sound_range: 0,
            sound_dropoff: 0,
            sound_volume: 255,
            sound_rates: [256; 2],
            active: true,
            walksmoothing: true,
            respawndir: Some(4),
            walkflags: 0,
            fade: 0,
            tint: [0; 4],
            consumed: 0,
            raw: vec![],
        }
    }

    pub fn decode(id: i32, bytes: &[u8]) -> Result<Self> {
        let mut draft = NpcDraft {
            npc: Self::empty(id),
            models_present: false,
        };
        draft.npc.raw = bytes.to_vec();
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "NPC",
            id: i64::from(id),
        };
        NPC_OPCODES.run(&mut packet, &mut draft, record)?;
        draft.npc.consumed = packet.pos;
        Ok(draft.npc)
    }

    /// Whether the NPC plays a background sound of its own. The crawl-only
    /// sound is deliberately not counted.
    pub fn own_sound(&self) -> bool {
        self.sounds[0] != -1 || self.sounds[2] != -1 || self.sounds[3] != -1
    }
    /// Whether the NPC or, when it morphs, any direct morph target has a
    /// background sound; targets of targets are not followed.
    pub fn has_background_sound(&self, all: &BTreeMap<i32, Npc>) -> bool {
        if let Some(ids) = &self.multinpc {
            ids.iter()
                .filter(|&&id| id != -1)
                .any(|id| all.get(id).is_some_and(Npc::own_sound))
        } else {
            self.own_sound()
        }
    }
    pub fn packet_type(&self) -> npc::NpcType {
        npc::NpcType {
            size: self.size,
            turnspeed: self.turnspeed,
            vislevel: self.vislevel,
            name: self.name.clone(),
            head_icons: self.headicons.clone(),
            covermarker: self.covermarker,
            walkflags: self.walkflags,
            respawndir: self.respawndir,
            has_sound_or_multinpc: self.own_sound() || self.multinpc.is_some(),
        }
    }
    pub fn customisation(&self) -> npc_custom::Spec {
        npc_custom::Spec {
            recolour: self.recolour.as_ref().map(Vec::len),
            retexture: self.retexture.as_ref().map(Vec::len),
        }
    }
}

/// NPC opcodes of this revision.
static NPC_OPCODES: Table<NpcDraft, Error> = Table::new(
    &[
        Entry::new(
            at(1),
            Rule::Custom(|s, d, _| {
                d.npc.models = read_id_list(s)?;
                d.models_present = true;
                Ok(())
            }),
        ),
        Entry::new(at(2), Rule::Text(|d, _, v| d.npc.name = v)),
        Entry::new(at(12), Rule::Byte(|d, _, v| d.npc.size = i32::from(v))),
        // Menu operations, dropped.
        Entry::new(span(30, 34), Rule::Skip(&[Field::Text])),
        Entry::new(
            at(40),
            Rule::Custom(|s, d, _| {
                d.npc.recolour = Some(read_destinations(s)?);
                Ok(())
            }),
        ),
        Entry::new(
            at(41),
            Rule::Custom(|s, d, _| {
                d.npc.retexture = Some(read_destinations(s)?);
                Ok(())
            }),
        ),
        Entry::new(
            at(42),
            Rule::Custom(|s, _, _| {
                for _ in 0..s.byte()? {
                    s.byte()?;
                }
                Ok(())
            }),
        ),
        Entry::new(span(44, 45), Rule::Skip(&[Field::Short])),
        Entry::new(
            at(60),
            Rule::Custom(|s, d, _| {
                d.npc.heads = Some(read_id_list(s)?);
                Ok(())
            }),
        ),
        Entry::new(at(93), Rule::Skip(&[])),
        Entry::new(at(95), Rule::Short(|d, _, v| d.npc.vislevel = i32::from(v))),
        Entry::new(span(97, 98), Rule::Skip(&[Field::Short])),
        Entry::new(at(99), Rule::Skip(&[])),
        Entry::new(span(100, 101), Rule::Skip(&[Field::Byte])),
        Entry::new(at(102), Rule::Custom(read_head_icons)),
        Entry::new(
            at(103),
            Rule::Short(|d, _, v| d.npc.turnspeed = i32::from(v)),
        ),
        Entry::new(
            at(106),
            Rule::Custom(|s, d, _| read_morphs(s, &mut d.npc, false)),
        ),
        Entry::new(at(107), Rule::Flag(|d, _| d.npc.active = false)),
        Entry::new(at(109), Rule::Flag(|d, _| d.npc.walksmoothing = false)),
        Entry::new(at(111), Rule::Skip(&[])),
        Entry::new(at(113), Rule::Skip(&[Field::Short, Field::Short])),
        Entry::new(at(114), Rule::Skip(&[Field::Byte, Field::Byte])),
        Entry::new(
            at(118),
            Rule::Custom(|s, d, _| read_morphs(s, &mut d.npc, true)),
        ),
        Entry::new(
            at(119),
            Rule::SignedByte(|d, _, v| d.npc.walkflags = i32::from(v)),
        ),
        Entry::new(at(121), Rule::Custom(read_model_offsets)),
        Entry::new(at(123), Rule::Skip(&[Field::Short])),
        Entry::new(
            at(125),
            Rule::SignedByte(|d, _, v| {
                let direction = i32::from(v);
                d.npc.respawndir = if (0..8).contains(&direction) {
                    Some(direction)
                } else {
                    None
                };
            }),
        ),
        Entry::new(at(127), Rule::Short(|d, _, v| d.npc.bas = i32::from(v))),
        Entry::new(at(128), Rule::Skip(&[Field::Byte])),
        Entry::new(
            at(134),
            Rule::Custom(|s, d, _| {
                for sound in &mut d.npc.sounds {
                    *sound = read_nullable(s)?;
                }
                d.npc.sound_range = i32::from(s.byte()?);
                Ok(())
            }),
        ),
        Entry::new(span(135, 136), Rule::Skip(&[Field::Byte, Field::Short])),
        Entry::new(at(137), Rule::Skip(&[Field::Short])),
        Entry::new(at(138), Rule::SmartId(|d, _, v| d.npc.covermarker = v)),
        Entry::new(
            at(140),
            Rule::Byte(|d, _, v| d.npc.sound_volume = i32::from(v)),
        ),
        Entry::new(at(141), Rule::Skip(&[])),
        Entry::new(at(142), Rule::Skip(&[Field::Short])),
        Entry::new(at(143), Rule::Skip(&[])),
        Entry::new(span(150, 154), Rule::Skip(&[Field::Text])),
        Entry::new(
            at(155),
            Rule::Custom(|s, d, _| {
                for value in &mut d.npc.tint {
                    *value = s.byte()? as i8;
                }
                Ok(())
            }),
        ),
        Entry::new(span(158, 159), Rule::Skip(&[])),
        Entry::new(
            at(160),
            Rule::Custom(|s, _, _| {
                for _ in 0..s.byte()? {
                    s.short()?;
                }
                Ok(())
            }),
        ),
        Entry::new(at(162), Rule::Skip(&[])),
        Entry::new(at(163), Rule::Skip(&[Field::Byte])),
        Entry::new(
            at(164),
            Rule::Custom(|s, d, _| {
                d.npc.sound_rates = [i32::from(s.short()?), i32::from(s.short()?)];
                Ok(())
            }),
        ),
        Entry::new(at(165), Rule::Skip(&[Field::Byte])),
        Entry::new(
            at(168),
            Rule::Byte(|d, _, v| d.npc.sound_dropoff = i32::from(v)),
        ),
        Entry::new(at(169), Rule::Skip(&[])),
        Entry::new(span(170, 175), Rule::Skip(&[Field::Short])),
        Entry::new(at(178), Rule::Skip(&[])),
        Entry::new(
            at(179),
            Rule::Skip(&[
                Field::Smart,
                Field::Smart,
                Field::Smart,
                Field::Smart,
                Field::Smart,
                Field::Smart,
            ]),
        ),
        Entry::new(at(180), Rule::Byte(|d, _, v| d.npc.fade = i32::from(v))),
        Entry::new(at(181), Rule::Skip(&[Field::Short, Field::Byte])),
        Entry::new(at(182), Rule::Skip(&[])),
        Entry::new(at(249), Rule::Custom(skip_params)),
    ],
    Unknown::Reject,
);

/// Head-icon table: a bit mask, then per set bit a model id and an icon id.
/// Clear bits stay as `(-1, -1)`.
fn read_head_icons(source: Input<Error>, draft: &mut NpcDraft, _: Slot) -> Result<()> {
    let mask = u32::from(source.byte()?);
    let width = 32 - mask.leading_zeros();
    let mut icons = vec![];
    for bit in 0..width {
        icons.push(if mask & (1 << bit) == 0 {
            (-1, -1)
        } else {
            (source.smart_id()?, (source.smart()? - 1) as i16)
        });
    }
    draft.npc.headicons = Some(icons);
    Ok(())
}

/// Morph list: varbit, varp, an optional explicit fallback, a count, then
/// the targets; the fallback goes last.
fn read_morphs(source: Input<Error>, npc: &mut Npc, has_fallback: bool) -> Result<()> {
    npc.multivarbit = read_nullable(source)?;
    npc.multivarp = read_nullable(source)?;
    let fallback = if has_fallback {
        read_nullable(source)?
    } else {
        -1
    };
    let count = source.smart()?;
    let mut list = vec![];
    for _ in 0..=count {
        list.push(read_nullable(source)?);
    }
    list.push(fallback);
    npc.multinpc = Some(list);
    Ok(())
}

/// Per-model offsets, indexed by model slot; needs the model list first.
fn read_model_offsets(source: Input<Error>, draft: &mut NpcDraft, _: Slot) -> Result<()> {
    if !draft.models_present {
        return Err(Error::Invalid("NPC offset before models"));
    }
    let mut offsets = vec![None; draft.npc.models.len()];
    let count = source.byte()?;
    for _ in 0..count {
        let index = usize::from(source.byte()?);
        let offset = [
            i32::from(source.signed_byte()?),
            i32::from(source.signed_byte()?),
            i32::from(source.signed_byte()?),
        ];
        *offsets
            .get_mut(index)
            .ok_or(Error::Invalid("NPC model offset index"))? = Some(offset);
    }
    draft.npc.modeloffset = Some(offsets);
    Ok(())
}
