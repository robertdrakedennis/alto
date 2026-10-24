//! 910 script-adjacent config codecs: `enum`, `struct` and `param`.
//!
//! 910-only: there are no build-version branches anywhere in this module. Every
//! layout below is the single 910 shape the client decoders describe; anything
//! else in the bytes is invalid data, rejected loudly rather than skipped or
//! defaulted. String bytes are Windows-1252 via [`Packet::gjstr`], exactly like
//! the client's `gjstr`.
//!
//! Pack map (paths relative to the alto repo root; group math follows the
//! client's config group table):
//! - `enum` ([`EnumConfig`]): `server/data/pack/client.enum.config.js5`. A dedicated
//!   archive holding only enums; entry id is `group << 8 | file` (256 files per
//!   group, `Js5ConfigGroup(8, 8)`; 62 groups, 15,813 entries).
//! - `struct` ([`StructConfig`]): `server/data/pack/client.struct.config.js5`.
//!   Entry id is `group << 5 | file` (32 files per group,
//!   `Js5ConfigGroup(26, 5)`; 1,385 groups, 44,223 entries).
//! - `param` ([`ParamConfig`]): `server/data/pack/client.config.js5`, group 11,
//!   entry id = file id (`Js5ConfigGroup(11)` carries no group bits; 8,060
//!   entries).
//! - `var` ([`VarConfig`]): `server/data/pack/client.config.js5`, one group per
//!   [`VarScope`] (see [`var_group_id`]): player 60, npc 61, client 62, world
//!   63, region 64, object 65, clan 66, clan-setting 67, controller 68, global
//!   75, player-group 80. Entry id = file id within the domain's group.
//! - `varbit` ([`VarBitConfig`]): `server/data/pack/client.config.js5`, group 69
//!   (`Js5ConfigGroup(69)`), entry id = file id (45,601 entries).
//!
//! Canonical opcode order emitted by `encode_*`. This matches the order the
//! 910 packer wrote and is verified byte-identical over the full corpus, so
//! decode accepts opcodes in any order but encode always normalizes here:
//! - enum: legacy input (1), legacy output (2), smart input (101), smart output
//!   (102), one values block (5/6/7/8), string default (3), int default (4).
//! - struct: params block (249) when non-empty.
//! - param: autodisable-clear marker (4) when `autodisable` is false, legacy
//!   kind (1), smart kind (101), int default (2), string default (5).
//! - var: debug name (1), data type (3), lifetime (4), transmit level (5),
//!   varname-hash marker (6), legacy-default-off marker (7), player client
//!   code (110, player domain only).
//! - varbit: base var (1), bit range (2).
//!
//! Accepted var grammar (anything else is a hard error, even where the client
//! merely ignores it — no corpus entry uses anything else, so acceptance would
//! only add untestable surface):
//! - var opcode 2 (`DOMAIN`) crashes the client (its switch has no arm for
//!   it); player opcodes 100-109/111-117 and any other byte are client-ignored
//!   no-op flags with no payload and no storage.
//! - varbit serials 3-15 are likewise client-ignored no-op flags; serial 0 and
//!   serials above 15 would crash the client (null key switch).
//!
//! Deliberately deferred:
//! - `dbtable`/`dbrow` (`client.config.js5` groups 40/41): out of scope for M3
//!   unless trivial; their column/row value encoding needs its own survey.

use crate::error::{NativeError, Result};
use crate::packet::{ByteWriter, Packet};
use crate::vars::VarScope;
use std::collections::BTreeMap;
use std::path::Path;

/// A scalar config value: 32-bit int or NUL-terminated string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigValue {
    /// 32-bit signed int (`g4s`).
    Int(i32),
    /// Windows-1252 string (`gjstr`).
    Str(String),
}

/// One sparse enum row: an explicit `key` (`g4s`) plus its value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumRow {
    /// Row key.
    pub key: i32,
    /// Row value.
    pub value: ConfigValue,
}

/// One dense enum slot: an array `index` (`g2`) plus its value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumSlot {
    /// Slot index into the dense array.
    pub index: u16,
    /// Slot value.
    pub value: ConfigValue,
}

/// An enum's key-to-value table, rows kept in file order.
///
/// Rows are a list, never a map: real entries repeat keys (enum 1547 repeats
/// sparse keys, enums 688 and 3907 repeat a dense index), and the client keeps
/// the last duplicate while the bytes keep them all — only the ordered list
/// re-encodes byte-identical.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnumValues {
    /// Sparse string rows, opcode 5 (`count g2`, then `key g4s, value gjstr`).
    SparseString(Vec<EnumRow>),
    /// Sparse int rows, opcode 6 (`count g2`, then `key g4s, value g4s`).
    SparseInt(Vec<EnumRow>),
    /// Dense string slots, opcode 7 (`capacity g2, count g2`, then
    /// `index g2, value gjstr`).
    DenseString {
        /// Declared array capacity, kept verbatim: entries 688 and 3907
        /// declare fewer slots than they fill (a duplicate index reuses one),
        /// so this is data, not a bound to normalize.
        capacity: u16,
        /// Slots in file order.
        slots: Vec<EnumSlot>,
    },
    /// Dense int slots, opcode 8 (same layout with `g4s` values).
    DenseInt {
        /// Declared array capacity, kept verbatim (see
        /// [`EnumValues::DenseString`]).
        capacity: u16,
        /// Slots in file order.
        slots: Vec<EnumSlot>,
    },
}

/// A decoded 910 enum: an id-to-value map whose
/// values are all ints or all strings.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EnumConfig {
    /// Legacy char input type, opcode 1 (raw byte; the client reads it signed
    /// for its Cp1252 mapping — only 2 of 15,813 corpus entries use this form).
    pub input_legacy: Option<u8>,
    /// Legacy char output type, opcode 2.
    pub output_legacy: Option<u8>,
    /// Smart input type id, opcode 101 (`gSmart1or2`).
    pub input_type: Option<u16>,
    /// Smart output type id, opcode 102 (`gSmart1or2`).
    pub output_type: Option<u16>,
    /// The key-to-value table, opcodes 5/6/7/8 (at most one block per entry).
    pub values: Option<EnumValues>,
    /// String default, opcode 3.
    pub default_string: Option<String>,
    /// Int default, opcode 4.
    pub default_int: Option<i32>,
}

/// One struct row: the `param` id (`g3`) plus its int/string value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructRow {
    /// Param id.
    pub param: u32,
    /// Row value.
    pub value: ConfigValue,
}

/// A decoded 910 struct: a typed field map keyed
/// by param id, rows in file order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StructConfig {
    /// Field rows, opcode 249 (`count g1`, then per row `is_string g1`,
    /// `param g3`, value). Empty when the entry is a bare terminator (16,408
    /// of 44,223 corpus entries).
    pub params: Vec<StructRow>,
}

/// A decoded 910 param: a typed constant. Absent
/// fields are `None`, never synthesized: the client's fallbacks (0, null,
/// `autodisable = true`) live in the client, not in this model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParamConfig {
    /// Legacy char kind, opcode 1 (raw byte, as with enum legacy types; unused
    /// by all 8,060 corpus entries, which use opcode 101 or carry no kind).
    pub kind_legacy: Option<u8>,
    /// Smart kind id, opcode 101 (`gSmart1or2`).
    pub kind: Option<u16>,
    /// Int default, opcode 2.
    pub default_int: Option<i32>,
    /// String default, opcode 5.
    pub default_string: Option<String>,
    /// Opcode 4 clears this; the client defaults it to true.
    pub autodisable: bool,
}

impl Default for ParamConfig {
    /// A bare terminator: no kind, no defaults, autodisable on.
    fn default() -> Self {
        Self {
            kind_legacy: None,
            kind: None,
            default_int: None,
            default_string: None,
            autodisable: true,
        }
    }
}

/// `gSmart1or2`: one byte below 128, else two bytes minus `0x8000`.
///
/// Private: [`Packet`] exposes no peek, so the marker byte is read and then
/// re-read after rewinding with `set_pos` (which cannot fail for a position
/// just observed).
fn g_smart1or2(packet: &mut Packet<'_>) -> Result<u16> {
    let pos = packet.pos();
    let marker = packet.g1()?;
    packet.set_pos(pos)?;
    if marker < 128 {
        Ok(u16::from(packet.g1()?))
    } else {
        Ok(packet.g2()?.saturating_sub(0x8000))
    }
}

/// Inverse of [`g_smart1or2`]: values below 128 write one byte, larger values
/// below `0x8000` write two bytes with the high bit set.
fn p_smart1or2(writer: &mut ByteWriter, value: u16) -> Result<()> {
    if value < 128 {
        writer.p1(value as u8);
        Ok(())
    } else if value < 0x8000 {
        writer.p2(value | 0x8000);
        Ok(())
    } else {
        Err(NativeError::Invalid(format!(
            "smart1or2 value {value} needs more than 15 bits"
        )))
    }
}

/// Reject bytes after the opcode-0 terminator: a well-formed entry ends
/// exactly at its terminator, so anything past it is corrupt input, not data.
fn reject_trailing(packet: &Packet<'_>, what: &str) -> Result<()> {
    if packet.is_empty() {
        Ok(())
    } else {
        Err(NativeError::Invalid(format!(
            "{what} has {} trailing byte(s) after its terminator",
            packet.remaining()
        )))
    }
}

/// Decode one values block (opcodes 5/6/7/8) into `out`. A second block is a
/// hard error: the client rebuilds its table per block, discarding the first,
/// so merging blocks would silently corrupt the model.
fn decode_enum_values(packet: &mut Packet<'_>, opcode: u8, out: &mut EnumConfig) -> Result<()> {
    if out.values.is_some() {
        return Err(NativeError::Invalid(format!(
            "enum has a second values block (opcode {opcode})"
        )));
    }
    let block = match opcode {
        5 => EnumValues::SparseString(decode_sparse_rows(packet, true)?),
        6 => EnumValues::SparseInt(decode_sparse_rows(packet, false)?),
        7 => {
            let (capacity, slots) = decode_dense_slots(packet, true)?;
            EnumValues::DenseString { capacity, slots }
        }
        8 => {
            let (capacity, slots) = decode_dense_slots(packet, false)?;
            EnumValues::DenseInt { capacity, slots }
        }
        _ => {
            return Err(NativeError::Invalid(format!(
                "unknown enum opcode {opcode}"
            )));
        }
    };
    out.values = Some(block);
    Ok(())
}

/// Decode a sparse block body: `count g2`, then `key g4s` plus a string or int
/// value per row.
fn decode_sparse_rows(packet: &mut Packet<'_>, strings: bool) -> Result<Vec<EnumRow>> {
    let count = usize::from(packet.g2()?);
    // A corrupt count can declare millions of rows; cap the pre-size (not the
    // decode) so the loop below still fails honestly on truncation.
    let mut rows = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let key = packet.g4s()?;
        let value = if strings {
            ConfigValue::Str(packet.gjstr()?)
        } else {
            ConfigValue::Int(packet.g4s()?)
        };
        rows.push(EnumRow { key, value });
    }
    Ok(rows)
}

/// Decode a dense block body: `capacity g2, count g2`, then `index g2` plus a
/// string or int value per slot. An index outside the capacity would crash the
/// client on load, so it is invalid data, not a slot we carry.
fn decode_dense_slots(packet: &mut Packet<'_>, strings: bool) -> Result<(u16, Vec<EnumSlot>)> {
    let capacity = packet.g2()?;
    let count = usize::from(packet.g2()?);
    let mut slots = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let index = packet.g2()?;
        if index >= capacity {
            return Err(NativeError::Invalid(format!(
                "dense enum slot index {index} outside capacity {capacity}"
            )));
        }
        let value = if strings {
            ConfigValue::Str(packet.gjstr()?)
        } else {
            ConfigValue::Int(packet.g4s()?)
        };
        slots.push(EnumSlot { index, value });
    }
    Ok((capacity, slots))
}

/// Decode one 910 enum entry's raw bytes.
pub fn decode_enum(data: &[u8]) -> Result<EnumConfig> {
    let mut packet = Packet::new(data);
    let mut out = EnumConfig::default();
    loop {
        match packet.g1()? {
            0 => {
                reject_trailing(&packet, "enum")?;
                return Ok(out);
            }
            1 => out.input_legacy = Some(packet.g1()?),
            2 => out.output_legacy = Some(packet.g1()?),
            3 => out.default_string = Some(packet.gjstr()?),
            4 => out.default_int = Some(packet.g4s()?),
            opcode @ (5..=8) => decode_enum_values(&mut packet, opcode, &mut out)?,
            101 => out.input_type = Some(g_smart1or2(&mut packet)?),
            102 => out.output_type = Some(g_smart1or2(&mut packet)?),
            other => {
                return Err(NativeError::Invalid(format!("unknown enum opcode {other}")));
            }
        }
    }
}

/// Encode one values block, checking every row carries the block's kind: a
/// `Str` in an int block (or vice versa) has no byte form and is rejected
/// rather than coerced.
fn encode_enum_values(writer: &mut ByteWriter, values: &EnumValues) -> Result<()> {
    match values {
        EnumValues::SparseString(rows) => {
            writer.p1(5);
            encode_sparse_rows(writer, rows, true)?;
        }
        EnumValues::SparseInt(rows) => {
            writer.p1(6);
            encode_sparse_rows(writer, rows, false)?;
        }
        EnumValues::DenseString { capacity, slots } => {
            writer.p1(7);
            writer.p2(*capacity);
            encode_dense_slots(writer, *capacity, slots, true)?;
        }
        EnumValues::DenseInt { capacity, slots } => {
            writer.p1(8);
            writer.p2(*capacity);
            encode_dense_slots(writer, *capacity, slots, false)?;
        }
    }
    Ok(())
}

/// Encode a sparse block body, rejecting row/value-kind mismatches.
fn encode_sparse_rows(writer: &mut ByteWriter, rows: &[EnumRow], strings: bool) -> Result<()> {
    let count = u16::try_from(rows.len()).map_err(|_| {
        NativeError::Invalid(format!(
            "enum holds {} sparse rows, a block carries at most 65535",
            rows.len()
        ))
    })?;
    writer.p2(count);
    for row in rows {
        writer.p4s(row.key);
        match &row.value {
            ConfigValue::Str(text) if strings => writer.pjstr(text)?,
            ConfigValue::Int(number) if !strings => writer.p4s(*number),
            ConfigValue::Str(_) => {
                return Err(NativeError::Invalid(
                    "string row in an int enum block".to_string(),
                ));
            }
            ConfigValue::Int(_) => {
                return Err(NativeError::Invalid(
                    "int row in a string enum block".to_string(),
                ));
            }
        }
    }
    Ok(())
}

/// Encode a dense block body, rejecting slot/value-kind mismatches and slots
/// outside the declared capacity.
fn encode_dense_slots(
    writer: &mut ByteWriter,
    capacity: u16,
    slots: &[EnumSlot],
    strings: bool,
) -> Result<()> {
    let count = u16::try_from(slots.len()).map_err(|_| {
        NativeError::Invalid(format!(
            "enum holds {} dense slots, a block carries at most 65535",
            slots.len()
        ))
    })?;
    writer.p2(count);
    for slot in slots {
        if slot.index >= capacity {
            return Err(NativeError::Invalid(format!(
                "dense enum slot index {} outside capacity {capacity}",
                slot.index
            )));
        }
        writer.p2(slot.index);
        match &slot.value {
            ConfigValue::Str(text) if strings => writer.pjstr(text)?,
            ConfigValue::Int(number) if !strings => writer.p4s(*number),
            ConfigValue::Str(_) => {
                return Err(NativeError::Invalid(
                    "string slot in an int enum block".to_string(),
                ));
            }
            ConfigValue::Int(_) => {
                return Err(NativeError::Invalid(
                    "int slot in a string enum block".to_string(),
                ));
            }
        }
    }
    Ok(())
}

/// Encode an [`EnumConfig`] back to 910 binary in canonical opcode order (see
/// the module docs): decode accepts any order, encode always normalizes here,
/// which is byte-identical for every corpus entry.
pub fn encode_enum(value: &EnumConfig) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::default();
    if let Some(legacy) = value.input_legacy {
        writer.p1(1);
        writer.p1(legacy);
    }
    if let Some(legacy) = value.output_legacy {
        writer.p1(2);
        writer.p1(legacy);
    }
    if let Some(type_id) = value.input_type {
        writer.p1(101);
        p_smart1or2(&mut writer, type_id)?;
    }
    if let Some(type_id) = value.output_type {
        writer.p1(102);
        p_smart1or2(&mut writer, type_id)?;
    }
    if let Some(values) = &value.values {
        encode_enum_values(&mut writer, values)?;
    }
    if let Some(default) = &value.default_string {
        writer.p1(3);
        writer.pjstr(default)?;
    }
    if let Some(default) = value.default_int {
        writer.p1(4);
        writer.p4s(default);
    }
    writer.p1(0);
    Ok(writer.data)
}

/// Decode one 910 struct entry's raw bytes. Opcode 249 is the only payload
/// opcode the client knows; a second 249 block is a hard error (see
/// [`decode_enum_values`] for why blocks never merge).
pub fn decode_struct(data: &[u8]) -> Result<StructConfig> {
    let mut packet = Packet::new(data);
    let mut params = Vec::new();
    let mut seen_params = false;
    loop {
        match packet.g1()? {
            0 => {
                reject_trailing(&packet, "struct")?;
                return Ok(StructConfig { params });
            }
            249 => {
                if seen_params {
                    return Err(NativeError::Invalid(
                        "struct has a second params block (opcode 249)".to_string(),
                    ));
                }
                seen_params = true;
                let count = usize::from(packet.g1()?);
                params = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    // The client tests `== 1`: any other flag byte reads an int.
                    let strings = packet.g1()? == 1;
                    let param = packet.g3()?;
                    let value = if strings {
                        ConfigValue::Str(packet.gjstr()?)
                    } else {
                        ConfigValue::Int(packet.g4s()?)
                    };
                    params.push(StructRow { param, value });
                }
            }
            other => {
                return Err(NativeError::Invalid(format!(
                    "unknown struct opcode {other}"
                )));
            }
        }
    }
}

/// Encode a [`StructConfig`] back to 910 binary: one 249 block when non-empty,
/// a bare terminator otherwise (both shapes occur in the corpus).
pub fn encode_struct(value: &StructConfig) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::default();
    if !value.params.is_empty() {
        let count = u8::try_from(value.params.len()).map_err(|_| {
            NativeError::Invalid(format!(
                "struct holds {} params, opcode 249 carries at most 255",
                value.params.len()
            ))
        })?;
        writer.p1(249);
        writer.p1(count);
        for row in &value.params {
            match &row.value {
                ConfigValue::Str(text) => {
                    writer.p1(1);
                    writer.p3(row.param)?;
                    writer.pjstr(text)?;
                }
                ConfigValue::Int(number) => {
                    writer.p1(0);
                    writer.p3(row.param)?;
                    writer.p4s(*number);
                }
            }
        }
    }
    writer.p1(0);
    Ok(writer.data)
}

/// Decode one 910 param entry's raw bytes.
pub fn decode_param(data: &[u8]) -> Result<ParamConfig> {
    let mut packet = Packet::new(data);
    let mut out = ParamConfig::default();
    loop {
        match packet.g1()? {
            0 => {
                reject_trailing(&packet, "param")?;
                return Ok(out);
            }
            1 => out.kind_legacy = Some(packet.g1()?),
            2 => out.default_int = Some(packet.g4s()?),
            4 => out.autodisable = false,
            5 => out.default_string = Some(packet.gjstr()?),
            101 => out.kind = Some(g_smart1or2(&mut packet)?),
            other => {
                return Err(NativeError::Invalid(format!(
                    "unknown param opcode {other}"
                )));
            }
        }
    }
}

/// Encode a [`ParamConfig`] back to 910 binary in canonical opcode order (see
/// the module docs): the autodisable-clear marker first, then kind, then
/// defaults — the order the 910 corpus uses.
pub fn encode_param(value: &ParamConfig) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::default();
    if !value.autodisable {
        writer.p1(4);
    }
    if let Some(legacy) = value.kind_legacy {
        writer.p1(1);
        writer.p1(legacy);
    }
    if let Some(kind) = value.kind {
        writer.p1(101);
        p_smart1or2(&mut writer, kind)?;
    }
    if let Some(default) = value.default_int {
        writer.p1(2);
        writer.p4s(default);
    }
    if let Some(default) = &value.default_string {
        writer.p1(5);
        writer.pjstr(default)?;
    }
    writer.p1(0);
    Ok(writer.data)
}

/// The `client.config.js5` group holding a [`VarScope`]'s var entries
/// (the client's per-scope config group).
pub fn var_group_id(domain: VarScope) -> u32 {
    match domain {
        VarScope::Player => 60,
        VarScope::Npc => 61,
        VarScope::Client => 62,
        VarScope::World => 63,
        VarScope::Region => 64,
        VarScope::Object => 65,
        VarScope::Clan => 66,
        VarScope::ClanSetting => 67,
        VarScope::Controller => 68,
        VarScope::Group => 80,
        VarScope::Global => 75,
    }
}

/// Resolve a `client.config.js5` group back to its var [`VarScope`]. Groups
/// outside the var family are invalid data for this codec, never a default.
pub fn var_domain_from_group(group: u32) -> Result<VarScope> {
    match group {
        60 => Ok(VarScope::Player),
        61 => Ok(VarScope::Npc),
        62 => Ok(VarScope::Client),
        63 => Ok(VarScope::World),
        64 => Ok(VarScope::Region),
        65 => Ok(VarScope::Object),
        66 => Ok(VarScope::Clan),
        67 => Ok(VarScope::ClanSetting),
        68 => Ok(VarScope::Controller),
        80 => Ok(VarScope::Group),
        75 => Ok(VarScope::Global),
        other => Err(NativeError::Invalid(format!(
            "config group {other} holds no var domain"
        ))),
    }
}

/// Storage width of a var value: what a later var read decodes (`g4s`, `g8`,
/// `gjstr2`, or a serializable-compound codec).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VarValueKind {
    /// 32-bit int storage (`g4s`).
    Int,
    /// 64-bit long storage (`g8`).
    Long,
    /// String storage (`gjstr2`).
    String,
    /// Compound storage (in 910: a fine coordinate; anything else a later table
    /// revision adds).
    Other,
}

/// Map a script var type serial (var opcode 3) to its [`VarValueKind`].
///
/// Transcribed from the client's script var type table: LONG
/// serials are 35, 49, 56, 71, 110, 115, 116 and 118; STRING is 36; COORDFINE
/// is 50; every other declared serial (0-51, 53-81, 83-129, 200-208) is
/// INTEGER. Undeclared serials (52, 82, 130-199, 209+) yield `None`: the entry
/// itself still round-trips (the serial is preserved raw), only this kind
/// query is partial. Kind is storage width, not semantics (`COORDGRID`, for
/// example, is int-stored).
pub fn var_value_kind(data_type: u8) -> Option<VarValueKind> {
    match data_type {
        35 | 49 | 56 | 71 | 110 | 115 | 116 | 118 => Some(VarValueKind::Long),
        36 => Some(VarValueKind::String),
        50 => Some(VarValueKind::Other),
        0..=51 | 53..=81 | 83..=129 | 200..=208 => Some(VarValueKind::Int),
        _ => None,
    }
}

/// A decoded 910 var (the basic var header, plus the player scope's client
/// code). The scope is not stored — the caller supplies it to
/// [`decode_var`]/[`encode_var`] to select the grammar, exactly like the
/// scope the client passes to its var decoder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VarConfig {
    /// Script var type serial, opcode 3 — the value-type hook read by
    /// [`var_value_kind`]. Every corpus entry carries one.
    pub data_type: Option<u8>,
    /// Lifetime id, opcode 4 (raw; the client resolves the lifetime).
    pub lifetime: Option<u8>,
    /// Transmit-level id, opcode 5 (raw; no corpus entry carries one).
    pub transmit_level: Option<u8>,
    /// Debug name, opcode 1 (`gjstr2`; no corpus entry carries one).
    pub debug_name: Option<String>,
    /// Opcode 6 marker (`VARNAME_HASH32`): the client reads no payload and
    /// stores nothing — presence-only, kept for byte-identity.
    pub varname_hash: bool,
    /// The client's `legacyDefaultValue` (default true); opcode 7 clears it.
    pub legacy_default_value: bool,
    /// Player-only client code, opcode 110 (`g2`).
    pub client_code: Option<u16>,
}

impl Default for VarConfig {
    /// A bare terminator: no type info, legacy defaulting on.
    fn default() -> Self {
        Self {
            data_type: None,
            lifetime: None,
            transmit_level: None,
            debug_name: None,
            varname_hash: false,
            legacy_default_value: true,
            client_code: None,
        }
    }
}

/// Whether `domain` uses the player grammar (opcode 110 reads `g2`).
fn is_player_domain(domain: VarScope) -> bool {
    matches!(domain, VarScope::Player)
}

/// Decode one 910 var entry's raw bytes under `domain`'s grammar: the shared
/// header for every domain, plus opcode 110 (`client_code`) for the player
/// domain. See the module docs for the accepted grammar; anything else is a
/// hard error.
pub fn decode_var(data: &[u8], domain: VarScope) -> Result<VarConfig> {
    let mut packet = Packet::new(data);
    let mut out = VarConfig::default();
    let player = is_player_domain(domain);
    loop {
        match packet.g1()? {
            0 => {
                reject_trailing(&packet, "var")?;
                return Ok(out);
            }
            1 => out.debug_name = Some(packet.gjstr2()?),
            // Opcode 2 (DOMAIN) has no arm in the client's switch: it crashes
            // the client, so it is invalid data here too.
            2 => {
                return Err(NativeError::Invalid(
                    "var opcode 2 (DOMAIN) is rejected by the client".to_string(),
                ));
            }
            3 => out.data_type = Some(packet.g1()?),
            4 => out.lifetime = Some(packet.g1()?),
            5 => out.transmit_level = Some(packet.g1()?),
            6 => out.varname_hash = true,
            7 => out.legacy_default_value = false,
            110 if player => out.client_code = Some(packet.g2()?),
            other => {
                return Err(NativeError::Invalid(format!("unknown var opcode {other}")));
            }
        }
    }
}

/// Encode a [`VarConfig`] back to 910 binary in ascending opcode order (see the
/// module docs). `domain` must be the decode domain: a `client_code` outside
/// the player domain has no byte form and is rejected rather than dropped.
pub fn encode_var(value: &VarConfig, domain: VarScope) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::default();
    if let Some(name) = &value.debug_name {
        writer.p1(1);
        writer.pjstr2(name)?;
    }
    if let Some(data_type) = value.data_type {
        writer.p1(3);
        writer.p1(data_type);
    }
    if let Some(lifetime) = value.lifetime {
        writer.p1(4);
        writer.p1(lifetime);
    }
    if let Some(level) = value.transmit_level {
        writer.p1(5);
        writer.p1(level);
    }
    if value.varname_hash {
        writer.p1(6);
    }
    if !value.legacy_default_value {
        writer.p1(7);
    }
    if let Some(code) = value.client_code {
        if !is_player_domain(domain) {
            return Err(NativeError::Invalid(
                "var client code outside the player domain".to_string(),
            ));
        }
        writer.p1(110);
        writer.p2(code);
    }
    writer.p1(0);
    Ok(writer.data)
}

/// A varbit's base var reference, opcode 1: the base domain
/// (scope serial, stored raw) plus the base var id
/// (`gSmart2or4s`, where 32767 reads as null/-1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VarBitBase {
    /// Base var domain serial.
    pub domain: u8,
    /// Base var id.
    pub var: i32,
}

/// A varbit's bit range, opcode 2: the half-open slice the client masks with
/// `masklookup[end - start]`, stored raw and order-unchecked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VarBitRange {
    /// First bit.
    pub start: u8,
    /// End bit.
    pub end: u8,
}

/// A decoded 910 varbit (`VarBitConfig` in the client): a bit-slice into a base
/// var. `None` fields mean the opcode was absent (every corpus entry carries
/// both); they are never defaulted, only preserved.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VarBitConfig {
    /// Base var reference, opcode 1.
    pub base: Option<VarBitBase>,
    /// Bit range, opcode 2.
    pub bits: Option<VarBitRange>,
}

/// Inverse of [`Packet::gsmart2or4null`] (the client's `pSmart2or4`): -1
/// writes the null short 32767, values below 32767 write one short, larger
/// values write an int with the high bit set. Anything below -1 is invalid
/// data, rejected rather than wrapped.
fn p_smart2or4null(writer: &mut ByteWriter, value: i32) -> Result<()> {
    if value == -1 {
        writer.p2(32767);
    } else if (0..32767).contains(&value) {
        writer.p2(value as u16);
    } else if value >= 32767 {
        writer.p4s(value | i32::MIN);
    } else {
        return Err(NativeError::Invalid(format!(
            "smart2or4null value {value} is below the null floor -1"
        )));
    }
    Ok(())
}

/// Decode one 910 varbit entry's raw bytes. Only opcodes 1 and 2 carry state;
/// see the module docs for why the client's bare flag serials are hard errors
/// here.
pub fn decode_varbit(data: &[u8]) -> Result<VarBitConfig> {
    let mut packet = Packet::new(data);
    let mut out = VarBitConfig::default();
    loop {
        match packet.g1()? {
            0 => {
                reject_trailing(&packet, "varbit")?;
                return Ok(out);
            }
            1 => {
                let domain = packet.g1()?;
                let var = packet.gsmart2or4null()?;
                out.base = Some(VarBitBase { domain, var });
            }
            2 => {
                let start = packet.g1()?;
                let end = packet.g1()?;
                out.bits = Some(VarBitRange { start, end });
            }
            other => {
                return Err(NativeError::Invalid(format!(
                    "unknown varbit opcode {other}"
                )));
            }
        }
    }
}

/// Encode a [`VarBitConfig`] back to 910 binary: base var (1), then bit range
/// (2) — the order every corpus entry uses.
pub fn encode_varbit(value: &VarBitConfig) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::default();
    if let Some(base) = &value.base {
        writer.p1(1);
        writer.p1(base.domain);
        p_smart2or4null(&mut writer, base.var)?;
    }
    if let Some(bits) = &value.bits {
        writer.p1(2);
        writer.p1(bits.start);
        writer.p1(bits.end);
    }
    writer.p1(0);
    Ok(writer.data)
}

/// Config-backed lane resolver for the expression family (`struct_param`,
/// `oc_param` and siblings, `_enum`): the result type lives in config data,
/// not in the opcode.
///
/// Proof rows (each verified against the retail command handlers):
/// - `struct_param`: two int pops (struct id, param id), then the
///   [`ParamConfig`] string-type branch — a string push of the param's
///   default-aware string value when string, else an int push of its
///   default-aware int value. Fixed 2 int pops; push lane is config data.
/// - `oc_param`: identical shape over the obj config (two int pops, same
///   branch). Fixed 2 int pops; push lane is config data. The `nc_param`,
///   `lc_param` and `seq_param` siblings (npc, loc, seq) share the exact
///   shape and branch.
/// - `_enum`: four int pops (input-type id, output-type id, enum id, key), a
///   guard that the enum's input and output types match the first two pops,
///   then an output-type id of 36 (string) selects a string push of the
///   enum's string value for the key, else an int push of its int value.
///   Fixed 4 int pops; push lane is the enum's output type (which the guard
///   pins equal to the stack value on success).
/// - The param string test: the param's type serial equals 36 — null (no
///   kind) is int-lane, only serial 36 (`'s'`) is string-lane. Every other
///   serial, including all int-stored config types, takes the int branch.
///
/// `Effect::Unknown` in [`crate::effects`] stays: that contract has no config
/// access, so it claims nothing. This resolver supplies the lane the effect
/// table cannot — unknown or missing ids fail loudly here, never guessed.
///
/// String-ness needs only the `'s'` check, never a full script var type
/// table: the int branch is every non-36 serial plus null, and the legacy
/// byte form carries the same split (`0x73` is `'s'`; all 8,060 corpus params
/// use the smart form or carry no kind, and only 2 of 15,813 enums use the
/// legacy form).
#[derive(Clone, Debug, Default)]
pub struct ConfigTypes {
    vars: BTreeMap<(u8, u16), VarValueKind>,
    dbtables: BTreeMap<u32, crate::dbtable::DbTableType>,
    db_listall: BTreeMap<u32, bool>,
    /// Param id (file id in `client.config.js5` group 11) to decoded entry.
    params: BTreeMap<i32, ParamConfig>,
    /// Enum id (`group << 8 | file` in `client.enum.config.js5`) to entry.
    enums: BTreeMap<i32, EnumConfig>,
}

impl ConfigTypes {
    /// Empty resolver: every lookup fails loudly as unknown. Unit tests for
    /// non-config operators use this; config-operator typing/lowering/lifting
    /// with it fails with unknown-id errors, never a guessed lane.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build from already-decoded tables (tests hand-build the two real ids
    /// they need; the pack loader below fills the full corpus).
    #[must_use]
    pub fn from_parts(
        params: BTreeMap<i32, ParamConfig>,
        enums: BTreeMap<i32, EnumConfig>,
    ) -> Self {
        Self {
            params,
            enums,
            ..Self::default()
        }
    }

    /// Load the full param and enum tables from the runtime pack under
    /// `pack_root` (the same root the dump verbs read). Param ids are file
    /// ids in group 11; enum ids are `group << 8 | file`. A script pack entry
    /// that fails to decode is invalid data for this resolver, rejected
    /// rather than skipped.
    pub fn load(pack_root: &Path) -> Result<Self> {
        use crate::pack::PackArchive;
        let mut params = BTreeMap::new();
        let config_archive = PackArchive::open(&pack_root.join("client.config.js5"))?;
        let Some(files) = config_archive.group_files(11)? else {
            return Err(NativeError::Invalid(
                "config pack holds no param group 11".to_string(),
            ));
        };
        for (file, bytes) in &files {
            let id = i32::try_from(*file).map_err(|_| {
                NativeError::Invalid(format!("param file id {file} does not fit an i32"))
            })?;
            let entry = decode_param(bytes)
                .map_err(|error| NativeError::Invalid(format!("param {id}: {error}")))?;
            params.insert(id, entry);
        }
        let mut enums = BTreeMap::new();
        let enum_archive = PackArchive::open(&pack_root.join("client.enum.config.js5"))?;
        for group in enum_archive.group_ids() {
            let Some(files) = enum_archive.group_files(group)? else {
                continue;
            };
            for (file, bytes) in &files {
                let id = group
                    .checked_mul(256)
                    .and_then(|base| base.checked_add(*file))
                    .ok_or_else(|| {
                        NativeError::Invalid(format!("enum {group}/{file}: id overflow"))
                    })?;
                let id = i32::try_from(id).map_err(|_| {
                    NativeError::Invalid(format!("enum {group}/{file}: id does not fit an i32"))
                })?;
                let entry = decode_enum(bytes)
                    .map_err(|error| NativeError::Invalid(format!("enum {id}: {error}")))?;
                enums.insert(id, entry);
            }
        }
        let mut vars = BTreeMap::new();
        for domain_id in 0..=10 {
            let domain = VarScope::from_id(domain_id)?;
            if let Some(files) = config_archive.group_files(var_group_id(domain))? {
                for (id, bytes) in files {
                    let entry = decode_var(&bytes, domain)?;
                    if let (Ok(id), Some(kind)) =
                        (u16::try_from(id), entry.data_type.and_then(var_value_kind))
                    {
                        vars.insert((domain_id, id), kind);
                    }
                }
            }
        }
        let mut dbtables = BTreeMap::new();
        if let Some(files) = config_archive.group_files(40)? {
            for (id, bytes) in files {
                dbtables.insert(id, crate::dbtable::decode_dbtable(&bytes)?);
            }
        }
        let mut db_listall = BTreeMap::new();
        let indexes = pack_root.join("client.dbtableindex.js5");
        if indexes.is_file() {
            let archive = PackArchive::open(&indexes)?;
            for table in archive.group_ids() {
                if let Some(files) = archive.group_files(table)?
                    && let Some(bytes) = files.get(&0)
                {
                    let index = crate::dbtable::decode_dbindex(bytes)?;
                    db_listall.insert(table, index.lookup_int(0).is_some());
                }
            }
        }
        Ok(Self {
            vars,
            dbtables,
            db_listall,
            params,
            enums,
        })
    }

    /// Whether listall's key-zero lookup returns a list. An unknown table is
    /// resolvable only when every provisioned table agrees; invalid IDs throw.
    pub fn db_listall_pushes_count(&self, table: Option<i32>) -> Option<bool> {
        if let Some(table) = table {
            return self
                .db_listall
                .get(&u32::from_ne_bytes(table.to_ne_bytes()))
                .copied();
        }
        let first = *self.db_listall.values().next()?;
        self.db_listall
            .values()
            .all(|value| *value == first)
            .then_some(first)
    }

    /// Tuple schema selected using revision 910's unsigned table/column split.
    pub fn db_field_types(&self, field: i32) -> Option<&[u16]> {
        let field = u32::from_ne_bytes(field.to_ne_bytes());
        self.dbtables
            .get(&(field >> 8))?
            .columns
            .iter()
            .rev()
            .find(|column| u32::from(column.column) == field & 255)
            .map(|column| column.types.as_slice())
    }

    /// Declared storage lane; absent definitions and compound values stay unresolved.
    pub fn var_kind(&self, domain: VarScope, id: u16) -> Option<VarValueKind> {
        self.vars.get(&(u8::from(domain), id)).copied()
    }

    /// Whether param `id` is string-typed (`ParamConfig.isStringType`): smart
    /// kind 36 or legacy `'s'`; everything else — including no kind (null) —
    /// is int-lane. Unknown ids fail loudly.
    pub fn param_is_string(&self, id: i32) -> Result<bool> {
        let entry = self
            .params
            .get(&id)
            .ok_or_else(|| NativeError::Invalid(format!("unknown param id {id}")))?;
        if entry.kind == Some(36) {
            return Ok(true);
        }
        if entry.kind_legacy == Some(b's') {
            return Ok(true);
        }
        Ok(false)
    }

    /// Whether enum `id`'s output type is string-typed: smart output 36 or
    /// legacy `'s'`; any other present output is int-lane. Unknown ids and
    /// entries with no output type at all (neither smart nor legacy — 7,787
    /// corpus entries no `_enum` site references) fail loudly: there is no
    /// lane to read.
    pub fn enum_output_is_string(&self, id: i32) -> Result<bool> {
        let entry = self
            .enums
            .get(&id)
            .ok_or_else(|| NativeError::Invalid(format!("unknown enum id {id}")))?;
        if let Some(output) = entry.output_type {
            return Ok(output == 36);
        }
        if let Some(legacy) = entry.output_legacy {
            return Ok(legacy == b's');
        }
        Err(NativeError::Invalid(format!(
            "enum {id} carries no output type"
        )))
    }
}
