//! Engine-owned database resource image. Imported scripts bind its exact bytes;
//! it preserves typed values without a revision-specific cache reader at runtime.
use crate::ui_db_schema::{
    Column, Database, FieldLayout, FieldValue, Row, RowColumn, ScriptType, Table,
};
use anyhow::{ensure, Result};
use native910::packet::{ByteWriter, Packet};
use std::collections::BTreeMap;

const FORMAT: &[u8] = b"ALTO-DB\x01";
const INT: u8 = 1;
const LONG: u8 = 2;
const TEXT: u8 = 3;
const COORDINATE: u8 = 4;
const PRESENT: u8 = 1;

fn count(writer: &mut ByteWriter, length: usize) -> Result<()> {
    writer.p4s(i32::try_from(length)?);
    Ok(())
}
fn bytes(writer: &mut ByteWriter, value: &[u8]) -> Result<()> {
    count(writer, value.len())?;
    writer.pdata(value);
    Ok(())
}
fn text(writer: &mut ByteWriter, value: &Option<String>) -> Result<()> {
    writer.p1(u8::from(value.is_some()));
    if let Some(value) = value {
        bytes(writer, value.as_bytes())?;
    }
    Ok(())
}
fn value(writer: &mut ByteWriter, value: &FieldValue) -> Result<()> {
    match value {
        FieldValue::Int(value) => {
            writer.p1(INT);
            writer.p4s(*value);
        }
        FieldValue::Long(value) => {
            writer.p1(LONG);
            writer.p8s(*value);
        }
        FieldValue::Text(value) => {
            writer.p1(TEXT);
            bytes(writer, value.as_bytes())?;
        }
        FieldValue::Coordinate { level, position } => {
            writer.p1(COORDINATE);
            writer.p4s(*level);
            for axis in position {
                writer.p4s(axis.to_bits() as i32);
            }
        }
    }
    Ok(())
}
fn values(writer: &mut ByteWriter, entries: &[FieldValue]) -> Result<()> {
    count(writer, entries.len())?;
    for entry in entries {
        value(writer, entry)?;
    }
    Ok(())
}
fn types(writer: &mut ByteWriter, entries: &[u16]) -> Result<()> {
    count(writer, entries.len())?;
    for entry in entries {
        writer.p2(*entry);
    }
    Ok(())
}
fn ignored(writer: &mut ByteWriter, entries: &[(usize, u8)]) -> Result<()> {
    count(writer, entries.len())?;
    for (offset, tag) in entries {
        count(writer, *offset)?;
        writer.p1(*tag);
    }
    Ok(())
}
fn read_count(packet: &mut Packet<'_>) -> Result<usize> {
    let length = usize::try_from(packet.g4s()?)?;
    ensure!(
        length <= packet.remaining(),
        "resource count exceeds remaining bytes"
    );
    Ok(length)
}
fn read_bytes(packet: &mut Packet<'_>) -> Result<Vec<u8>> {
    let length = read_count(packet)?;
    Ok(packet.gdata(length, "database resource bytes")?)
}
fn present(packet: &mut Packet<'_>) -> Result<bool> {
    let tag = packet.g1()?;
    ensure!(tag <= PRESENT, "invalid optional resource value");
    Ok(tag == PRESENT)
}
fn read_text(packet: &mut Packet<'_>) -> Result<Option<String>> {
    if present(packet)? {
        Ok(Some(String::from_utf8(read_bytes(packet)?)?))
    } else {
        Ok(None)
    }
}
fn read_value(packet: &mut Packet<'_>) -> Result<FieldValue> {
    Ok(match packet.g1()? {
        INT => FieldValue::Int(packet.g4s()?),
        LONG => FieldValue::Long(packet.g8s()?),
        TEXT => FieldValue::Text(String::from_utf8(read_bytes(packet)?)?),
        COORDINATE => FieldValue::Coordinate {
            level: packet.g4s()?,
            position: [
                f32::from_bits(packet.g4s()? as u32),
                f32::from_bits(packet.g4s()? as u32),
                f32::from_bits(packet.g4s()? as u32),
            ],
        },
        _ => anyhow::bail!("invalid database resource value tag"),
    })
}
fn read_values(packet: &mut Packet<'_>) -> Result<Vec<FieldValue>> {
    (0..read_count(packet)?)
        .map(|_| read_value(packet))
        .collect()
}
fn read_types(packet: &mut Packet<'_>) -> Result<Vec<u16>> {
    (0..read_count(packet)?).map(|_| Ok(packet.g2()?)).collect()
}
fn read_ignored(packet: &mut Packet<'_>) -> Result<Vec<(usize, u8)>> {
    (0..read_count(packet)?)
        .map(|_| Ok((usize::try_from(packet.g4s()?)?, packet.g1()?)))
        .collect()
}
impl Database {
    pub fn encode_resource(&self) -> Result<Vec<u8>> {
        let mut writer = ByteWriter::default();
        writer.pdata(FORMAT);
        let (table_shift, column_shift, column_mask, selector_mask) = self.layout.parts();
        for part in [table_shift, column_shift, column_mask, selector_mask] {
            writer.p4s(part as i32);
        }
        count(&mut writer, self.types.len())?;
        for (id, kind) in &self.types {
            ensure!(
                kind.base == kind.default.base_type(),
                "resource default base mismatch"
            );
            writer.p2(*id);
            value(&mut writer, &kind.default)?;
        }
        count(&mut writer, self.tables.len())?;
        for (id, table) in &self.tables {
            writer.p4s(*id as i32);
            text(&mut writer, &table.name)?;
            count(&mut writer, table.reserved_headers.len())?;
            for header in &table.reserved_headers {
                bytes(&mut writer, header)?;
            }
            ignored(&mut writer, &table.ignored_tags)?;
            count(&mut writer, table.columns.len())?;
            for column in &table.columns {
                types(&mut writer, &column.types)?;
                values(&mut writer, &column.defaults)?;
                text(&mut writer, &column.name)?;
            }
        }
        count(&mut writer, self.rows.len())?;
        for (id, row) in &self.rows {
            writer.p4s(*id);
            writer.p1(u8::from(row.table.is_some()));
            if let Some(table) = row.table {
                writer.p4s(table);
            }
            ignored(&mut writer, &row.ignored_tags)?;
            count(&mut writer, row.columns.len())?;
            for column in &row.columns {
                types(&mut writer, &column.types)?;
                values(&mut writer, &column.values)?;
            }
        }
        Ok(writer.data)
    }
    pub fn decode_resource(bytes: &[u8]) -> Result<Self> {
        let mut packet = Packet::new(bytes);
        ensure!(
            packet.gdata(FORMAT.len(), "database resource header")? == FORMAT,
            "unknown database resource format"
        );
        let layout = FieldLayout::new(
            packet.g4s()? as u32,
            packet.g4s()? as u32,
            packet.g4s()? as u32,
            packet.g4s()? as u32,
        )?;
        let mut types = BTreeMap::new();
        for _ in 0..read_count(&mut packet)? {
            let id = packet.g2()?;
            let default = read_value(&mut packet)?;
            ensure!(
                types
                    .insert(
                        id,
                        ScriptType {
                            base: default.base_type(),
                            default
                        }
                    )
                    .is_none(),
                "duplicate resource type"
            );
        }
        let mut tables = BTreeMap::new();
        for _ in 0..read_count(&mut packet)? {
            let id = packet.g4s()? as u32;
            let name = read_text(&mut packet)?;
            let reserved_headers = (0..read_count(&mut packet)?)
                .map(|_| read_bytes(&mut packet))
                .collect::<Result<_>>()?;
            let ignored_tags = read_ignored(&mut packet)?;
            let columns = (0..read_count(&mut packet)?)
                .map(|_| {
                    Ok(Column {
                        types: read_types(&mut packet)?,
                        defaults: read_values(&mut packet)?,
                        name: read_text(&mut packet)?,
                    })
                })
                .collect::<Result<_>>()?;
            ensure!(
                tables
                    .insert(
                        id,
                        Table {
                            name,
                            reserved_headers,
                            ignored_tags,
                            columns
                        }
                    )
                    .is_none(),
                "duplicate resource table"
            );
        }
        let mut rows = BTreeMap::new();
        for _ in 0..read_count(&mut packet)? {
            let id = packet.g4s()?;
            let table = if present(&mut packet)? {
                Some(packet.g4s()?)
            } else {
                None
            };
            let ignored_tags = read_ignored(&mut packet)?;
            let columns = (0..read_count(&mut packet)?)
                .map(|_| {
                    Ok(RowColumn {
                        types: read_types(&mut packet)?,
                        values: read_values(&mut packet)?,
                    })
                })
                .collect::<Result<_>>()?;
            ensure!(
                rows.insert(
                    id,
                    Row {
                        table,
                        ignored_tags,
                        columns
                    }
                )
                .is_none(),
                "duplicate resource row"
            );
        }
        ensure!(packet.remaining() == 0, "trailing database resource bytes");
        Ok(Self {
            layout,
            types,
            tables,
            rows,
        })
    }
}
