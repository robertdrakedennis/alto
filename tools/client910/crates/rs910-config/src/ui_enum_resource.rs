//! Engine-owned enum resources with resolved types and exact value tags.
use crate::ui_enum_schema::{Definition, Value};
use anyhow::{ensure, Result};
use native910::packet::{ByteWriter, Packet};
use std::collections::BTreeMap;

const FORMAT: &[u8] = b"ALTO-ENUM\x01";
const ABSENT: u8 = 0;
const INT: u8 = 1;
const TEXT: u8 = 2;
const PRESENT: u8 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Library {
    pub string_type: u16,
    pub definitions: BTreeMap<i32, Definition>,
}
fn count(writer: &mut ByteWriter, length: usize) -> Result<()> {
    writer.p4s(i32::try_from(length)?);
    Ok(())
}
fn text(writer: &mut ByteWriter, value: &str) -> Result<()> {
    count(writer, value.len())?;
    writer.pdata(value.as_bytes());
    Ok(())
}
fn kind(writer: &mut ByteWriter, value: Option<u16>) {
    writer.p1(u8::from(value.is_some()));
    if let Some(value) = value {
        writer.p2(value);
    }
}
fn value(writer: &mut ByteWriter, entry: Option<&Value>) -> Result<()> {
    match entry {
        None => writer.p1(ABSENT),
        Some(Value::Int(value)) => {
            writer.p1(INT);
            writer.p4s(*value);
        }
        Some(Value::Text(value)) => {
            writer.p1(TEXT);
            text(writer, value)?;
        }
    }
    Ok(())
}
fn read_count(packet: &mut Packet<'_>) -> Result<usize> {
    let count = usize::try_from(packet.g4s()?)?;
    ensure!(
        count <= packet.remaining(),
        "enum resource count exceeds remaining bytes"
    );
    Ok(count)
}
fn read_text(packet: &mut Packet<'_>) -> Result<String> {
    let length = read_count(packet)?;
    Ok(String::from_utf8(
        packet.gdata(length, "enum resource text")?,
    )?)
}
fn read_kind(packet: &mut Packet<'_>) -> Result<Option<u16>> {
    let present = packet.g1()?;
    ensure!(present <= PRESENT, "invalid enum type presence");
    if present == PRESENT {
        Ok(Some(packet.g2()?))
    } else {
        Ok(None)
    }
}
fn read_value(packet: &mut Packet<'_>) -> Result<Option<Value>> {
    Ok(match packet.g1()? {
        ABSENT => None,
        INT => Some(Value::Int(packet.g4s()?)),
        TEXT => Some(Value::Text(read_text(packet)?)),
        _ => anyhow::bail!("invalid enum resource value tag"),
    })
}
impl Library {
    pub fn encode_resource(&self) -> Result<Vec<u8>> {
        let mut writer = ByteWriter::default();
        writer.pdata(FORMAT);
        writer.p2(self.string_type);
        count(&mut writer, self.definitions.len())?;
        for (id, definition) in &self.definitions {
            writer.p4s(*id);
            kind(&mut writer, definition.input_type);
            kind(&mut writer, definition.output_type);
            writer.p4s(definition.integer_default);
            text(&mut writer, &definition.text_default)?;
            writer.p2(definition.wire_count);
            count(&mut writer, definition.consumed)?;
            count(&mut writer, definition.ignored_tags.len())?;
            for (offset, tag) in &definition.ignored_tags {
                count(&mut writer, *offset)?;
                writer.p1(*tag);
            }
            count(&mut writer, definition.sparse.len())?;
            for (key, entry) in &definition.sparse {
                writer.p4s(*key);
                value(&mut writer, Some(entry))?;
            }
            count(&mut writer, definition.dense.len())?;
            for entry in &definition.dense {
                value(&mut writer, entry.as_ref())?;
            }
        }
        Ok(writer.data)
    }
    pub fn decode_resource(bytes: &[u8]) -> Result<Self> {
        let mut packet = Packet::new(bytes);
        ensure!(
            packet.gdata(FORMAT.len(), "enum resource header")? == FORMAT,
            "unknown enum resource format"
        );
        let string_type = packet.g2()?;
        let mut definitions = BTreeMap::new();
        for _ in 0..read_count(&mut packet)? {
            let id = packet.g4s()?;
            let input_type = read_kind(&mut packet)?;
            let output_type = read_kind(&mut packet)?;
            let integer_default = packet.g4s()?;
            let text_default = read_text(&mut packet)?;
            let wire_count = packet.g2()?;
            let consumed = usize::try_from(packet.g4s()?)?;
            let mut ignored_tags = Vec::new();
            for _ in 0..read_count(&mut packet)? {
                ignored_tags.push((usize::try_from(packet.g4s()?)?, packet.g1()?));
            }
            let mut sparse = BTreeMap::new();
            for _ in 0..read_count(&mut packet)? {
                let key = packet.g4s()?;
                let entry = read_value(&mut packet)?
                    .ok_or_else(|| anyhow::anyhow!("absent sparse enum value"))?;
                ensure!(
                    sparse.insert(key, entry).is_none(),
                    "duplicate enum resource key"
                );
            }
            let dense = (0..read_count(&mut packet)?)
                .map(|_| read_value(&mut packet))
                .collect::<Result<_>>()?;
            ensure!(
                definitions
                    .insert(
                        id,
                        Definition {
                            input_type,
                            output_type,
                            integer_default,
                            text_default,
                            wire_count,
                            sparse,
                            dense,
                            ignored_tags,
                            consumed
                        }
                    )
                    .is_none(),
                "duplicate enum resource identity"
            );
        }
        ensure!(
            packet.remaining() == usize::default(),
            "trailing enum resource bytes"
        );
        Ok(Self {
            string_type,
            definitions,
        })
    }
}
