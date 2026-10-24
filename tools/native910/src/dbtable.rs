//! 910 `dbtable`/`dbrow` codecs: table schemas (column types) and row tuples.
//!
//! 910-only: there are no build-version branches anywhere in this module. Every
//! layout below is the single 910 shape the client decoders describe (the
//! dbtable and dbrow config decoders and the column-value unpacker);
//! anything else in the bytes is invalid data, rejected loudly rather than skipped or defaulted.
//! String bytes are Windows-1252 via [`Packet::gjstr`],
//! exactly like the client's `gjstr`.
//!
//! Pack map (paths relative to the alto repo root; group math follows the
//! client's config group table, which reads `getfile(group.id, entry)`
//! whenever the group carries no group bits):
//! - `dbtable`: `server/data/pack/client.config.js5`, group 40
//!   (`Js5ConfigGroup(40)` carries no group bits; 75 entries, file id 0..74).
//!   Entry id is the file id. 33 entries are bare terminators (no schema).
//! - `dbrow`: `server/data/pack/client.config.js5`, group 41
//!   (`Js5ConfigGroup(41)`; 2,399 entries, file id 0..2398). Entry id is the
//!   file id. 1,106 entries are table links only (opcode 4, no columns).
//! - `client.dbtableindex.js5` is a separate query-index archive (the client's
//!   table index: `getfile(table, column + 1)` / `getfile(id, 0)`, a base
//!   var type plus a varint key-to-rows map). It holds no schemas or row
//!   tuples, only the lookup structure the script opcodes query. `DbIndex`
//!   preserves key/row order and duplicate keys; lookup follows the client's last
//!   duplicate wins behavior and distinguishes absent keys from empty row lists.
//!
//! Canonical opcode order emitted by `encode_*`. Decode accepts opcodes in any
//! order, but encode always normalizes here, which is byte-identical over the
//! full corpus:
//! - dbtable: one schema block (1) with columns in file order, then the
//!   terminator; a table with no block encodes to a bare terminator. Column
//!   markers are deliberately NOT sorted: 9 tables (e.g. 5, 29, 72) store them
//!   out of order, so file order is data.
//! - dbrow: table link (4), then one columns block (3), then the terminator —
//!   every corpus entry with both blocks writes 4 before 3. A table-only row
//!   encodes to link plus terminator.
//!
//! Column/cell type inventory observed over the full corpus (script var type
//! serials; storage follows the base var type):
//! - int-stored (`g4s`): 0, 1, 9, 17, 23, 26, 30, 31, 32, 33, 39, 57, 73, 74.
//! - string-stored (`gjstr`): 36. Long-stored (35, 49, 56, 71, 110, 115, 116,
//!   118) and compound (50 `COORDFINE`) serials never occur in 910 db columns;
//!   the long arms still round-trip hand-built models, while compound or
//!   undeclared serials are hard errors (the client would crash on them too).
//!
//! Preserve-not-normalize discipline (anything the client distinguishes is
//! kept, following `config.rs`): duplicate columns stay an ordered list, the
//! column-count/size bytes are stored verbatim (never re-derived), default
//! presence is an `Option` (a present-but-empty default block differs from an
//! absent one), and every row tuple is kept — including the 373-tuple column
//! in dbrow 1283.
//!
//! Deliberately deferred:
//! - `client.dbtableindex.js5` decoding and any row-lookup/query API: bytes
//!   only (see the pack map above).

use crate::error::{NativeError, Result};
use crate::packet::{ByteWriter, Packet};

/// Storage width of one db cell: what a column tuple element decodes with
/// (`g4s`, `g8`, or `gjstr`), driven by the element's script var type serial.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DbCellBase {
    /// 32-bit int storage (`g4s`).
    Int,
    /// 64-bit long storage (`g8`).
    Long,
    /// String storage (`gjstr`).
    String,
}

/// Map a script var type serial to its [`DbCellBase`].
///
/// Transcribed from the client's script var type and base var type tables:
/// LONG serials are 35, 49, 56, 71, 110, 115, 116 and 118; STRING is 36; every
/// other declared serial (0-49, 51, 53-81, 83-129, 200-208) is INTEGER.
/// Serial 50 (`COORDFINE`, a 13-byte compound) has no scalar form here and
/// undeclared serials (52, 82, 130-199, 209+) name no client type at all, so
/// both yield `None`: the entry still fails loudly at the cell, never with a
/// guessed width.
pub fn db_cell_base(type_id: u16) -> Option<DbCellBase> {
    match type_id {
        36 => Some(DbCellBase::String),
        35 | 49 | 56 | 71 | 110 | 115 | 116 | 118 => Some(DbCellBase::Long),
        0..=49 | 51 | 53..=81 | 83..=129 | 200..=208 => Some(DbCellBase::Int),
        _ => None,
    }
}

/// One db cell: a single tuple element in int/long/string storage form.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DbCellValue {
    /// 32-bit signed int (`g4s`).
    Int(i32),
    /// 64-bit signed long (`g8`).
    Long(i64),
    /// Windows-1252 string (`gjstr`).
    Str(String),
}

/// One dbtable schema column, opcode 1: the masked column id, its tuple type
/// ids (`gSmart1or2` each), and its default tuple block when the marker's
/// high bit is set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbTableColumn {
    /// Column id: the marker byte masked with `0x7F`, in file order.
    pub column: u8,
    /// Tuple type ids, one `gSmart1or2` per element.
    pub types: Vec<u16>,
    /// Default tuples (`gSmart1or2` count, then that many tuples): `None`
    /// when the marker carries no high bit, `Some` (possibly empty) when it
    /// does — presence is data, never defaulted.
    pub defaults: Option<Vec<Vec<DbCellValue>>>,
}

/// A decoded 910 dbtable: a table schema.
/// Columns are a list in file order, never a map: markers repeat in theory
/// (the client keeps the last) and arrive unsorted in 9 tables, so only the
/// ordered list re-encodes byte-identical.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DbTableType {
    /// The opcode-1 column-count byte, kept verbatim: it sizes the client's
    /// column arrays, so it is data, not a bound to re-derive. `None` when
    /// the entry is a bare terminator (33 of 75 corpus entries).
    pub column_count: Option<u8>,
    /// Schema columns in file order.
    pub columns: Vec<DbTableColumn>,
}

/// One dbrow value column, opcode 3: the column id, its tuple type ids, and
/// its row tuples in file order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbRowColumn {
    /// Column id (raw marker byte; 255 terminates the block).
    pub column: u8,
    /// Tuple type ids, one `gSmart1or2` per element.
    pub types: Vec<u16>,
    /// Row tuples (`gSmart1or2` count, then that many tuples), file order.
    pub rows: Vec<Vec<DbCellValue>>,
}

/// A decoded 910 dbrow: a table link plus value
/// columns. Absent fields are `None`/empty, never synthesized.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DbRowType {
    /// Owning table id, opcode 4 (`gVarInt2`).
    pub table: Option<u32>,
    /// The opcode-3 array-size byte, kept verbatim (see
    /// [`DbTableType::column_count`]). `None` for table-only rows (1,106 of
    /// 2,399 corpus entries).
    pub column_count: Option<u8>,
    /// Value columns in file order.
    pub columns: Vec<DbRowColumn>,
}

/// `gSmart1or2`: one byte below 128, else two bytes minus `0x8000`.
///
/// Private: [`Packet`] exposes no peek, so the marker byte is read and then
/// re-read after rewinding with `set_pos` (which cannot fail for a position
/// just observed). Same idiom as `config.rs`.
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

/// `gVarInt2`: little-endian 7-bit groups, low bit group first (the client's
/// `gVarInt2`). More than 5 groups cannot fit a `u32` and are corrupt input.
fn g_varint2(packet: &mut Packet<'_>) -> Result<u32> {
    let mut value = 0_u32;
    let mut shift = 0_u32;
    loop {
        if shift >= 32 {
            return Err(NativeError::Invalid(
                "varint2 value needs more than 32 bits".to_string(),
            ));
        }
        let byte = u32::from(packet.g1()?);
        value |= (byte & 0x7F) << shift;
        if byte <= 0x7F {
            return Ok(value);
        }
        shift += 7;
    }
}

/// Inverse of [`g_varint2`]: minimal little-endian 7-bit groups, the form
/// every corpus entry uses.
fn p_varint2(writer: &mut ByteWriter, mut value: u32) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            writer.p1(byte);
            return;
        }
        writer.p1(byte | 0x80);
    }
}

/// Reject bytes after the opcode-0 terminator: a well-formed entry ends
/// exactly at its terminator, so anything past it is corrupt input, not data.
/// Same idiom as `config.rs`.
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

/// Decode one tuple body: one cell per entry of `types`, each in its type's
/// storage width. An unmapped type id is a hard error (see [`db_cell_base`]).
fn decode_tuple(packet: &mut Packet<'_>, types: &[u16]) -> Result<Vec<DbCellValue>> {
    let mut cells = Vec::with_capacity(types.len());
    for type_id in types {
        let cell = match db_cell_base(*type_id) {
            Some(DbCellBase::Int) => DbCellValue::Int(packet.g4s()?),
            Some(DbCellBase::Long) => DbCellValue::Long(packet.g8s()?),
            Some(DbCellBase::String) => DbCellValue::Str(packet.gjstr()?),
            None => {
                return Err(NativeError::Invalid(format!(
                    "db column type {type_id} has no scalar storage form"
                )));
            }
        };
        cells.push(cell);
    }
    Ok(cells)
}

/// Encode one tuple body, checking arity and kind per element: a `Str` in an
/// int slot (or any other mismatch) has no byte form and is rejected rather
/// than coerced. Same discipline as the enum block encoders in `config.rs`.
fn encode_tuple(writer: &mut ByteWriter, types: &[u16], cells: &[DbCellValue]) -> Result<()> {
    if cells.len() != types.len() {
        return Err(NativeError::Invalid(format!(
            "db tuple holds {} cells for {} column types",
            cells.len(),
            types.len()
        )));
    }
    for (type_id, cell) in types.iter().zip(cells.iter()) {
        match (db_cell_base(*type_id), cell) {
            (Some(DbCellBase::Int), DbCellValue::Int(number)) => writer.p4s(*number),
            (Some(DbCellBase::Long), DbCellValue::Long(number)) => writer.p8s(*number),
            (Some(DbCellBase::String), DbCellValue::Str(text)) => writer.pjstr(text)?,
            (None, _) => {
                return Err(NativeError::Invalid(format!(
                    "db column type {type_id} has no scalar storage form"
                )));
            }
            _ => {
                return Err(NativeError::Invalid(format!(
                    "db cell kind mismatches column type {type_id}"
                )));
            }
        }
    }
    Ok(())
}

/// Decode a `gSmart1or2` tuple-counted row list: `count`, then that many
/// [`decode_tuple`] bodies. A corrupt count can declare thousands of rows;
/// cap the pre-size (not the decode) so the loop below still fails honestly
/// on truncation. Same idiom as `config.rs`.
fn decode_tuples(packet: &mut Packet<'_>, types: &[u16]) -> Result<Vec<Vec<DbCellValue>>> {
    let count = usize::from(g_smart1or2(packet)?);
    let mut tuples = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        tuples.push(decode_tuple(packet, types)?);
    }
    Ok(tuples)
}

/// Encode a tuple list: minimal `gSmart1or2` count, then the bodies.
fn encode_tuples(
    writer: &mut ByteWriter,
    types: &[u16],
    tuples: &[Vec<DbCellValue>],
) -> Result<()> {
    let count = u16::try_from(tuples.len()).map_err(|_| {
        NativeError::Invalid(format!(
            "db column holds {} tuples, a block carries at most 32767",
            tuples.len()
        ))
    })?;
    p_smart1or2(writer, count)?;
    for tuple in tuples {
        encode_tuple(writer, types, tuple)?;
    }
    Ok(())
}

/// Decode the type-id header of one column: `length g1`, then that many
/// `gSmart1or2` ids.
fn decode_type_ids(packet: &mut Packet<'_>) -> Result<Vec<u16>> {
    let length = usize::from(packet.g1()?);
    let mut types = Vec::with_capacity(length);
    for _ in 0..length {
        types.push(g_smart1or2(packet)?);
    }
    Ok(types)
}

/// Encode a type-id header: the length byte must fit `g1`.
fn encode_type_ids(writer: &mut ByteWriter, types: &[u16]) -> Result<()> {
    let length = u8::try_from(types.len()).map_err(|_| {
        NativeError::Invalid(format!(
            "db column holds {} types, a header carries at most 255",
            types.len()
        ))
    })?;
    writer.p1(length);
    for type_id in types {
        p_smart1or2(writer, *type_id)?;
    }
    Ok(())
}

/// Check a column id against its block's count byte: the client indexes a
/// fixed array with it, so an id outside the count would crash the client on
/// load — invalid data, not a column we carry. Same discipline as the dense
/// enum slots in `config.rs`.
fn check_column_id(column: u8, count: u8, what: &str) -> Result<()> {
    if column >= count {
        return Err(NativeError::Invalid(format!(
            "{what} column {column} outside column count {count}"
        )));
    }
    Ok(())
}

/// Decode one 910 dbtable entry's raw bytes (opcode 1 only; a
/// second schema block is a hard error, mirroring the second-values-block
/// discipline in `config.rs`).
pub fn decode_dbtable(data: &[u8]) -> Result<DbTableType> {
    let mut packet = Packet::new(data);
    let mut column_count = None;
    let mut columns = Vec::new();
    let mut seen_schema = false;
    loop {
        match packet.g1()? {
            0 => {
                reject_trailing(&packet, "dbtable")?;
                return Ok(DbTableType {
                    column_count,
                    columns,
                });
            }
            1 => {
                if seen_schema {
                    return Err(NativeError::Invalid(
                        "dbtable has a second schema block (opcode 1)".to_string(),
                    ));
                }
                seen_schema = true;
                let count = packet.g1()?;
                column_count = Some(count);
                loop {
                    let marker = packet.g1()?;
                    if marker == u8::MAX {
                        break;
                    }
                    let column = marker & 0x7F;
                    check_column_id(column, count, "dbtable")?;
                    let has_default = marker & 0x80 != 0;
                    let types = decode_type_ids(&mut packet)?;
                    let defaults = if has_default {
                        Some(decode_tuples(&mut packet, &types)?)
                    } else {
                        None
                    };
                    columns.push(DbTableColumn {
                        column,
                        types,
                        defaults,
                    });
                }
            }
            other => {
                return Err(NativeError::Invalid(format!(
                    "unknown dbtable opcode {other}"
                )));
            }
        }
    }
}

/// Encode a [`DbTableType`] back to 910 binary in canonical opcode order (see
/// the module docs): one schema block with columns in stored file order, then
/// the terminator — a bare terminator when there is no block. A column list
/// without a count byte has no byte form and is rejected rather than guessed.
pub fn encode_dbtable(value: &DbTableType) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::default();
    if let Some(count) = value.column_count {
        writer.p1(1);
        writer.p1(count);
        for column in &value.columns {
            if column.column > 0x7F {
                return Err(NativeError::Invalid(format!(
                    "dbtable column {} does not fit the 7-bit marker",
                    column.column
                )));
            }
            check_column_id(column.column, count, "dbtable")?;
            if column.column == 0x7F && column.defaults.is_some() {
                return Err(NativeError::Invalid(
                    "dbtable column 127 with a default is the 0xFF terminator".to_string(),
                ));
            }
            let mut marker = column.column;
            if column.defaults.is_some() {
                marker |= 0x80;
            }
            writer.p1(marker);
            encode_type_ids(&mut writer, &column.types)?;
            if let Some(defaults) = &column.defaults {
                encode_tuples(&mut writer, &column.types, defaults)?;
            }
        }
        writer.p1(u8::MAX);
    } else if !value.columns.is_empty() {
        return Err(NativeError::Invalid(
            "dbtable has columns but no column count".to_string(),
        ));
    }
    writer.p1(0);
    Ok(writer.data)
}

/// Decode one 910 dbrow entry's raw bytes (opcodes 3 and 4; a
/// second columns block or a second table link is a hard error, mirroring the
/// second-block discipline in `config.rs` and [`decode_dbtable`]).
pub fn decode_dbrow(data: &[u8]) -> Result<DbRowType> {
    let mut packet = Packet::new(data);
    let mut table = None;
    let mut column_count = None;
    let mut columns = Vec::new();
    let mut seen_columns = false;
    let mut seen_table = false;
    loop {
        match packet.g1()? {
            0 => {
                reject_trailing(&packet, "dbrow")?;
                return Ok(DbRowType {
                    table,
                    column_count,
                    columns,
                });
            }
            3 => {
                if seen_columns {
                    return Err(NativeError::Invalid(
                        "dbrow has a second columns block (opcode 3)".to_string(),
                    ));
                }
                seen_columns = true;
                let count = packet.g1()?;
                column_count = Some(count);
                loop {
                    let column = packet.g1()?;
                    if column == u8::MAX {
                        break;
                    }
                    check_column_id(column, count, "dbrow")?;
                    let types = decode_type_ids(&mut packet)?;
                    let rows = decode_tuples(&mut packet, &types)?;
                    columns.push(DbRowColumn {
                        column,
                        types,
                        rows,
                    });
                }
            }
            4 => {
                if seen_table {
                    return Err(NativeError::Invalid(
                        "dbrow has a second table link (opcode 4)".to_string(),
                    ));
                }
                seen_table = true;
                table = Some(g_varint2(&mut packet)?);
            }
            other => {
                return Err(NativeError::Invalid(format!(
                    "unknown dbrow opcode {other}"
                )));
            }
        }
    }
}

/// Encode a [`DbRowType`] back to 910 binary in canonical opcode order (see
/// the module docs): table link (4), then one columns block (3) with columns
/// in stored file order, then the terminator. A column list without a size
/// byte has no byte form and is rejected rather than guessed.
pub fn encode_dbrow(value: &DbRowType) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::default();
    if let Some(table) = value.table {
        writer.p1(4);
        p_varint2(&mut writer, table);
    }
    if let Some(count) = value.column_count {
        writer.p1(3);
        writer.p1(count);
        for column in &value.columns {
            check_column_id(column.column, count, "dbrow")?;
            writer.p1(column.column);
            encode_type_ids(&mut writer, &column.types)?;
            encode_tuples(&mut writer, &column.types, &column.rows)?;
        }
        writer.p1(u8::MAX);
    } else if !value.columns.is_empty() {
        return Err(NativeError::Invalid(
            "dbrow has columns but no column count".to_string(),
        ));
    }
    writer.p1(0);
    Ok(writer.data)
}

/// Query-index entries preserve file order and duplicate keys. Retail lookup
/// keeps the last duplicate, as a map insert does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbIndex {
    /// Base var type serial: integer 0, long 1, string 2.
    pub base: u8,
    pub entries: Vec<(DbCellValue, Vec<u32>)>,
}
impl DbIndex {
    pub fn lookup_int(&self, key: i32) -> Option<&[u32]> {
        self.entries
            .iter()
            .rev()
            .find_map(|(value, rows)| (*value == DbCellValue::Int(key)).then_some(rows.as_slice()))
    }
}

/// Query-index entry list: base type, varint entry count, typed key and
/// varint row list. Compound keys are unsupported and remain explicit errors.
pub fn decode_dbindex(bytes: &[u8]) -> Result<DbIndex> {
    let mut packet = Packet::new(bytes);
    let base = packet.g1()?;
    if base > 2 {
        return Err(NativeError::Invalid(format!(
            "unsupported DB index base {base}"
        )));
    }
    let count = g_varint2(&mut packet)? as usize;
    if count > packet.remaining() {
        return Err(NativeError::Invalid(
            "DB index entry count exceeds input".into(),
        ));
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let key = match base {
            0 => DbCellValue::Int(packet.g4s()?),
            1 => DbCellValue::Long(packet.g8s()?),
            _ => DbCellValue::Str(packet.gjstr()?),
        };
        let count = g_varint2(&mut packet)? as usize;
        if count > packet.remaining() {
            return Err(NativeError::Invalid(
                "DB index row count exceeds input".into(),
            ));
        }
        let mut rows = Vec::with_capacity(count);
        for _ in 0..count {
            rows.push(g_varint2(&mut packet)?);
        }
        entries.push((key, rows));
    }
    if !packet.is_empty() {
        return Err(NativeError::Invalid("DB index trailing bytes".into()));
    }
    Ok(DbIndex { base, entries })
}

pub fn encode_dbindex(index: &DbIndex) -> Result<Vec<u8>> {
    if index.base > 2 {
        return Err(NativeError::Invalid("unsupported DB index base".into()));
    }
    let mut writer = ByteWriter::default();
    writer.p1(index.base);
    p_varint2(
        &mut writer,
        u32::try_from(index.entries.len())
            .map_err(|_| NativeError::Invalid("DB index too large".into()))?,
    );
    for (key, rows) in &index.entries {
        match (index.base, key) {
            (0, DbCellValue::Int(value)) => writer.p4s(*value),
            (1, DbCellValue::Long(value)) => writer.p8s(*value),
            (2, DbCellValue::Str(value)) => writer.pjstr(value)?,
            _ => return Err(NativeError::Invalid("DB index key/base mismatch".into())),
        }
        p_varint2(
            &mut writer,
            u32::try_from(rows.len())
                .map_err(|_| NativeError::Invalid("DB row list too large".into()))?,
        );
        for row in rows {
            p_varint2(&mut writer, *row);
        }
    }
    Ok(writer.data)
}
