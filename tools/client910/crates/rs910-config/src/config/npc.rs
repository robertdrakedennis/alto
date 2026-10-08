//! NPC configs: names, models, size, menu operations and the morph list.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::{
    decode_records, load_all, select_multi, AllowMembers, ParamValue, NPC_ARCHIVE, NPC_GROUP_BITS,
};
use crate::cache::Pack;
use crate::opcode_table::{
    at, decode_record, payload, span, Entry, Field, Input, Record, Rule, Slot, Table, Unknown,
};

/// One NPC type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Npc {
    /// Config id.
    pub id: u32,
    /// Display name; `"null"` when the type has none.
    pub name: String,
    /// Combat level shown in menus; `-1` for none.
    pub vislevel: i32,
    /// Collision footprint in tiles.
    pub size: u8,
    /// Model ids, unset entries dropped.
    pub models: Vec<u32>,
    /// Chat head model ids, unset entries dropped.
    pub head_models: Vec<u32>,
    /// Recolour sources.
    pub recol_s: Vec<u16>,
    /// Recolour destinations, parallel to `recol_s`.
    pub recol_d: Vec<u16>,
    /// Retexture sources.
    pub retex_s: Vec<u16>,
    /// Retexture destinations, parallel to `retex_s`.
    pub retex_d: Vec<u16>,
    /// Palette indirection for the recolour destinations: pair `i` below the
    /// list's length recolours to `clientpalette[recol_d_palette[i] & 0xFF]`.
    pub recol_d_palette: Vec<i8>,
    /// Tint hue.
    pub tint_hue: i8,
    /// Tint saturation.
    pub tint_saturation: i8,
    /// Tint luminence.
    pub tint_luminence: i8,
    /// Tint weight; zero disables the tint.
    pub tint_weight: i8,
    /// Model scale along X.
    pub resize_x: i32,
    /// Model scale along Y.
    pub resize_y: i32,
    /// Ambient light adjustment.
    pub ambient: i32,
    /// Contrast adjustment.
    pub contrast: i32,
    /// Body animation set id; `-1` for none.
    pub bas: i32,
    /// Menu operations; two opcode families write the same five slots.
    pub ops: [Option<String>; 5],
    /// Bit `i` is set when `ops[i]` was last written by the members opcode
    /// family, which stores nothing on a free world.
    pub members_op_slots: u8,
    /// Per-operation menu cursors; the sixth slot belongs to the examine
    /// entry.
    pub cursor: [i32; 6],
    /// Type parameters, kept for target-mode checks.
    pub params: Vec<(i32, ParamValue)>,
    /// Quest icon ids shown in menu text.
    pub quests: Vec<i32>,
    /// Cursor used for the attack operation.
    pub cursorattack: i32,
    /// Sprite group of the cover marker.
    pub covermarker: i32,
    /// Whether the attack operation is moved to the top of the menu: 1 (the
    /// default when the cache says nothing) or 0.
    pub reprioritise_attack_op: i8,
    /// Whether the NPC shows a minimap dot.
    pub minimap: bool,
    /// Whether the NPC can be interacted with.
    pub active: bool,
    /// Map element (icon) id; `-1` for none.
    pub mapelement: i32,
    /// Varbit selecting the morph; `-1` for none.
    pub multivarbit: i32,
    /// Varp selecting the morph; `-1` for none.
    pub multivarp: i32,
    /// Morph targets; empty when the NPC does not morph. The last entry is
    /// the fallback (`-1` for none).
    pub multinpc: Vec<i32>,
    /// Marks a stand-in NPC for a transmog effect.
    pub transmogfakenpc: bool,
    /// Model ids with the unset (`-1`) slots kept; interface customisations
    /// index this list.
    pub model_slots: Vec<i32>,
    /// Chat head models with the unset slots kept.
    pub head_slots: Option<Vec<i32>>,
    /// Recolour palette indices, one per bit of the stored mask (`-1` for a
    /// clear bit).
    pub recolindices: Option<Vec<i8>>,
    /// Retexture palette indices, in the same form.
    pub retexindices: Option<Vec<i8>>,
    /// Per-model offsets, indexed by model slot.
    pub modeloffset: Option<Vec<Option<[i32; 3]>>>,
    /// Drawn above other NPCs on its tile.
    pub drawabove: bool,
    /// Follower NPC.
    pub follower: bool,
    /// Drawn below other NPCs on its tile.
    pub drawbelow: bool,
    /// Cycles a newly added NPC fades in over; zero for no fade.
    pub fade_in: i32,
    /// Whether each NPC of the type gets a small random colour shift, so no
    /// two look exactly alike (on unless the cache clears it).
    pub antimacro: bool,
    /// Whether the NPC casts a ground shadow.
    pub spotshadow: bool,
    /// Ring colours of the shadow (HSL); unused with a shadow texture.
    pub spotshadow_colours: [i32; 2],
    /// Inner and outer ring transparency of the shadow (unsigned).
    pub spotshadow_trans: [i32; 2],
    /// Shadow material; `-1` for the graphics default.
    pub spotshadow_texture: i32,
    /// Shadow material transparency.
    pub spotshadow_texture_alpha: i32,
    /// Client-side spawn and wander flags: bit 0 lets a map square place the
    /// NPC, bit 1 lets it wander.
    pub walkflags: i32,
    /// Height of the overhead elements; `-1` for the model's own.
    pub overlayheight: i32,
    /// Picking: with `picksizeshift`, decides between the exact and the
    /// coarse triangle test; `-1` for none.
    pub picksize: i32,
    /// Extra pick tolerance in screen pixels.
    pub picksizeshift: i32,
    /// Replaces the model in picking: `[min x, min y, min z, max x, max y,
    /// max z]`.
    pub clickbox: Option<[i32; 6]>,
}

impl Npc {
    fn blank(id: u32) -> Self {
        Self {
            id,
            name: "null".to_string(),
            vislevel: -1,
            size: 1,
            models: Vec::new(),
            head_models: Vec::new(),
            recol_s: Vec::new(),
            recol_d: Vec::new(),
            retex_s: Vec::new(),
            retex_d: Vec::new(),
            recol_d_palette: Vec::new(),
            tint_hue: 0,
            tint_saturation: 0,
            tint_luminence: 0,
            tint_weight: 0,
            resize_x: 128,
            resize_y: 128,
            ambient: 0,
            contrast: 0,
            bas: -1,
            ops: [None, None, None, None, None],
            members_op_slots: 0,
            cursor: [-1; 6],
            params: Vec::new(),
            quests: Vec::new(),
            cursorattack: -1,
            covermarker: -1,
            reprioritise_attack_op: -1,
            minimap: true,
            active: true,
            mapelement: -1,
            multivarbit: -1,
            multivarp: -1,
            multinpc: Vec::new(),
            transmogfakenpc: false,
            model_slots: Vec::new(),
            head_slots: None,
            recolindices: None,
            retexindices: None,
            modeloffset: None,
            drawabove: false,
            follower: false,
            drawbelow: false,
            fade_in: 0,
            antimacro: true,
            spotshadow: true,
            spotshadow_colours: [0, 0],
            spotshadow_trans: [160, 240],
            spotshadow_texture: -1,
            spotshadow_texture_alpha: 0,
            walkflags: 0,
            overlayheight: -1,
            picksize: -1,
            picksizeshift: 0,
            clickbox: None,
        }
    }
}

/// NPC opcodes of this revision.
static NPC_OPCODES: Table<Npc, anyhow::Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Custom(read_models)),
        Entry::new(at(2), Rule::Text(|n, _, v| n.name = v)),
        Entry::new(at(12), Rule::Byte(|n, _, v| n.size = v)),
        Entry::new(
            span(30, 34),
            Rule::Text(|n, slot, v| {
                n.ops[slot] = Some(v);
                n.members_op_slots &= !(1 << slot);
            }),
        ),
        Entry::new(
            at(40),
            Rule::Custom(|s, n, _| {
                (n.recol_s, n.recol_d) = payload::pairs(s)?;
                Ok(())
            }),
        ),
        Entry::new(
            at(41),
            Rule::Custom(|s, n, _| {
                (n.retex_s, n.retex_d) = payload::pairs(s)?;
                Ok(())
            }),
        ),
        Entry::new(at(42), Rule::Custom(read_recolour_palette)),
        Entry::new(span(44, 45), Rule::Custom(read_index_mask)),
        Entry::new(at(60), Rule::Custom(read_head_models)),
        Entry::new(at(93), Rule::Flag(|n, _| n.minimap = false)),
        Entry::new(at(95), Rule::Short(|n, _, v| n.vislevel = i32::from(v))),
        Entry::new(at(97), Rule::Short(|n, _, v| n.resize_x = i32::from(v))),
        Entry::new(at(98), Rule::Short(|n, _, v| n.resize_y = i32::from(v))),
        Entry::new(at(99), Rule::Flag(|n, _| n.drawabove = true)),
        Entry::new(
            at(100),
            Rule::SignedByte(|n, _, v| n.ambient = i32::from(v)),
        ),
        Entry::new(
            at(101),
            Rule::SignedByte(|n, _, v| n.contrast = i32::from(v)),
        ),
        Entry::new(at(102), Rule::Custom(skip_head_icons)),
        Entry::new(at(103), Rule::Skip(&[Field::Short])),
        Entry::new(at(106), Rule::Custom(read_morphs)),
        Entry::new(at(107), Rule::Flag(|n, _| n.active = false)),
        Entry::new(at(109), Rule::Skip(&[])),
        Entry::new(at(111), Rule::Flag(|n, _| n.spotshadow = false)),
        Entry::new(
            at(113),
            Rule::Custom(|source, n, _| {
                n.spotshadow_colours = [i32::from(source.short()?), i32::from(source.short()?)];
                Ok(())
            }),
        ),
        Entry::new(
            at(114),
            Rule::Custom(|source, n, _| {
                n.spotshadow_trans = [i32::from(source.byte()?), i32::from(source.byte()?)];
                Ok(())
            }),
        ),
        Entry::new(at(118), Rule::Custom(read_morphs_with_fallback)),
        Entry::new(
            at(119),
            Rule::Byte(|n, _, v| n.walkflags = i32::from(v as i8)),
        ),
        Entry::new(at(121), Rule::Custom(read_model_offsets)),
        Entry::new(
            at(123),
            Rule::Short(|n, _, v| n.overlayheight = i32::from(v)),
        ),
        Entry::new(at(125), Rule::Skip(&[Field::Byte])),
        Entry::new(at(127), Rule::Short(|n, _, v| n.bas = i32::from(v))),
        Entry::new(at(128), Rule::Skip(&[Field::Byte])),
        Entry::new(
            at(134),
            Rule::Skip(&[
                Field::Short,
                Field::Short,
                Field::Short,
                Field::Short,
                Field::Byte,
            ]),
        ),
        Entry::new(span(135, 136), Rule::Skip(&[Field::Byte, Field::Short])),
        Entry::new(
            at(137),
            Rule::Short(|n, _, v| n.cursorattack = i32::from(v)),
        ),
        Entry::new(at(138), Rule::Custom(read_cover_marker)),
        Entry::new(at(140), Rule::Skip(&[Field::Byte])),
        Entry::new(at(141), Rule::Flag(|n, _| n.follower = true)),
        Entry::new(at(142), Rule::Short(|n, _, v| n.mapelement = i32::from(v))),
        Entry::new(at(143), Rule::Flag(|n, _| n.drawbelow = true)),
        Entry::new(
            span(150, 154),
            Rule::Text(|n, slot, v| {
                n.ops[slot] = Some(v);
                n.members_op_slots |= 1 << slot;
            }),
        ),
        Entry::new(at(155), Rule::Custom(read_tint)),
        Entry::new(at(158), Rule::Flag(|n, _| n.reprioritise_attack_op = 1)),
        Entry::new(at(159), Rule::Flag(|n, _| n.reprioritise_attack_op = 0)),
        Entry::new(at(160), Rule::Custom(read_quests)),
        Entry::new(at(162), Rule::Skip(&[])),
        Entry::new(at(163), Rule::Byte(|n, _, v| n.picksize = i32::from(v))),
        Entry::new(at(164), Rule::Skip(&[Field::Short, Field::Short])),
        Entry::new(
            at(165),
            Rule::Byte(|n, _, v| n.picksizeshift = i32::from(v)),
        ),
        Entry::new(at(168), Rule::Skip(&[Field::Byte])),
        Entry::new(at(169), Rule::Flag(|n, _| n.antimacro = false)),
        Entry::new(
            span(170, 175),
            Rule::Short(|n, slot, v| {
                n.cursor[slot] = if v == 65535 { -1 } else { i32::from(v) };
            }),
        ),
        Entry::new(at(178), Rule::Skip(&[])),
        Entry::new(
            at(179),
            Rule::Custom(|source, n, _| {
                let mut bounds = [0; 6];
                for bound in &mut bounds {
                    *bound = source.smart_signed()?;
                }
                n.clickbox = Some(bounds);
                Ok(())
            }),
        ),
        Entry::new(at(180), Rule::Byte(|n, _, v| n.fade_in = i32::from(v))),
        Entry::new(
            at(181),
            Rule::Custom(|source, n, _| {
                n.spotshadow_texture = i32::from(source.short()? as i16);
                n.spotshadow_texture_alpha = i32::from(source.byte()?);
                Ok(())
            }),
        ),
        Entry::new(at(182), Rule::Flag(|n, _| n.transmogfakenpc = true)),
        Entry::new(
            at(249),
            Rule::Custom(|s, n, _| super::read_params(s, &mut n.params)),
        ),
    ],
    Unknown::Reject,
);

fn read_models(source: Input<anyhow::Error>, npc: &mut Npc, _: Slot) -> Result<()> {
    let count = source.byte()?;
    npc.models.clear();
    npc.model_slots.clear();
    for _ in 0..count {
        let model = source.smart_id()?;
        npc.model_slots.push(model);
        if model >= 0 {
            npc.models.push(model as u32);
        }
    }
    Ok(())
}

fn read_head_models(source: Input<anyhow::Error>, npc: &mut Npc, _: Slot) -> Result<()> {
    let count = source.byte()?;
    npc.head_models.clear();
    let mut slots = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let head = source.smart_id()?;
        slots.push(head);
        if head >= 0 {
            npc.head_models.push(head as u32);
        }
    }
    npc.head_slots = Some(slots);
    Ok(())
}

fn read_recolour_palette(source: Input<anyhow::Error>, npc: &mut Npc, _: Slot) -> Result<()> {
    let count = source.byte()?;
    npc.recol_d_palette.clear();
    for _ in 0..count {
        npc.recol_d_palette.push(source.signed_byte()?);
    }
    Ok(())
}

/// A 16-bit mask whose set bits enumerate palette slots: one running index
/// per set bit, `-1` per clear bit, up to the highest set bit. Slot 0 is the
/// recolour table, slot 1 the retexture table.
fn read_index_mask(source: Input<anyhow::Error>, npc: &mut Npc, slot: Slot) -> Result<()> {
    let mask = i32::from(source.short()?);
    let mut width = 0;
    let mut rest = mask;
    while rest > 0 {
        width += 1;
        rest >>= 1;
    }
    let mut next = 0i8;
    let indices = (0..width)
        .map(|bit| {
            if mask & (1 << bit) > 0 {
                next = next.wrapping_add(1);
                next.wrapping_sub(1)
            } else {
                -1
            }
        })
        .collect();
    if slot == 0 {
        npc.recolindices = Some(indices);
    } else {
        npc.retexindices = Some(indices);
    }
    Ok(())
}

/// Head-icon table: a bit mask, then per set bit a model id and an icon id.
/// Nothing is kept.
fn skip_head_icons(source: Input<anyhow::Error>, _: &mut Npc, _: Slot) -> Result<()> {
    let mask = source.byte()?;
    let mut width = 0_u32;
    let mut rest = mask;
    while rest != 0 {
        width += 1;
        rest >>= 1;
    }
    for bit in 0..width {
        if u32::from(mask) & (1_u32 << bit) == 0 {
            continue;
        }
        let _model = source.smart_id()?;
        let _icon = source.smart_nullable()?;
    }
    Ok(())
}

fn read_morphs(source: Input<anyhow::Error>, npc: &mut Npc, _: Slot) -> Result<()> {
    read_morph_list(source, npc, false)
}

fn read_morphs_with_fallback(source: Input<anyhow::Error>, npc: &mut Npc, _: Slot) -> Result<()> {
    read_morph_list(source, npc, true)
}

fn read_morph_list(source: Input<anyhow::Error>, npc: &mut Npc, has_fallback: bool) -> Result<()> {
    npc.multivarbit = payload::nullable_short(source)?;
    npc.multivarp = payload::nullable_short(source)?;
    let fallback = if has_fallback {
        payload::nullable_short(source)?
    } else {
        -1
    };
    let count = source.smart()?;
    if count < 0 {
        return Err(anyhow::anyhow!(
            "npc {}: negative multinpc count {count}",
            npc.id
        ));
    }
    npc.multinpc.clear();
    for _ in 0..=count {
        npc.multinpc.push(payload::nullable_short(source)?);
    }
    npc.multinpc.push(fallback);
    Ok(())
}

/// Per-model offsets: a count, then `(model slot, x, y, z)`.
fn read_model_offsets(source: Input<anyhow::Error>, npc: &mut Npc, _: Slot) -> Result<()> {
    let mut offsets = vec![None; npc.model_slots.len()];
    let count = source.byte()?;
    for _ in 0..count {
        let slot = usize::from(source.byte()?);
        let offset = [
            i32::from(source.signed_byte()?),
            i32::from(source.signed_byte()?),
            i32::from(source.signed_byte()?),
        ];
        *offsets
            .get_mut(slot)
            .with_context(|| format!("npc {}: modeloffset slot {slot} out of range", npc.id))? =
            Some(offset);
    }
    npc.modeloffset = Some(offsets);
    Ok(())
}

/// A 16-bit id, or a 31-bit one when the first byte has its top bit set. Unlike
/// [`Source::smart_id`], 32767 is an ordinary value here.
fn read_cover_marker(source: Input<anyhow::Error>, npc: &mut Npc, _: Slot) -> Result<()> {
    npc.covermarker = if (source.peek()? as i8) >= 0 {
        i32::from(source.short()?)
    } else {
        source.int()? & i32::MAX
    };
    Ok(())
}

fn read_tint(source: Input<anyhow::Error>, npc: &mut Npc, _: Slot) -> Result<()> {
    npc.tint_hue = source.signed_byte()?;
    npc.tint_saturation = source.signed_byte()?;
    npc.tint_luminence = source.signed_byte()?;
    npc.tint_weight = source.signed_byte()?;
    Ok(())
}

fn read_quests(source: Input<anyhow::Error>, npc: &mut Npc, _: Slot) -> Result<()> {
    let count = source.byte()?;
    npc.quests.clear();
    for _ in 0..count {
        npc.quests.push(i32::from(source.short()?));
    }
    Ok(())
}

/// Decode one npc entry. An unknown opcode is an error naming the id and the
/// opcode.
pub fn decode_npc(id: u32, data: &[u8]) -> Result<Npc> {
    let record = Record {
        kind: "npc",
        id: i64::from(id),
    };
    let mut npc = decode_record(&NPC_OPCODES, "config", record, data, Npc::blank(id))?;
    // Unless the cache says otherwise, the attack operation moves up.
    if npc.reprioritise_attack_op < 0 {
        npc.reprioritise_attack_op = 1;
    }
    Ok(npc)
}

impl Npc {
    /// The morph target selected by the varbit/varp value `read` returns;
    /// callers test for a non-empty `multinpc` first.
    pub fn multi_npc(&self, read: &dyn Fn(bool, i32) -> Option<i32>) -> Option<u32> {
        select_multi(self.multivarbit, self.multivarp, &self.multinpc, read)
    }

    /// The operations as decoded for a world that allows members content or
    /// not: the members opcode family stores nothing on a free world.
    pub fn ops_for(&self, allow_members: bool) -> [Option<&str>; 5] {
        std::array::from_fn(|slot| {
            if !allow_members && self.members_op_slots & (1 << slot) != 0 {
                None
            } else {
                self.ops[slot].as_deref()
            }
        })
    }
}

/// All npc configs, keyed by id.
#[derive(Clone, Debug, Default)]
pub struct NpcStore {
    entries: BTreeMap<u32, Npc>,
    /// Whether members-only content applies.
    pub allow_members: AllowMembers,
}

impl NpcStore {
    /// A store over decoded NPCs, so tests can inject synthetic ones.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn from_map(entries: BTreeMap<u32, Npc>) -> Self {
        Self {
            entries,
            allow_members: AllowMembers::default(),
        }
    }

    /// Decode every file of the npc archive (`id = group << 7 | file`).
    pub fn load(pack: &Pack) -> Result<Self> {
        Ok(Self {
            entries: load_all(pack, NPC_ARCHIVE, NPC_GROUP_BITS, decode_npc)?,
            allow_members: AllowMembers::default(),
        })
    }

    /// [`Self::load`] over the npc archive already read (ids as keys).
    pub fn from_records(files: &BTreeMap<i32, Vec<u8>>) -> Result<Self> {
        Ok(Self {
            entries: decode_records(NPC_ARCHIVE, files, decode_npc)?,
            allow_members: AllowMembers::default(),
        })
    }

    /// Look up one npc by id.
    pub fn get(&self, id: u32) -> Option<&Npc> {
        self.entries.get(&id)
    }

    /// Number of decoded npcs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store holds no npcs.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(id, npc)` in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&u32, &Npc)> {
        self.entries.iter()
    }
}
