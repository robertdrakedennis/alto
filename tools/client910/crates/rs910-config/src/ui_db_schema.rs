//! Schema-driven database definitions and field projection. Wire tags and
//! packed-field layout are supplied by a verified revision description.
use crate::ui_bytes::Cursor;
use anyhow::{anyhow, bail, ensure, Result};
use std::collections::BTreeMap;
use std::ops::Range;

const SELECTOR_BASE: usize = 1;
const VARINT_GROUP_BITS: u32 = 7;
const VARINT_PAYLOAD_MASK: u8 = 0x7f;
const VARINT_CONTINUATION_MASK: u8 = 0x80;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BaseType {
    Int,
    Long,
    Text,
    Coordinate,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Int(i32),
    Long(i64),
    Text(String),
    Coordinate { level: i32, position: [f32; 3] },
}
impl FieldValue {
    pub fn base_type(&self) -> BaseType {
        match self {
            Self::Int(_) => BaseType::Int,
            Self::Long(_) => BaseType::Long,
            Self::Text(_) => BaseType::Text,
            Self::Coordinate { .. } => BaseType::Coordinate,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScriptType {
    pub base: BaseType,
    pub default: FieldValue,
}
pub type TypeCatalog = BTreeMap<u16, ScriptType>;

#[derive(Clone, Copy, Debug)]
pub struct DefinitionFormat {
    pub end: u8,
    pub table_name: u8,
    pub table_columns: u8,
    pub reserved_header_bytes: usize,
    pub end_column: u8,
    pub end_columns: u8,
    pub column_types: u8,
    pub column_defaults: u8,
    pub column_name: u8,
    pub row_columns: u8,
    pub row_table: u8,
    pub drop_unassigned_text_bytes: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldLayout {
    table_shift: u32,
    column_shift: u32,
    column_mask: u32,
    selector_mask: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldId {
    pub table: u32,
    pub column: usize,
    /// Zero selects the entire tuple; other values are one-based selectors.
    pub selector: usize,
}
impl FieldLayout {
    pub fn new(
        table_shift: u32,
        column_shift: u32,
        column_mask: u32,
        selector_mask: u32,
    ) -> Result<Self> {
        ensure!(
            table_shift < u32::BITS && column_shift < table_shift,
            "invalid field shifts"
        );
        ensure!(
            column_mask >> (table_shift - column_shift) == 0,
            "column overlaps table"
        );
        ensure!(
            selector_mask >> column_shift == 0,
            "selector overlaps column"
        );
        Ok(Self {
            table_shift,
            column_shift,
            column_mask,
            selector_mask,
        })
    }
    pub(crate) fn parts(self) -> (u32, u32, u32, u32) {
        (
            self.table_shift,
            self.column_shift,
            self.column_mask,
            self.selector_mask,
        )
    }
    pub fn pack(self, table: u32, column: usize, selector: usize) -> Result<i32> {
        let column = u32::try_from(column)?;
        let selector = u32::try_from(selector)?;
        ensure!(
            table <= u32::MAX >> self.table_shift
                && column & !self.column_mask == 0
                && selector & !self.selector_mask == 0,
            "field address outside layout"
        );
        Ok(((table << self.table_shift) | (column << self.column_shift) | selector) as i32)
    }
    pub fn unpack(self, packed: i32) -> FieldId {
        let packed = packed as u32;
        FieldId {
            table: packed >> self.table_shift,
            column: ((packed >> self.column_shift) & self.column_mask) as usize,
            selector: (packed & self.selector_mask) as usize,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Column {
    pub types: Vec<u16>,
    pub defaults: Vec<FieldValue>,
    pub name: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub name: Option<String>,
    pub reserved_headers: Vec<Vec<u8>>,
    pub columns: Vec<Column>,
    /// Unknown tags consume no payload. Their offsets remain inspectable.
    pub ignored_tags: Vec<(usize, u8)>,
}
#[derive(Clone, Debug, Default)]
pub struct RowColumn {
    pub types: Vec<u16>,
    pub values: Vec<FieldValue>,
}
#[derive(Clone, Debug, Default)]
pub struct Row {
    /// Absence is retained rather than inventing a table identity.
    pub table: Option<i32>,
    pub columns: Vec<RowColumn>,
    pub ignored_tags: Vec<(usize, u8)>,
}

pub struct Codec<'a> {
    pub format: DefinitionFormat,
    pub types: &'a TypeCatalog,
}
impl Codec<'_> {
    fn text(&self, cursor: &mut Cursor<'_>) -> Result<String> {
        let mut text = String::new();
        loop {
            let byte = cursor.g1()?;
            if byte == u8::default() {
                return Ok(text);
            }
            if self.format.drop_unassigned_text_bytes {
                if let Some(character) = rs910_core::cp1252::cp1252(byte) {
                    text.push(character);
                }
            } else {
                text.push(rs910_core::cp1252::cp1252_decode_byte(byte));
            }
        }
    }
    fn types(&self, cursor: &mut Cursor<'_>) -> Result<Vec<u16>> {
        let count = usize::from(cursor.g1()?);
        // Unknown serials are retained when no values need their codec.
        (0..count)
            .map(|_| Ok(u16::try_from(cursor.gsmart1or2()?)?))
            .collect()
    }
    fn values(&self, cursor: &mut Cursor<'_>, types: &[u16]) -> Result<Vec<FieldValue>> {
        let rows = usize::try_from(cursor.gsmart1or2()?)?;
        let mut values = Vec::new();
        for _ in 0..rows {
            for serial in types {
                let base = self
                    .types
                    .get(serial)
                    .ok_or_else(|| anyhow!("unknown script type {serial}"))?
                    .base;
                values.push(match base {
                    BaseType::Int => FieldValue::Int(cursor.g4s()?),
                    BaseType::Long => FieldValue::Long(cursor.g8()?),
                    BaseType::Text => FieldValue::Text(self.text(cursor)?),
                    BaseType::Coordinate => FieldValue::Coordinate {
                        level: i32::from(cursor.g1()?),
                        position: [
                            cursor.g4s()? as f32,
                            cursor.g4s()? as f32,
                            cursor.g4s()? as f32,
                        ],
                    },
                });
            }
        }
        Ok(values)
    }
    pub fn table(&self, bytes: &[u8]) -> Result<Table> {
        let mut cursor = Cursor::new(bytes);
        let mut table = Table::default();
        loop {
            let offset = cursor.pos();
            let tag = cursor.g1()?;
            if tag == self.format.end {
                break;
            }
            if tag == self.format.table_name {
                table.name = Some(self.text(&mut cursor)?);
            } else if tag == self.format.table_columns {
                let header = (0..self.format.reserved_header_bytes)
                    .map(|_| cursor.g1())
                    .collect::<Result<_>>()?;
                table.reserved_headers.push(header);
                table.columns.clear();
                table
                    .columns
                    .resize_with(usize::from(cursor.g1()?), Column::default);
                loop {
                    let index = cursor.g1()?;
                    if index == self.format.end_columns {
                        break;
                    }
                    let column = table
                        .columns
                        .get_mut(usize::from(index))
                        .ok_or_else(|| anyhow!("table column {index} outside schema"))?;
                    loop {
                        let offset = cursor.pos();
                        let tag = cursor.g1()?;
                        if tag == self.format.end_column {
                            break;
                        }
                        if tag == self.format.column_types {
                            column.types = self.types(&mut cursor)?;
                        } else if tag == self.format.column_defaults {
                            column.defaults = self.values(&mut cursor, &column.types)?;
                        } else if tag == self.format.column_name {
                            column.name = Some(self.text(&mut cursor)?);
                        } else {
                            table.ignored_tags.push((offset, tag));
                        }
                    }
                }
            } else {
                table.ignored_tags.push((offset, tag));
            }
        }
        ensure!(
            cursor.remaining() == 0,
            "trailing DB table bytes at {}",
            cursor.pos()
        );
        Ok(table)
    }
    pub fn row(&self, bytes: &[u8]) -> Result<Row> {
        let mut cursor = Cursor::new(bytes);
        let mut row = Row::default();
        loop {
            let offset = cursor.pos();
            let tag = cursor.g1()?;
            if tag == self.format.end {
                break;
            }
            if tag == self.format.row_table {
                let mut value = u64::default();
                let mut shift = u32::default();
                loop {
                    let byte = cursor.g1()?;
                    value |= u64::from(byte & VARINT_PAYLOAD_MASK).wrapping_shl(shift);
                    shift = shift.wrapping_add(VARINT_GROUP_BITS);
                    if byte & VARINT_CONTINUATION_MASK == 0 {
                        break;
                    }
                }
                row.table = Some(value as i32);
            } else if tag == self.format.row_columns {
                row.columns.clear();
                row.columns
                    .resize_with(usize::from(cursor.g1()?), RowColumn::default);
                loop {
                    let index = cursor.g1()?;
                    if index == self.format.end_columns {
                        break;
                    }
                    let column = row
                        .columns
                        .get_mut(usize::from(index))
                        .ok_or_else(|| anyhow!("row column {index} outside schema"))?;
                    column.types = self.types(&mut cursor)?;
                    column.values = self.values(&mut cursor, &column.types)?;
                }
            } else {
                row.ignored_tags.push((offset, tag));
            }
        }
        ensure!(
            cursor.remaining() == 0,
            "trailing DB row bytes at {}",
            cursor.pos()
        );
        Ok(row)
    }
}

/// Shared schema projection for inspection and the runtime adapter. The row's
/// own table identity does not override the table packed into a field.
pub struct Database {
    pub layout: FieldLayout,
    pub types: TypeCatalog,
    pub tables: BTreeMap<u32, Table>,
    pub rows: BTreeMap<i32, Row>,
}
impl Database {
    fn column(&self, field: FieldId) -> Option<&Column> {
        self.tables.get(&field.table)?.columns.get(field.column)
    }
    fn values<'a>(&'a self, row: i32, field: FieldId, column: &'a Column) -> &'a [FieldValue] {
        self.rows
            .get(&row)
            .and_then(|row| row.columns.get(field.column))
            .map(|column| column.values.as_slice())
            .filter(|values| !values.is_empty())
            .unwrap_or(&column.defaults)
    }
    fn selection(&self, field: FieldId, width: usize) -> Result<Range<usize>> {
        ensure!(width != 0, "DB field has no tuple schema");
        if field.selector == 0 {
            return Ok(0..width);
        }
        ensure!(
            field.selector <= width,
            "DB tuple selector {} outside width {width}",
            field.selector
        );
        Ok(field.selector - SELECTOR_BASE..field.selector)
    }
    pub fn field_types(&self, packed: i32) -> Result<&[u16]> {
        let field = self.layout.unpack(packed);
        let types = &self
            .column(field)
            .ok_or_else(|| anyhow!("missing DB field schema"))?
            .types;
        Ok(&types[self.selection(field, types.len())?])
    }
    /// Count ignores the tuple selector and uses the complete column width.
    pub fn field_count(&self, row: i32, packed: i32) -> usize {
        let field = self.layout.unpack(packed);
        self.column(field)
            .filter(|column| !column.types.is_empty())
            .map_or(0, |column| {
                self.values(row, field, column).len() / column.types.len()
            })
    }
    /// Deliver in tuple order. A failure in a later element retains the sink's
    /// earlier writes, matching the script command's incremental pushes.
    pub fn visit_field(
        &self,
        row: i32,
        packed: i32,
        index: i32,
        mut push: impl FnMut(&FieldValue) -> Result<()>,
    ) -> Result<()> {
        let field = self.layout.unpack(packed);
        let column = self
            .column(field)
            .ok_or_else(|| anyhow!("missing DB field schema"))?;
        let selection = self.selection(field, column.types.len())?;
        let values = self.values(row, field, column);
        let offset = if values.is_empty() {
            0
        } else {
            let rows = values.len() / column.types.len();
            ensure!(
                (index as u32 as usize) < rows,
                "DB row index {index} outside {rows}"
            );
            column.types.len() * index as u32 as usize
        };
        for element in selection {
            let serial = column.types[element];
            let script_type = self
                .types
                .get(&serial)
                .ok_or_else(|| anyhow!("unknown script type {serial}"))?;
            let value = if values.is_empty() {
                &script_type.default
            } else {
                &values[offset + element]
            };
            if value.base_type() != script_type.base {
                bail!("DB value does not match script type {serial}");
            }
            push(value)?;
        }
        Ok(())
    }
}
