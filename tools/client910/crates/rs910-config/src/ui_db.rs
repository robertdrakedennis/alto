//! Database tables and rows (config groups 40 and 41) and the `db_getfield` /
//! `db_getfieldcount` commands. Column values are stored flat:
//! `columns * rows` entries, row-major.
//! The `dbtableindex` js5 archive (archive 49:
//! group = table, file 0 = list-all index, file `column + 1` = column index)
//! backs the finder commands `db_find`/`db_find_with_count`, `db_listall`,
//! `db_findnext`, `db_find_get`, `db_find_refine` and `db_getrowtable`,
//! which work over the shared [`Finder`] state.
use crate::{cache::Pack, ui_bytes::Cursor};
use anyhow::{anyhow, Context, Result};
use native910::vm::{Value, VmError, VmResult};
use rs910_core::fault::Fault;
use std::collections::BTreeMap;

/// A decoded database column value (integer, long, string or fine coordinate).
#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
    Int(i32),
    Long(i64),
    Str(String),
    /// Fine coordinate: level, x, y, z.
    FineCoord(u8, i32, i32, i32),
    /// A component hook (trigger and arguments): retained as a decoded opaque
    /// key. The finder commands supply integer keys, so no Rust query can
    /// construct this identity-based object; decoding it still matters for
    /// `db_listall`, which consumes the index's row lists regardless of key.
    HookRef {
        trigger: Option<i32>,
        args: Vec<ComponentHookArg>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ComponentHookArg {
    Int(i32),
    Str(String),
}

/// Script var type serial ids, each read with `gSmart1or2`.
pub type Types = Vec<u8>;

/// Unpack a column's values: a row count, then one value per column and row.
fn unpack_values(c: &mut Cursor<'_>, types: &[u8]) -> Result<Vec<DbValue>> {
    let rows = c.gsmart1or2()?;
    anyhow::ensure!(rows >= 0, "negative DB row count");
    let mut values = Vec::with_capacity(types.len() * rows as usize);
    for _ in 0..rows {
        for &t in types {
            // The codec follows the script var type's base type.
            let base = crate::types910::script_types::script_type(t)
                .ok_or_else(|| anyhow!("script var type serial {t} has no base type"))?
                .0;
            values.push(match base {
                0 => DbValue::Int(c.g4s()?),
                1 => DbValue::Long(c.g8()?),
                2 => DbValue::Str(c.gjstr()?),
                3 => DbValue::FineCoord(c.g1()?, c.g4s()?, c.g4s()?, c.g4s()?),
                other => anyhow::bail!("DB column base type {other} has no codec"),
            });
        }
    }
    Ok(values)
}

fn read_types(c: &mut Cursor<'_>, count: usize) -> Result<Types> {
    let mut types = Vec::with_capacity(count);
    for _ in 0..count {
        let id = c.gsmart1or2()?;
        let id = u8::try_from(id)
            .ok()
            .filter(|id| crate::types910::script_types::script_type(*id).is_some())
            .ok_or_else(|| anyhow!("script var type serial {id} is not serializable"))?;
        types.push(id);
    }
    Ok(types)
}

/// One table index: decodes the key type (base var type
/// serial: 0 int, 1 long, 2 string, 3 coordfine, 4 component hook), then
/// `gVarInt2` entries of key → `gVarInt2` row ids. Kept in file order; lookup
/// is a hash lookup, so only a key of the same type can match.
#[derive(Clone, Debug, PartialEq)]
pub struct Index {
    pub key_serial: u8,
    pub entries: Vec<(DbValue, Vec<i32>)>,
}
impl Index {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let key_serial = c.g1()?;
        let count = c.gvarint2()?;
        anyhow::ensure!(count >= 0, "negative DB index size");
        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let key = match key_serial {
                0 => DbValue::Int(c.g4s()?),
                1 => DbValue::Long(c.g8()?),
                2 => DbValue::Str(c.gjstr()?),
                3 => DbValue::FineCoord(c.g1()?, c.g4s()?, c.g4s()?, c.g4s()?),
                4 => {
                    let count = c.g1()?;
                    if count == 0 {
                        DbValue::HookRef {
                            trigger: None,
                            args: Vec::new(),
                        }
                    } else {
                        let args = usize::from(count - 1);
                        c.g1()?;
                        let trigger = c.g4s()?;
                        let mut values = Vec::with_capacity(args);
                        for _ in 0..args {
                            values.push(match c.g1()? {
                                0 => ComponentHookArg::Int(c.g4s()?),
                                1 => ComponentHookArg::Str(c.gjstr()?),
                                other => {
                                    anyhow::bail!("hook argument type {other}")
                                }
                            });
                        }
                        DbValue::HookRef {
                            trigger: Some(trigger),
                            args: values,
                        }
                    }
                }
                other => anyhow::bail!("base var type serial {other} is not indexable"),
            };
            let n = c.gvarint2()?;
            anyhow::ensure!(n >= 0, "negative DB index row list");
            let mut rows = Vec::with_capacity(n as usize);
            for _ in 0..n {
                rows.push(c.gvarint2()?);
            }
            entries.push((key, rows));
        }
        Ok(Self {
            key_serial,
            entries,
        })
    }
    /// Row ids for a key; only a key of the same type can match.
    pub fn get(&self, key: &DbValue) -> Option<&Vec<i32>> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, rows)| rows)
    }
}

/// The finder state: the result list, its table (-1 at start) and the live
/// iterator (none until a find).
#[derive(Clone, Debug, PartialEq)]
pub struct Finder {
    pub results: Option<Vec<i32>>,
    pub table: i32,
    pub cursor: Option<usize>,
}
impl Default for Finder {
    fn default() -> Self {
        Self {
            results: None,
            table: -1,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    /// `columnTypes[column]`; `None` is an absent entry.
    pub column_types: Option<Vec<Option<Types>>>,
    /// `columnDefaultValues[column]`, allocated lazily.
    pub column_defaults: Option<Vec<Option<Vec<DbValue>>>>,
}
impl Table {
    /// Decode one database table definition.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let mut t = Self::default();
        loop {
            let opcode = c.g1()?;
            if opcode == 0 {
                return Ok(t);
            }
            if opcode != 1 {
                continue;
            }
            let count = c.g1()? as usize;
            let column_types = t.column_types.get_or_insert_with(|| vec![None; count]);
            loop {
                let header = c.g1()?;
                if header == 255 {
                    break;
                }
                let column = (header & 0x7F) as usize;
                let has_default = header & 0x80 != 0;
                let n = c.g1()? as usize;
                let types = read_types(&mut c, n)?;
                *column_types
                    .get_mut(column)
                    .ok_or_else(|| anyhow!("DB table column {column} outside {count}"))? =
                    Some(types.clone());
                if has_default {
                    let len = column_types.len();
                    let defaults = t.column_defaults.get_or_insert_with(|| vec![None; len]);
                    *defaults.get_mut(column).ok_or_else(|| {
                        anyhow!("DB table default column {column} outside {len}")
                    })? = Some(unpack_values(&mut c, &types)?);
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub column_values: Option<Vec<Option<Vec<DbValue>>>>,
    pub column_types: Option<Vec<Option<Types>>>,
    pub table_id: i32,
}
impl Row {
    /// Decode one database row.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let mut r = Self::default();
        loop {
            let opcode = c.g1()?;
            match opcode {
                0 => return Ok(r),
                3 => {
                    let count = c.g1()? as usize;
                    if r.column_values.is_none() {
                        r.column_values = Some(vec![None; count]);
                        r.column_types = Some(vec![None; count]);
                    }
                    loop {
                        let column = c.g1()?;
                        if column == 255 {
                            break;
                        }
                        let column = column as usize;
                        let n = c.g1()? as usize;
                        let types = read_types(&mut c, n)?;
                        let values = unpack_values(&mut c, &types)?;
                        let slot = r.column_values.as_mut().unwrap();
                        let len = slot.len();
                        *slot
                            .get_mut(column)
                            .ok_or_else(|| anyhow!("DB row column {column} outside {len}"))? =
                            Some(values);
                        r.column_types.as_mut().unwrap()[column] = Some(types);
                    }
                }
                4 => r.table_id = c.gvarint2()?,
                _ => {}
            }
        }
    }
    /// The values of one column, when the row has it.
    fn values(&self, column: usize) -> Option<&Vec<DbValue>> {
        self.column_values
            .as_ref()
            .and_then(|v| v.get(column))
            .and_then(Option::as_ref)
    }
}

/// The table types (config group 40) and row types (41): an absent file
/// decodes as a default instance.
#[derive(Default)]
pub struct Tables {
    pub tables: BTreeMap<i32, Table>,
    pub rows: BTreeMap<i32, Row>,
    empty_table: Table,
    empty_row: Row,
    /// The `dbtableindex` archive; indexes load on demand.
    pack: Option<Pack>,
    /// Index cache keyed by (table, file); `None` is a missing js5 file (a
    /// null-pointer failure when used).
    indexes: BTreeMap<(i32, u32), Option<Index>>,
    pub finder: Finder,
}
impl Tables {
    pub fn load(pack: &Pack) -> Result<Self> {
        let mut s = Self {
            pack: Some(pack.clone()),
            ..Self::default()
        };
        for (id, bytes) in pack
            .read_group("config", 40)
            .context("dbtable config group 40")?
        {
            s.tables.insert(
                id as i32,
                Table::decode(&bytes).with_context(|| format!("dbtable {id}"))?,
            );
        }
        for (id, bytes) in pack
            .read_group("config", 41)
            .context("dbrow config group 41")?
        {
            s.rows.insert(
                id as i32,
                Row::decode(&bytes).with_context(|| format!("dbrow {id}"))?,
            );
        }
        Ok(s)
    }
    pub fn table(&self, id: i32) -> &Table {
        self.tables.get(&id).unwrap_or(&self.empty_table)
    }
    pub fn row(&self, id: i32) -> &Row {
        self.rows.get(&id).unwrap_or(&self.empty_row)
    }
    /// Shared prologue of both commands: the column's declared types and the
    /// row values (falling back to the table defaults).
    fn column(&self, row_id: i32, field: i32) -> Result<(&Types, Option<&Vec<DbValue>>)> {
        // The table and column packed into the field id.
        let table_id = ((field as u32) >> 8) as i32;
        let column = (field & 0xFF) as usize;
        let row = self.row(row_id);
        let table = self.table(table_id);
        let types = table
            .column_types
            .as_ref()
            .with_context(|| Fault::MissingValue.message("DB table column types"))?
            .get(column)
            .with_context(|| Fault::IndexOutOfRange.message("DB column"))?
            .as_ref()
            .with_context(|| Fault::MissingValue.message("DB column types"))?;
        let mut values = row.values(column);
        if values.is_none() {
            if let Some(defaults) = &table.column_defaults {
                values = defaults.get(column).and_then(Option::as_ref);
            }
        }
        Ok((types, values))
    }
    pub fn dispatch(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
    ) -> Option<VmResult<Option<Value>>> {
        match command {
            "db_getfield" => Some(self.get_field(ints, objs)),
            "db_getfieldcount" => Some(self.get_field_count(ints)),
            "db_find" | "db_find_with_count" | "db_listall" | "db_findnext" | "db_find_get"
            | "db_find_refine" | "db_getrowtable" => Some(self.finder_command(command, ints)),
            _ => None,
        }
    }
    /// The index of `table` (file 0 lists all rows, file `column + 1` indexes a
    /// column), loaded once and cached.
    fn index(&mut self, command: &str, table: i32, file: u32) -> VmResult<&Index> {
        if !self.indexes.contains_key(&(table, file)) {
            let loaded = match (&self.pack, u32::try_from(table)) {
                (Some(pack), Ok(group)) => match pack.read_group("dbtableindex", group) {
                    Ok(files) => match files.get(&file) {
                        Some(bytes) => Some(Index::decode(bytes).map_err(|e| {
                            Self::failed(command, e.context(format!("dbtableindex {table}/{file}")))
                        })?),
                        None => None,
                    },
                    Err(_) => None,
                },
                _ => None,
            };
            self.indexes.insert((table, file), loaded);
        }
        self.indexes[&(table, file)].as_ref().ok_or_else(|| {
            Self::failed(
                command,
                anyhow!(Fault::MissingValue.message(format_args!(
                    "dbtableindex {table}/{file} is not in the archive"
                ))),
            )
        })
    }
    fn pop(command: &str, ints: &mut Vec<i32>, n: usize) -> VmResult<Vec<i32>> {
        if ints.len() < n {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let _ = command;
        Ok(ints.split_off(ints.len() - n))
    }
    fn finder_command(&mut self, command: &str, ints: &mut Vec<i32>) -> VmResult<Option<Value>> {
        match command {
            // db_find and db_find_with_count.
            "db_find" | "db_find_with_count" => {
                let a = Self::pop(command, ints, 2)?;
                let (field, key) = (a[0], a[1]);
                let table = ((field as u32) >> 8) as i32;
                let rows = self
                    .index(command, table, (field & 0xFF) as u32 + 1)?
                    .get(&DbValue::Int(key))
                    .cloned();
                let with_count = command == "db_find_with_count";
                match rows {
                    Some(rows) => {
                        let size = rows.len() as i32;
                        self.finder = Finder {
                            results: Some(rows),
                            table,
                            cursor: Some(0),
                        };
                        Ok(with_count.then_some(Value::Int(size)))
                    }
                    None => {
                        self.finder.results = None;
                        self.finder.table = -1;
                        self.finder.cursor = None;
                        Ok(with_count.then_some(Value::Int(0)))
                    }
                }
            }
            // :17125-17138: no push at all when the list-all index has no key 0.
            "db_listall" => {
                let table = Self::pop(command, ints, 1)?[0];
                let rows = self
                    .index(command, table, 0)?
                    .get(&DbValue::Int(0))
                    .cloned();
                match rows {
                    Some(rows) => {
                        let size = rows.len() as i32;
                        self.finder = Finder {
                            results: Some(rows),
                            table,
                            cursor: Some(0),
                        };
                        Ok(Some(Value::Int(size)))
                    }
                    None => {
                        self.finder.results = None;
                        Ok(None)
                    }
                }
            }
            // :17141-17147.
            "db_findnext" => {
                let next = match (&self.finder.results, self.finder.cursor) {
                    (Some(rows), Some(i)) if i < rows.len() => {
                        self.finder.cursor = Some(i + 1);
                        rows[i]
                    }
                    _ => -1,
                };
                Ok(Some(Value::Int(next)))
            }
            // :17213-17220.
            "db_find_get" => {
                let i = Self::pop(command, ints, 1)?[0];
                let v = match &self.finder.results {
                    Some(rows) if i >= 0 && (i as usize) < rows.len() => rows[i as usize],
                    _ => -1,
                };
                Ok(Some(Value::Int(v)))
            }
            // :17223-17245: `retainAll` keeps the current order.
            "db_find_refine" => {
                let a = Self::pop(command, ints, 2)?;
                let (field, key) = (a[0], a[1]);
                let table = ((field as u32) >> 8) as i32;
                let refine = self
                    .index(command, table, (field & 0xFF) as u32 + 1)?
                    .get(&DbValue::Int(key))
                    .cloned();
                if table != self.finder.table {
                    return Err(Self::failed(
                        command,
                        anyhow!(Fault::InvalidState.message(format_args!(
                            "refine table {table} differs from {}",
                            self.finder.table
                        ))),
                    ));
                }
                let Some(current) = self.finder.results.take() else {
                    // A missing result list is dereferenced, which fails.
                    return Err(Self::failed(
                        command,
                        anyhow!(Fault::MissingValue.message("finder result list")),
                    ));
                };
                let kept: Vec<i32> = match refine {
                    None => Vec::new(),
                    Some(list) => current.into_iter().filter(|r| list.contains(r)).collect(),
                };
                let size = kept.len() as i32;
                self.finder.results = Some(kept);
                self.finder.cursor = Some(0);
                Ok(Some(Value::Int(size)))
            }
            // :17248-17252.
            "db_getrowtable" => {
                let row = Self::pop(command, ints, 1)?[0];
                Ok(Some(Value::Int(self.row(row).table_id)))
            }
            _ => unreachable!(),
        }
    }
    fn failed(command: &str, e: anyhow::Error) -> VmError {
        VmError::TrapFailed {
            command: command.into(),
            reason: format!("{e:#}"),
        }
    }
    /// `db_getfield`.
    fn get_field(&self, ints: &mut Vec<i32>, objs: &mut Vec<String>) -> VmResult<Option<Value>> {
        if ints.len() < 3 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let args = ints.split_off(ints.len() - 3);
        let (row_id, field, index) = (args[0], args[1], args[2]);
        let (types, values) = self
            .column(row_id, field)
            .map_err(|e| Self::failed("db_getfield", e))?;
        let Some(values) = values else {
            // :17164-17176 typed empties.
            for &t in types {
                if t == 36 {
                    objs.push(String::new());
                } else if t == 0 || t == 1 {
                    ints.push(0);
                } else {
                    ints.push(-1);
                }
            }
            return Ok(None);
        };
        let per_row = values.len() / types.len();
        if index < 0 || index as usize >= per_row {
            return Err(Self::failed(
                "db_getfield",
                anyhow!(
                    Fault::InvalidState.message(format_args!("DB row index {index} of {per_row}"))
                ),
            ));
        }
        for (i, &t) in types.iter().enumerate() {
            let value = &values[types.len() * index as usize + i];
            if t == 36 {
                match value {
                    DbValue::Str(s) => objs.push(s.clone()),
                    other => {
                        return Err(Self::failed(
                            "db_getfield",
                            anyhow!(Fault::WrongValueType
                                .message(format_args!("{other:?} is not a string"))),
                        ))
                    }
                }
            } else {
                match value {
                    DbValue::Int(v) => ints.push(*v),
                    other => {
                        return Err(Self::failed(
                            "db_getfield",
                            anyhow!(Fault::WrongValueType
                                .message(format_args!("{other:?} is not an integer"))),
                        ))
                    }
                }
            }
        }
        Ok(None)
    }
    /// `db_getfieldcount`.
    fn get_field_count(&self, ints: &mut Vec<i32>) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let args = ints.split_off(ints.len() - 2);
        let (types, values) = self
            .column(args[0], args[1])
            .map_err(|e| Self::failed("db_getfieldcount", e))?;
        Ok(Some(Value::Int(
            values.map_or(0, |v| (v.len() / types.len()) as i32),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_and_table_decode() {
        // Table: 2 columns; column 0 = [INT, STRING] with default row ("x": 7);
        // column 1 = [INT] without default.
        let table = [
            1, 2, 0x80, 2, 0, 36, 1, 0, 0, 0, 7, b'x', 0, 1, 1, 0, 255, 0,
        ];
        let t = Table::decode(&table).unwrap();
        assert_eq!(
            t.column_types.as_ref().unwrap()[0].as_ref().unwrap(),
            &vec![0u8, 36]
        );
        assert_eq!(
            t.column_defaults.as_ref().unwrap()[0].as_ref().unwrap(),
            &vec![DbValue::Int(7), DbValue::Str("x".into())]
        );
        // Row: 2 columns; column 1 = [INT] x 2 rows (3, 4); table id 300.
        let row = [
            3, 2, 1, 1, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 255, 4, 0xAC, 0x02, 0,
        ];
        let r = Row::decode(&row).unwrap();
        assert_eq!(r.table_id, 300);
        assert_eq!(
            r.values(1).unwrap(),
            &vec![DbValue::Int(3), DbValue::Int(4)]
        );
        let mut tables = Tables::default();
        tables.tables.insert(5, t);
        tables.rows.insert(9, r);
        let mut ints = vec![9, (5 << 8) | 1, 1];
        let mut objs = vec![];
        assert!(tables
            .dispatch("db_getfield", &mut ints, &mut objs)
            .unwrap()
            .is_ok());
        assert_eq!(ints, vec![4]);
        let mut ints = vec![9, 5 << 8, 0];
        assert!(tables
            .dispatch("db_getfield", &mut ints, &mut objs)
            .unwrap()
            .is_ok());
        assert_eq!((ints, objs), (vec![7], vec!["x".to_string()]));
        let mut ints = vec![9, (5 << 8) | 1];
        let r = tables
            .dispatch("db_getfieldcount", &mut ints, &mut vec![])
            .unwrap()
            .unwrap();
        assert_eq!(r, Some(Value::Int(2)));
        // Absent row + absent default -> typed empties.
        let mut ints = vec![77, (5 << 8) | 1, 0];
        assert!(tables
            .dispatch("db_getfield", &mut ints, &mut vec![])
            .unwrap()
            .is_ok());
        assert_eq!(ints, vec![0]);
    }

    #[test]
    fn component_hook_index_keys_decode_for_list_all() {
        // CCHOOK key with no hook arguments: marker=1, skipped hook flag,
        // trigger 7, followed by one row id 5.
        let index = Index::decode(&[4, 1, 1, 0, 0, 0, 0, 7, 1, 5]).unwrap();
        assert_eq!(
            index.entries,
            vec![(
                DbValue::HookRef {
                    trigger: Some(7),
                    args: vec![]
                },
                vec![5]
            )]
        );
    }
}
