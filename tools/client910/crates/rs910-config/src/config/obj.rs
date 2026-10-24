//! Item ("obj") configs: appearance in the world and in inventories, menu
//! operations, and the derived kinds (noted, lent, bought, shard) that copy
//! most of their data from a template.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use super::{decode_records, load_all, AllowMembers, ParamValue, OBJ_ARCHIVE, OBJ_GROUP_BITS};
use crate::cache::Pack;
use crate::opcode_table::{
    at, decode_record, payload, span, Entry, Field, Input, Record, Rule, Slot, Table, Unknown,
};

/// One item type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Obj {
    pub inventory: ObjInventory,
    /// Config id.
    pub id: u32,
    /// Display name; `"null"` when the type has none.
    pub name: String,
    /// World model ids: zero or one entry, an unset model is dropped.
    pub models: Vec<u32>,
    /// Head model pairs, indexed by gender (0 male, 1 female) and then by
    /// head part.
    pub head_models: [[i32; 2]; 2],
    /// Recolour sources.
    pub recol_s: Vec<u16>,
    /// Recolour destinations, parallel to `recol_s`.
    pub recol_d: Vec<u16>,
    /// Retexture sources.
    pub retex_s: Vec<u16>,
    /// Retexture destinations, parallel to `retex_s`.
    pub retex_d: Vec<u16>,
    /// Ground operations.
    pub ops: [Option<String>; 5],
    /// Inventory operations.
    pub iops: [Option<String>; 5],
    /// Members-only item.
    pub members: bool,
    /// Menu colour override.
    pub minimenu_colour: Option<i32>,
    /// Type parameters in file order, looked up by key at run time.
    pub params: Vec<(i32, ParamValue)>,
    /// Category id; `-1` for none.
    pub category: i32,
    /// Ground stacks of this item scatter their models.
    pub scattered_drop: bool,
}

/// Inventory appearance and item metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjInventory {
    pub zoom: i32,
    pub angles: [i32; 3],
    pub offset: [i32; 2],
    pub resize: [i32; 3],
    pub ambient: i32,
    pub contrast: i32,
    /// 0 single, 1 stackable, 2 stackable with counts shown as models.
    pub stackable: i32,
    pub cost: i32,
    pub recol_palette: Vec<u8>,
    /// Models shown for stack sizes, as `(count, item)` pairs.
    pub countobj: Option<[(i32, i32); 10]>,
    /// The four derived kinds (noted, lent, bought, shard), each as
    /// `[link, template]`.
    pub derived: [[i32; 2]; 4],
    pub shardname: String,
    pub shardcount: i32,
    pub wearpos: [i32; 3],
    pub icursor: [i32; 5],
    pub cursor: [i32; 5],
    pub tradeable: bool,
    pub stockmarket: bool,
    pub cert_not_tradeable: bool,
    pub placeholder: bool,
    pub quests: Vec<i32>,
    /// Non-zero marks a dummy item, which is never tradeable.
    pub dummy: i32,
}

impl Default for ObjInventory {
    fn default() -> Self {
        Self {
            zoom: 2000,
            angles: [0; 3],
            offset: [0; 2],
            resize: [128; 3],
            ambient: 0,
            contrast: 0,
            stackable: 0,
            cost: 1,
            recol_palette: vec![],
            countobj: None,
            derived: [[-1; 2]; 4],
            shardname: "null".into(),
            shardcount: 0,
            wearpos: [-1; 3],
            icursor: [-1; 5],
            cursor: [-1; 5],
            tradeable: true,
            stockmarket: false,
            cert_not_tradeable: false,
            placeholder: true,
            quests: vec![],
            dummy: 0,
        }
    }
}

/// The ground operations every item starts with: "Take" in slot 2 (the
/// examine entry is appended by the menu owner).
pub fn obj_default_ops() -> [Option<String>; 5] {
    [
        None,
        None,
        Some(rs910_core::texts::Msg::Take.get().into()),
        None,
        None,
    ]
}

/// The inventory operations every item starts with: "Drop" in slot 4.
pub fn obj_default_iops() -> [Option<String>; 5] {
    [
        None,
        None,
        None,
        None,
        Some(rs910_core::texts::Msg::Drop.get().into()),
    ]
}

impl Obj {
    fn blank(id: u32) -> Self {
        Self {
            inventory: ObjInventory::default(),
            id,
            name: "null".to_string(),
            models: Vec::new(),
            head_models: [[-1; 2]; 2],
            recol_s: Vec::new(),
            recol_d: Vec::new(),
            retex_s: Vec::new(),
            retex_d: Vec::new(),
            ops: obj_default_ops(),
            iops: obj_default_iops(),
            members: false,
            minimenu_colour: None,
            params: Vec::new(),
            category: -1,
            scattered_drop: false,
        }
    }
}

/// Item opcodes of this revision.
static OBJ_OPCODES: Table<Obj, anyhow::Error> = Table::new(
    &[
        Entry::new(
            at(1),
            Rule::SmartId(|o, _, model| {
                o.models.clear();
                if model >= 0 {
                    o.models.push(model as u32);
                }
            }),
        ),
        Entry::new(at(2), Rule::Text(|o, _, v| o.name = v)),
        Entry::new(
            at(4),
            Rule::Short(|o, _, v| o.inventory.zoom = i32::from(v)),
        ),
        Entry::new(
            span(5, 6),
            Rule::Short(|o, slot, v| o.inventory.angles[slot] = i32::from(v)),
        ),
        Entry::new(
            span(7, 8),
            Rule::SignedShort(|o, slot, v| o.inventory.offset[slot] = i32::from(v)),
        ),
        Entry::new(at(11), Rule::Flag(|o, _| o.inventory.stackable = 1)),
        Entry::new(at(12), Rule::Int(|o, _, v| o.inventory.cost = v)),
        Entry::new(
            span(13, 14),
            Rule::Byte(|o, slot, v| o.inventory.wearpos[slot] = i32::from(v)),
        ),
        Entry::new(at(15), Rule::Flag(|o, _| o.inventory.tradeable = false)),
        Entry::new(at(16), Rule::Flag(|o, _| o.members = true)),
        // Worn model overrides, read and dropped.
        Entry::new(span(23, 26), Rule::Skip(&[Field::SmartId])),
        Entry::new(
            at(27),
            Rule::Byte(|o, _, v| o.inventory.wearpos[2] = i32::from(v)),
        ),
        Entry::new(span(30, 34), Rule::Text(|o, slot, v| o.ops[slot] = Some(v))),
        Entry::new(
            span(35, 39),
            Rule::Text(|o, slot, v| o.iops[slot] = Some(v)),
        ),
        Entry::new(
            at(40),
            Rule::Custom(|s, o, _| {
                (o.recol_s, o.recol_d) = payload::pairs(s)?;
                Ok(())
            }),
        ),
        Entry::new(
            at(41),
            Rule::Custom(|s, o, _| {
                (o.retex_s, o.retex_d) = payload::pairs(s)?;
                Ok(())
            }),
        ),
        Entry::new(at(42), Rule::Custom(read_recolour_palette)),
        Entry::new(at(43), Rule::Int(|o, _, v| o.minimenu_colour = Some(v))),
        Entry::new(span(44, 45), Rule::Skip(&[Field::Short])),
        Entry::new(at(65), Rule::Flag(|o, _| o.inventory.stockmarket = true)),
        Entry::new(span(78, 79), Rule::Skip(&[Field::SmartId])),
        Entry::new(
            span(90, 93),
            Rule::SmartId(|o, slot, v| o.head_models[slot / 2][slot % 2] = v),
        ),
        Entry::new(at(94), Rule::Short(|o, _, v| o.category = i32::from(v))),
        Entry::new(
            at(95),
            Rule::Short(|o, _, v| o.inventory.angles[2] = i32::from(v)),
        ),
        Entry::new(
            at(96),
            Rule::Byte(|o, _, v| o.inventory.dummy = i32::from(v)),
        ),
        Entry::new(
            span(97, 98),
            Rule::Short(|o, slot, v| o.inventory.derived[0][slot] = i32::from(v)),
        ),
        Entry::new(span(100, 109), Rule::Custom(read_stack_variant)),
        Entry::new(
            span(110, 112),
            Rule::Short(|o, slot, v| o.inventory.resize[slot] = i32::from(v)),
        ),
        Entry::new(
            at(113),
            Rule::SignedByte(|o, _, v| o.inventory.ambient = i32::from(v)),
        ),
        Entry::new(
            at(114),
            Rule::SignedByte(|o, _, v| o.inventory.contrast = i32::from(v)),
        ),
        Entry::new(at(115), Rule::Skip(&[Field::Byte])),
        Entry::new(
            span(121, 122),
            Rule::Short(|o, slot, v| o.inventory.derived[1][slot] = i32::from(v)),
        ),
        Entry::new(
            span(125, 126),
            Rule::Skip(&[Field::Byte, Field::Byte, Field::Byte]),
        ),
        Entry::new(span(127, 130), Rule::Skip(&[Field::Byte, Field::Short])),
        Entry::new(at(132), Rule::Custom(read_quests)),
        Entry::new(at(134), Rule::Skip(&[Field::Byte])),
        Entry::new(
            span(139, 140),
            Rule::Short(|o, slot, v| o.inventory.derived[2][slot] = i32::from(v)),
        ),
        Entry::new(
            span(142, 146),
            Rule::Short(|o, slot, v| o.inventory.cursor[slot] = i32::from(v)),
        ),
        Entry::new(
            span(150, 154),
            Rule::Short(|o, slot, v| o.inventory.icursor[slot] = i32::from(v)),
        ),
        Entry::new(at(156), Rule::Skip(&[])),
        Entry::new(at(157), Rule::Flag(|o, _| o.scattered_drop = true)),
        Entry::new(
            span(161, 162),
            Rule::Short(|o, slot, v| o.inventory.derived[3][slot] = i32::from(v)),
        ),
        Entry::new(
            at(163),
            Rule::Short(|o, _, v| o.inventory.shardcount = i32::from(v)),
        ),
        Entry::new(at(164), Rule::Text(|o, _, v| o.inventory.shardname = v)),
        Entry::new(at(165), Rule::Flag(|o, _| o.inventory.stackable = 2)),
        Entry::new(
            at(167),
            Rule::Flag(|o, _| o.inventory.cert_not_tradeable = true),
        ),
        Entry::new(at(168), Rule::Flag(|o, _| o.inventory.placeholder = false)),
        Entry::new(
            at(249),
            Rule::Custom(|s, o, _| super::read_params(s, &mut o.params)),
        ),
    ],
    Unknown::Reject,
);

/// Palette indirection for the recolour destinations; entries accumulate.
fn read_recolour_palette(source: Input<anyhow::Error>, obj: &mut Obj, _: Slot) -> Result<()> {
    let count = source.byte()?;
    for _ in 0..count {
        obj.inventory.recol_palette.push(source.byte()?);
    }
    Ok(())
}

/// Stack-size variants: slot `n` is the model shown from count `n`.
fn read_stack_variant(source: Input<anyhow::Error>, obj: &mut Obj, slot: Slot) -> Result<()> {
    let count = i32::from(source.short()?);
    let item = i32::from(source.short()?);
    obj.inventory.countobj.get_or_insert([(0, 0); 10])[slot] = (count, item);
    Ok(())
}

fn read_quests(source: Input<anyhow::Error>, obj: &mut Obj, _: Slot) -> Result<()> {
    let count = source.byte()?;
    for _ in 0..count {
        obj.inventory.quests.push(i32::from(source.short()?));
    }
    Ok(())
}

/// Decode one obj entry. An unknown opcode is an error naming the id and the
/// opcode.
pub fn decode_obj(id: u32, data: &[u8]) -> Result<Obj> {
    let record = Record {
        kind: "obj",
        id: i64::from(id),
    };
    decode_record(&OBJ_OPCODES, "config", record, data, Obj::blank(id))
}

/// Fill a derived item (noted, lent, bought, shard) from its template and
/// link, recursively, memoising in `out`.
fn resolve_obj(
    id: u32,
    raw: &BTreeMap<u32, Obj>,
    out: &mut BTreeMap<u32, Obj>,
    visiting: &mut BTreeSet<u32>,
) -> Result<Obj> {
    if let Some(o) = out.get(&id) {
        return Ok(o.clone());
    }
    anyhow::ensure!(visiting.insert(id), "cyclic item template {id}");
    let mut obj = raw.get(&id).cloned().unwrap_or(decode_obj(id, &[0])?);
    if let Some(kind) = obj.inventory.derived.iter().position(|pair| pair[1] != -1) {
        let [link, template] = obj.inventory.derived[kind];
        let t = resolve_obj(template as u32, raw, out, visiting)?;
        let l = resolve_obj(link as u32, raw, out, visiting)?;
        obj.models = t.models.clone();
        obj.head_models = t.head_models;
        obj.inventory.zoom = t.inventory.zoom;
        obj.inventory.angles = t.inventory.angles;
        obj.inventory.offset = t.inventory.offset;
        let colours = if kind == 0 { &t } else { &l };
        obj.recol_s = colours.recol_s.clone();
        obj.recol_d = colours.recol_d.clone();
        obj.inventory.recol_palette = colours.inventory.recol_palette.clone();
        obj.retex_s = colours.retex_s.clone();
        obj.retex_d = colours.retex_d.clone();
        obj.name = l.name.clone();
        obj.members = l.members;
        match kind {
            0 => {
                obj.inventory.cost = l.inventory.cost;
                obj.inventory.stackable = 1;
                obj.inventory.tradeable = !l.inventory.cert_not_tradeable && l.inventory.tradeable;
            }
            3 => {
                obj.name = l.inventory.shardname.clone();
                anyhow::ensure!(
                    l.inventory.shardcount != 0,
                    "zero shard count for item {}",
                    l.id
                );
                obj.inventory.cost = l.inventory.cost / l.inventory.shardcount;
                obj.inventory.stackable = 1;
                obj.inventory.stockmarket = l.inventory.stockmarket;
                obj.inventory.tradeable = l.inventory.tradeable;
                obj.category = t.category;
                obj.inventory.cursor = t.inventory.cursor;
                obj.inventory.icursor = t.inventory.icursor;
                obj.iops = [
                    Some(rs910_core::texts::Msg::ShardItemCombine.get().into()),
                    None,
                    None,
                    None,
                    Some(rs910_core::texts::Msg::Drop.get().into()),
                ];
            }
            _ => {
                obj.inventory.cost = 0;
                obj.inventory.stackable = l.inventory.stackable;
                obj.inventory.tradeable = false;
                obj.inventory.wearpos = l.inventory.wearpos;
                obj.category = l.category;
                obj.ops = l.ops.clone();
                obj.params = l.params.clone();
                obj.iops = l.iops.clone();
                obj.iops[4] = Some(
                    if kind == 1 {
                        rs910_core::texts::Msg::LentItemReturn
                    } else {
                        rs910_core::texts::Msg::BoughtItemDiscard
                    }
                    .get()
                    .into(),
                );
                obj.inventory.placeholder = false;
            }
        }
    }
    if obj.inventory.dummy != 0 {
        obj.inventory.tradeable = false;
    }
    visiting.remove(&id);
    out.insert(id, obj.clone());
    Ok(obj)
}

/// All obj configs, keyed by id, with derived items resolved.
#[derive(Clone, Debug, Default)]
pub struct ObjStore {
    entries: BTreeMap<u32, Obj>,
    /// Whether members-only content applies.
    pub allow_members: AllowMembers,
}

impl ObjStore {
    /// Decode every file of the obj archive (`id = group << 8 | file`) and
    /// resolve the derived items.
    pub fn load(pack: &Pack) -> Result<Self> {
        Self::resolve(load_all(pack, OBJ_ARCHIVE, OBJ_GROUP_BITS, decode_obj)?)
    }

    /// [`Self::load`] over the obj archive already read (ids as keys).
    pub fn from_records(files: &BTreeMap<i32, Vec<u8>>) -> Result<Self> {
        Self::resolve(decode_records(OBJ_ARCHIVE, files, decode_obj)?)
    }

    /// Resolve the derived items of the decoded objs.
    fn resolve(raw: BTreeMap<u32, Obj>) -> Result<Self> {
        let mut entries = BTreeMap::new();
        for &id in raw.keys() {
            resolve_obj(id, &raw, &mut entries, &mut BTreeSet::new())?;
        }
        Ok(Self {
            entries,
            allow_members: AllowMembers::default(),
        })
    }

    /// A store over decoded objects, so lifecycle tests can inject one
    /// without a cache pack.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn from_map(entries: BTreeMap<u32, Obj>) -> Self {
        Self {
            entries,
            allow_members: AllowMembers::default(),
        }
    }

    /// Look up one obj by id.
    pub fn get(&self, id: u32) -> Option<&Obj> {
        self.entries.get(&id)
    }

    /// Number of decoded objs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store holds no objs.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(id, obj)` in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&u32, &Obj)> {
        self.entries.iter()
    }
}

impl Obj {
    /// The item as a free world shows it: a members item loses its team,
    /// takes the default operations, is neither stock-market nor tradeable,
    /// drops its quest list and every parameter its type marks as
    /// auto-disabled. `autodisable(key)` answers for one parameter key.
    pub fn members_gated(
        &self,
        allow_members: bool,
        autodisable: &dyn Fn(i32) -> bool,
    ) -> Cow<'_, Obj> {
        if allow_members || !self.members {
            return Cow::Borrowed(self);
        }
        let mut obj = self.clone();
        obj.ops = obj_default_ops();
        obj.iops = obj_default_iops();
        obj.inventory.stockmarket = false;
        obj.inventory.quests.clear();
        obj.inventory.tradeable = false;
        obj.params.retain(|(key, _)| !autodisable(*key));
        Cow::Owned(obj)
    }
}
