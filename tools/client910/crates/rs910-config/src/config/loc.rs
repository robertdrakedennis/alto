//! Scenery ("loc") configs: footprint, collision, models, animation, map
//! icons, background sound and the morph list.

use std::collections::BTreeMap;

use anyhow::Result;

use super::{
    decode_records, load_all, select_multi, AllowMembers, ParamValue, LOC_ARCHIVE, LOC_GROUP_BITS,
};
use crate::cache::Pack;
use crate::opcode_table::{
    at, decode_record, payload, span, Entry, Field, Input, Record, Rule, Slot, Table, Unknown,
};

/// One scenery type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loc {
    /// Config id.
    pub id: u32,
    /// Display name; `"null"` when the type has none.
    pub name: String,
    /// Examine text. No opcode of this revision stores one, so the default is
    /// what the server sends for every loc.
    pub desc: Option<String>,
    /// Tile footprint along X.
    pub width: u8,
    /// Tile footprint along Z.
    pub length: u8,
    /// Collision: 2 blocks, 1 blocks only partially, 0 is walkable. A type
    /// that breaks route finding is forced to 0 after decoding.
    pub blockwalk: u8,
    /// Whether the loc blocks ranged attacks over it. The stock client
    /// ignores the stored flag; it is kept for the server-side rules.
    pub blockrange: bool,
    /// Model ids of every shape, flattened in stream order, without the
    /// unset (`-1`) entries.
    pub models: Vec<u32>,
    /// Recolour sources.
    pub recol_s: Vec<u16>,
    /// Recolour destinations, parallel to `recol_s`.
    pub recol_d: Vec<u16>,
    /// Palette indirection for the recolour destinations (empty when absent).
    /// When non-empty, pair `i` recolours to
    /// `clientpalette[recol_d_palette[i] & 0xFF]` instead of `recol_d[i]`.
    pub recol_d_palette: Vec<i8>,
    /// Retexture sources.
    pub retex_s: Vec<u16>,
    /// Retexture destinations, parallel to `retex_s`.
    pub retex_d: Vec<u16>,
    /// Ambient light adjustment (raw signed byte).
    pub ambient: i8,
    /// Contrast adjustment, stored as five times the raw signed byte.
    pub contrast: i16,
    /// Tint hue.
    pub tint_hue: i8,
    /// Tint saturation.
    pub tint_saturation: i8,
    /// Tint luminence.
    pub tint_luminence: i8,
    /// Tint weight; zero leaves the model untinted.
    pub tint_weight: i8,
    /// Menu operations. Two opcode families write the same five slots; see
    /// `members_op_slots`.
    pub ops: [Option<String>; 5],
    /// Per-operation menu cursors; the sixth slot belongs to the examine
    /// entry and is never set by the cache.
    pub cursor: [i32; 6],
    /// Type parameters, kept for target-mode checks in the scene menu.
    pub params: Vec<(i32, ParamValue)>,
    /// Quest icon ids shown in menu text.
    pub quests: Vec<i32>,
    /// Whether the loc casts a hard shadow onto the floor shade map under
    /// walls and centrepieces.
    pub has_hard_shadow: bool,

    /// Model ids per shape, `-1` entries kept.
    pub shape_models: Vec<(i8, Vec<i32>)>,
    /// `active` after the members-allowed post-decode rule.
    pub active: i32,
    /// `active` when members content is not allowed: only operations that do
    /// not come from the members opcodes count.
    pub active_restricted: i32,
    /// Bit `i` is set when `ops[i]` was last written by the members opcode
    /// family, which stores nothing on a free world.
    pub members_op_slots: u8,
    /// Members-only loc.
    pub members: bool,
    /// Map scene icon rotates with the loc.
    pub mapsceneiconrotate: bool,
    /// Rotation applied to the map icon.
    pub map_icon_rotation: i32,
    /// Map scene icon sprite; `-1` for none.
    pub mapsceneicon: i32,
    /// Map scene icon is mirrored.
    pub mapsceneiconmirror: bool,
    /// Map element (icon) id; `-1` for none.
    pub mapelement: i32,
    /// Terrain follow mode: 0 none, 1 to 5 the shapes the stream selects.
    pub hillchange: i8,
    /// Parameter of the terrain follow mode.
    pub hillchange_value: i32,
    /// Lighting is shared with neighbouring tiles.
    pub sharelight: bool,
    /// Occlusion: `-1` unset, 1 occludes, 0 explicitly does not.
    pub occlude: i32,
    /// Occluder width.
    pub occlude_width: i32,
    /// Occluder height.
    pub occlude_height: i32,
    /// The loc has an animation.
    pub has_anim: bool,
    /// Animation sequence ids.
    pub anims: Vec<i32>,
    /// Animation weights, normalised to 65535.
    pub anim_weights: Vec<i32>,
    /// Start the animation on a random frame.
    pub random_anim_frame: bool,
    /// Skip the animation at low detail.
    pub disable_anim_low_detail: bool,
    /// Wall offset in sub-tile units.
    pub walloff: i32,
    /// Model is mirrored.
    pub mirror: bool,
    /// Model scale along X.
    pub resizex: i32,
    /// Model scale along Y.
    pub resizey: i32,
    /// Model scale along Z.
    pub resizez: i32,
    /// Model offset along X.
    pub xoff: i32,
    /// Model offset along Y.
    pub yoff: i32,
    /// Model offset along Z.
    pub zoff: i32,
    /// Offset applied after placement, along X.
    pub post_xoff: i32,
    /// Offset applied after placement, along Y.
    pub post_yoff: i32,
    /// Offset applied after placement, along Z.
    pub post_zoff: i32,
    /// Forces the decoration slot of a wall.
    pub forcedecor: bool,
    /// Whether ground objects are raised onto the loc; unset resolves to
    /// "raised unless walkable".
    pub raiseobject: i32,
    /// Decoration height.
    pub decor_height: i32,
    /// The loc morphs with a variable.
    pub has_multiloc: bool,
    /// Varbit selecting the morph; `-1` for none.
    pub multivarbit: i32,
    /// Varp selecting the morph; `-1` for none.
    pub multivarp: i32,
    /// Morph targets; the last entry is the fallback (`-1` hides the loc).
    pub multiloc: Vec<i32>,
    /// The loc is drawn as a texture.
    pub istexture: bool,
    /// Whether the entity hard-shadow blob is drawn, distinct from
    /// `has_hard_shadow`.
    pub hardshadow: bool,
    /// Always built as a dynamic entity.
    pub force_dynamic: bool,
    /// `active` after the post-decode rule: dynamic when animated, forced or
    /// morphing.
    pub always_dynamic: bool,
    /// Anti-macro marker.
    pub antimacro: bool,
    /// Scene layer: 1 primary (default), 0, or 2 for a loc that shares its
    /// tile without taking the primary layer.
    pub layer: i32,
    /// Ambient sound id; `-1` for none.
    pub bgsound_sound: i32,
    /// Ambient sound range in tiles.
    pub bgsound_range: i32,
    /// Ambient sound fall-off range in tiles.
    pub bgsound_dropoffrange: i32,
    /// Ambient sound volume.
    pub bgsound_volume: i32,
    /// Shortest delay between plays.
    pub bgsound_mindelay: i32,
    /// Longest delay between plays.
    pub bgsound_maxdelay: i32,
    /// Sounds chosen between at random, when the block is present.
    pub bgsound_random: Option<Vec<i32>>,
    /// Slowest playback rate.
    pub bgsound_minrate: i32,
    /// Fastest playback rate.
    pub bgsound_maxrate: i32,
    /// Click box as `[min_x, min_y, min_z, max_x, max_y, max_z]`; `None`
    /// picks against the drawn model.
    pub clickbox: Option<[i32; 6]>,
    /// The two level-of-detail category bytes the stock client drops: the
    /// first defaults to 5 (automatic), the second to 0.
    pub nxt_lod: [u8; 2],
}

impl Loc {
    /// A type with every field at its default.
    fn blank(id: u32) -> Self {
        Self {
            id,
            name: "null".to_string(),
            desc: Some(rs910_core::texts::Msg::Examine.get().to_string()),
            width: 1,
            length: 1,
            blockwalk: 2,
            blockrange: true,
            models: Vec::new(),
            recol_s: Vec::new(),
            recol_d: Vec::new(),
            recol_d_palette: Vec::new(),
            retex_s: Vec::new(),
            retex_d: Vec::new(),
            ambient: 0,
            contrast: 0,
            tint_hue: 0,
            tint_saturation: 0,
            tint_luminence: 0,
            tint_weight: 0,
            ops: [None, None, None, None, None],
            cursor: [-1; 6],
            params: Vec::new(),
            quests: Vec::new(),
            has_hard_shadow: true,
            shape_models: Vec::new(),
            active: -1,
            active_restricted: -1,
            members_op_slots: 0,
            members: false,
            mapsceneiconrotate: false,
            map_icon_rotation: 0,
            mapsceneicon: -1,
            mapsceneiconmirror: false,
            mapelement: -1,
            hillchange: 0,
            hillchange_value: -1,
            sharelight: false,
            occlude: -1,
            occlude_width: 960,
            occlude_height: 0,
            has_anim: false,
            anims: Vec::new(),
            anim_weights: Vec::new(),
            random_anim_frame: true,
            disable_anim_low_detail: false,
            walloff: 64,
            mirror: false,
            resizex: 128,
            resizey: 128,
            resizez: 128,
            xoff: 0,
            yoff: 0,
            zoff: 0,
            post_xoff: 0,
            post_yoff: 0,
            post_zoff: 0,
            forcedecor: false,
            raiseobject: -1,
            decor_height: 0,
            has_multiloc: false,
            multivarbit: -1,
            multivarp: -1,
            multiloc: Vec::new(),
            istexture: false,
            hardshadow: true,
            force_dynamic: false,
            always_dynamic: false,
            antimacro: false,
            layer: 1,
            bgsound_sound: -1,
            bgsound_range: 0,
            bgsound_dropoffrange: 0,
            bgsound_volume: 255,
            bgsound_mindelay: 0,
            bgsound_maxdelay: 0,
            bgsound_random: None,
            bgsound_minrate: 256,
            bgsound_maxrate: 256,
            clickbox: None,
            nxt_lod: [5, 0],
        }
    }
}

/// A loc while its opcodes are being read.
struct LocDraft {
    loc: Loc,
    /// Opcode 74: applied after everything else, see [`finish`].
    breaks_route_finding: bool,
}

/// Loc opcodes of this revision.
static LOC_OPCODES: Table<LocDraft, anyhow::Error> = Table::new(
    &[
        Entry::new(at(1), Rule::Custom(read_shape_models)),
        Entry::new(at(2), Rule::Text(|d, _, v| d.loc.name = v)),
        Entry::new(at(14), Rule::Byte(|d, _, v| d.loc.width = v)),
        Entry::new(at(15), Rule::Byte(|d, _, v| d.loc.length = v)),
        Entry::new(at(17), Rule::Flag(|d, _| d.loc.blockwalk = 0)),
        Entry::new(at(18), Rule::Flag(|d, _| d.loc.blockrange = false)),
        Entry::new(at(19), Rule::Byte(|d, _, v| d.loc.active = i32::from(v))),
        Entry::new(at(21), Rule::Flag(|d, _| d.loc.hillchange = 1)),
        Entry::new(at(22), Rule::Flag(|d, _| d.loc.sharelight = true)),
        Entry::new(at(23), Rule::Flag(|d, _| d.loc.occlude = 1)),
        Entry::new(at(24), Rule::SmartId(read_single_anim)),
        Entry::new(at(27), Rule::Flag(|d, _| d.loc.blockwalk = 1)),
        Entry::new(
            at(28),
            Rule::Byte(|d, _, v| d.loc.walloff = i32::from(v) << 2),
        ),
        Entry::new(at(29), Rule::SignedByte(|d, _, v| d.loc.ambient = v)),
        Entry::new(
            at(39),
            Rule::SignedByte(|d, _, v| d.loc.contrast = i16::from(v) * 5),
        ),
        Entry::new(span(30, 34), Rule::Text(set_base_op)),
        Entry::new(at(40), Rule::Custom(read_recolours)),
        Entry::new(at(41), Rule::Custom(read_retextures)),
        Entry::new(at(42), Rule::Custom(read_recolour_palette)),
        Entry::new(span(44, 45), Rule::Skip(&[Field::Short])),
        Entry::new(at(62), Rule::Flag(|d, _| d.loc.mirror = true)),
        Entry::new(at(64), Rule::Flag(|d, _| d.loc.has_hard_shadow = false)),
        Entry::new(at(65), Rule::Short(|d, _, v| d.loc.resizex = i32::from(v))),
        Entry::new(at(66), Rule::Short(|d, _, v| d.loc.resizey = i32::from(v))),
        Entry::new(at(67), Rule::Short(|d, _, v| d.loc.resizez = i32::from(v))),
        // Stored but not used by the client.
        Entry::new(at(69), Rule::Skip(&[Field::Byte])),
        Entry::new(
            at(70),
            Rule::SignedShort(|d, _, v| d.loc.xoff = i32::from(v) << 2),
        ),
        Entry::new(
            at(71),
            Rule::SignedShort(|d, _, v| d.loc.yoff = i32::from(v) << 2),
        ),
        Entry::new(
            at(72),
            Rule::SignedShort(|d, _, v| d.loc.zoff = i32::from(v) << 2),
        ),
        Entry::new(at(73), Rule::Flag(|d, _| d.loc.forcedecor = true)),
        Entry::new(at(74), Rule::Flag(|d, _| d.breaks_route_finding = true)),
        Entry::new(
            at(75),
            Rule::Byte(|d, _, v| d.loc.raiseobject = i32::from(v)),
        ),
        Entry::new(at(77), Rule::Custom(read_morphs)),
        Entry::new(at(78), Rule::Custom(read_ambient_sound)),
        Entry::new(at(79), Rule::Custom(read_ambient_sound_block)),
        Entry::new(
            at(81),
            Rule::Byte(|d, _, v| {
                d.loc.hillchange = 2;
                d.loc.hillchange_value = i32::from(v) * 256;
            }),
        ),
        Entry::new(at(82), Rule::Flag(|d, _| d.loc.istexture = true)),
        Entry::new(at(88), Rule::Flag(|d, _| d.loc.hardshadow = false)),
        Entry::new(at(89), Rule::Flag(|d, _| d.loc.random_anim_frame = false)),
        Entry::new(at(91), Rule::Flag(|d, _| d.loc.members = true)),
        Entry::new(at(92), Rule::Custom(read_morphs_with_fallback)),
        Entry::new(
            at(93),
            Rule::Short(|d, _, v| {
                d.loc.hillchange = 3;
                d.loc.hillchange_value = i32::from(v);
            }),
        ),
        Entry::new(at(94), Rule::Flag(|d, _| d.loc.hillchange = 4)),
        Entry::new(
            at(95),
            Rule::SignedShort(|d, _, v| {
                d.loc.hillchange = 5;
                d.loc.hillchange_value = i32::from(v);
            }),
        ),
        Entry::new(at(97), Rule::Flag(|d, _| d.loc.mapsceneiconrotate = true)),
        Entry::new(at(98), Rule::Flag(|d, _| d.loc.force_dynamic = true)),
        Entry::new(span(99, 100), Rule::Skip(&[Field::Byte, Field::Short])),
        Entry::new(
            at(101),
            Rule::Byte(|d, _, v| d.loc.map_icon_rotation = i32::from(v)),
        ),
        Entry::new(
            at(102),
            Rule::Short(|d, _, v| d.loc.mapsceneicon = i32::from(v)),
        ),
        Entry::new(at(103), Rule::Flag(|d, _| d.loc.occlude = 0)),
        Entry::new(
            at(104),
            Rule::Byte(|d, _, v| d.loc.bgsound_volume = i32::from(v)),
        ),
        Entry::new(at(105), Rule::Flag(|d, _| d.loc.mapsceneiconmirror = true)),
        Entry::new(at(106), Rule::Custom(read_weighted_anims)),
        Entry::new(
            at(107),
            Rule::Short(|d, _, v| d.loc.mapelement = i32::from(v)),
        ),
        Entry::new(span(150, 154), Rule::Text(set_members_op)),
        Entry::new(at(160), Rule::Custom(read_quests)),
        Entry::new(
            at(162),
            Rule::Int(|d, _, v| {
                d.loc.hillchange = 3;
                d.loc.hillchange_value = v;
            }),
        ),
        Entry::new(at(163), Rule::Custom(read_tint)),
        Entry::new(
            at(164),
            Rule::SignedShort(|d, _, v| d.loc.post_xoff = i32::from(v)),
        ),
        Entry::new(
            at(165),
            Rule::SignedShort(|d, _, v| d.loc.post_yoff = i32::from(v)),
        ),
        Entry::new(
            at(166),
            Rule::SignedShort(|d, _, v| d.loc.post_zoff = i32::from(v)),
        ),
        Entry::new(
            at(167),
            Rule::Short(|d, _, v| d.loc.decor_height = i32::from(v)),
        ),
        // Opcodes the client accepts and ignores, without payload.
        Entry::new(span(168, 169), Rule::Skip(&[])),
        Entry::new(at(170), Rule::Smart(|d, _, v| d.loc.occlude_width = v)),
        Entry::new(at(171), Rule::Smart(|d, _, v| d.loc.occlude_height = v)),
        Entry::new(at(173), Rule::Custom(read_ambient_sound_rates)),
        Entry::new(at(177), Rule::Flag(|d, _| d.loc.always_dynamic = true)),
        Entry::new(
            at(178),
            Rule::Byte(|d, _, v| d.loc.bgsound_dropoffrange = i32::from(v)),
        ),
        Entry::new(at(186), Rule::Byte(|d, _, v| d.loc.layer = i32::from(v))),
        Entry::new(at(188), Rule::Skip(&[])),
        Entry::new(at(189), Rule::Flag(|d, _| d.loc.antimacro = true)),
        Entry::new(
            span(190, 195),
            Rule::Short(|d, slot, v| d.loc.cursor[slot] = i32::from(v)),
        ),
        Entry::new(
            span(196, 197),
            Rule::Byte(|d, slot, v| d.loc.nxt_lod[slot] = v),
        ),
        Entry::new(span(198, 199), Rule::Skip(&[])),
        Entry::new(
            at(200),
            Rule::Flag(|d, _| d.loc.disable_anim_low_detail = true),
        ),
        Entry::new(at(201), Rule::Custom(read_clickbox)),
        Entry::new(
            at(249),
            Rule::Custom(|s, d, _| super::read_params(s, &mut d.loc.params)),
        ),
    ],
    Unknown::Reject,
);

fn set_base_op(d: &mut LocDraft, slot: Slot, text: String) {
    d.loc.ops[slot] = Some(text);
    d.loc.members_op_slots &= !(1 << slot);
}

fn set_members_op(d: &mut LocDraft, slot: Slot, text: String) {
    d.loc.ops[slot] = Some(text);
    d.loc.members_op_slots |= 1 << slot;
}

/// Shapes, each with its model list. The flattened `models` list only ever
/// grows, while `shape_models` is replaced.
fn read_shape_models(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    let shapes = source.byte()?;
    d.loc.shape_models.clear();
    for _ in 0..shapes {
        let shape = source.signed_byte()?;
        let count = source.byte()?;
        let mut ids = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            let model = source.smart_id()?;
            ids.push(model);
            if model >= 0 {
                d.loc.models.push(model as u32);
            }
        }
        d.loc.shape_models.push((shape, ids));
    }
    Ok(())
}

/// One animation; `-1` leaves the loc without one.
fn read_single_anim(d: &mut LocDraft, _: Slot, anim: i32) {
    if anim != -1 {
        d.loc.has_anim = true;
        d.loc.anims = vec![anim];
    }
}

fn read_recolours(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    (d.loc.recol_s, d.loc.recol_d) = payload::pairs(source)?;
    Ok(())
}

fn read_retextures(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    (d.loc.retex_s, d.loc.retex_d) = payload::pairs(source)?;
    Ok(())
}

fn read_recolour_palette(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    let count = source.byte()?;
    d.loc.recol_d_palette.clear();
    for _ in 0..count {
        d.loc.recol_d_palette.push(source.signed_byte()?);
    }
    Ok(())
}

/// Morph list without a fallback entry.
fn read_morphs(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    read_morph_list(source, d, false)
}

/// Morph list followed by an explicit fallback.
fn read_morphs_with_fallback(
    source: Input<anyhow::Error>,
    d: &mut LocDraft,
    _: Slot,
) -> Result<()> {
    read_morph_list(source, d, true)
}

fn read_morph_list(
    source: Input<anyhow::Error>,
    d: &mut LocDraft,
    has_fallback: bool,
) -> Result<()> {
    let loc = &mut d.loc;
    loc.has_multiloc = true;
    loc.multivarbit = payload::nullable_short(source)?;
    loc.multivarp = payload::nullable_short(source)?;
    let fallback = if has_fallback { source.smart_id()? } else { -1 };
    let count = source.smart()?;
    if count < 0 {
        return Err(anyhow::anyhow!(
            "loc {}: negative multiloc count {count}",
            loc.id
        ));
    }
    loc.multiloc.clear();
    for _ in 0..=count {
        loc.multiloc.push(source.smart_id()?);
    }
    loc.multiloc.push(fallback);
    Ok(())
}

fn read_ambient_sound(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    d.loc.bgsound_sound = i32::from(source.short()?);
    d.loc.bgsound_range = i32::from(source.byte()?);
    Ok(())
}

fn read_ambient_sound_block(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    d.loc.bgsound_mindelay = i32::from(source.short()?);
    d.loc.bgsound_maxdelay = i32::from(source.short()?);
    d.loc.bgsound_range = i32::from(source.byte()?);
    let count = source.byte()?;
    let mut random = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        random.push(i32::from(source.short()?));
    }
    d.loc.bgsound_random = Some(random);
    Ok(())
}

fn read_ambient_sound_rates(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    d.loc.bgsound_minrate = i32::from(source.short()?);
    d.loc.bgsound_maxrate = i32::from(source.short()?);
    Ok(())
}

/// Weighted animations; the array exists even when empty, so the loc counts
/// as animated.
fn read_weighted_anims(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    let count = source.byte()?;
    let loc = &mut d.loc;
    loc.has_anim = true;
    loc.anims.clear();
    loc.anim_weights.clear();
    for _ in 0..count {
        loc.anims.push(source.smart_id()?);
        loc.anim_weights.push(i32::from(source.byte()?));
    }
    let total: i32 = loc.anim_weights.iter().sum();
    for weight in &mut loc.anim_weights {
        anyhow::ensure!(total != 0, "loc {}: zero animation weight total", loc.id);
        *weight = *weight * 65535 / total;
    }
    Ok(())
}

fn read_quests(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    let count = source.byte()?;
    d.loc.quests.clear();
    for _ in 0..count {
        d.loc.quests.push(i32::from(source.short()?));
    }
    Ok(())
}

fn read_tint(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    d.loc.tint_hue = source.signed_byte()?;
    d.loc.tint_saturation = source.signed_byte()?;
    d.loc.tint_luminence = source.signed_byte()?;
    d.loc.tint_weight = source.signed_byte()?;
    Ok(())
}

fn read_clickbox(source: Input<anyhow::Error>, d: &mut LocDraft, _: Slot) -> Result<()> {
    let mut bounds = [0; 6];
    for bound in &mut bounds {
        *bound = source.smart_signed()?;
    }
    d.loc.clickbox = Some(bounds);
    Ok(())
}

/// Rules that depend on the whole record, applied after the last opcode.
/// The active flag is derived before route finding is broken, so
/// `raiseobject` sees the original `blockwalk`.
fn finish(mut d: LocDraft) -> Loc {
    let loc = &mut d.loc;
    loc.active_restricted = loc.active;
    if loc.active == -1 {
        let centrepiece = loc.shape_models.len() == 1 && loc.shape_models[0].0 == 10;
        let restricted_ops = loc
            .ops
            .iter()
            .enumerate()
            .any(|(slot, op)| op.is_some() && loc.members_op_slots & (1 << slot) == 0);
        loc.active = i32::from(centrepiece || loc.ops.iter().any(Option::is_some));
        loc.active_restricted = i32::from(centrepiece || restricted_ops);
    }
    if loc.raiseobject == -1 {
        loc.raiseobject = if loc.blockwalk == 0 { 0 } else { 1 };
    }
    if loc.has_anim || loc.force_dynamic || loc.has_multiloc {
        loc.always_dynamic = true;
    }
    if d.breaks_route_finding {
        loc.blockwalk = 0;
    }
    d.loc
}

/// Decode one loc entry. An unknown opcode is an error naming the id and the
/// opcode.
pub fn decode_loc(id: u32, data: &[u8]) -> Result<Loc> {
    let draft = LocDraft {
        loc: Loc::blank(id),
        breaks_route_finding: false,
    };
    let record = Record {
        kind: "loc",
        id: i64::from(id),
    };
    decode_record(&LOC_OPCODES, "config", record, data, draft).map(finish)
}

impl Loc {
    /// The morph target selected by the varbit/varp value `read` returns;
    /// callers test for a non-empty `multiloc` first.
    pub fn multi_loc(&self, read: &dyn Fn(bool, i32) -> Option<i32>) -> Option<u32> {
        select_multi(self.multivarbit, self.multivarp, &self.multiloc, read)
    }

    /// The operations as decoded for a world that allows members content or
    /// not: the members opcode family stores nothing on a free world, and a
    /// members loc loses every operation there.
    pub fn ops_for(&self, allow_members: bool) -> [Option<&str>; 5] {
        std::array::from_fn(|slot| {
            if !allow_members && (self.members || self.members_op_slots & (1 << slot) != 0) {
                None
            } else {
                self.ops[slot].as_deref()
            }
        })
    }

    /// The quest icons a world that allows members content or not shows.
    pub fn quests_for(&self, allow_members: bool) -> &[i32] {
        if !allow_members && self.members {
            &[]
        } else {
            &self.quests
        }
    }

    /// `active` for a world that allows members content or not.
    pub fn active_for(&self, allow_members: bool) -> i32 {
        if allow_members {
            self.active
        } else {
            self.active_restricted
        }
    }
}

/// All loc configs, keyed by id.
#[derive(Clone, Debug, Default)]
pub struct LocStore {
    entries: BTreeMap<u32, Loc>,
    /// Whether members-only content applies.
    pub allow_members: AllowMembers,
}

impl LocStore {
    /// Decode every file of the loc archive (`id = group << 8 | file`).
    pub fn load(pack: &Pack) -> Result<Self> {
        rs910_core::profile::scope!("load loc configs");
        Ok(Self {
            entries: load_all(pack, LOC_ARCHIVE, LOC_GROUP_BITS, decode_loc)?,
            allow_members: AllowMembers::default(),
        })
    }

    /// [`Self::load`] over the loc archive already read (ids as keys).
    pub fn from_records(files: &BTreeMap<i32, Vec<u8>>) -> Result<Self> {
        rs910_core::profile::scope!("load loc configs");
        Ok(Self {
            entries: decode_records(LOC_ARCHIVE, files, decode_loc)?,
            allow_members: AllowMembers::default(),
        })
    }

    /// Look up one loc by id.
    pub fn get(&self, id: u32) -> Option<&Loc> {
        self.entries.get(&id)
    }

    /// Number of decoded locs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store holds no locs.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(id, loc)` in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&u32, &Loc)> {
        self.entries.iter()
    }

    /// A store over an explicit map, so tests can inject synthetic locs.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn from_map(map: BTreeMap<u32, Loc>) -> Self {
        Self {
            entries: map,
            allow_members: AllowMembers::default(),
        }
    }
}
