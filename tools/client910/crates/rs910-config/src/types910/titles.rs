//! Appearance title inputs: the title enums and the customization defaults
//! that name them. Array lookup takes priority over map lookup even if a
//! later opcode replaced the map. Values are typed. An opcode outside the
//! enum table is an error.
use super::{Error, Packet, Result};
use crate::opcode_table::{at, span, Entry, Field, Input, Record, Rule, Slot, Table, Unknown};
use std::collections::BTreeMap;

/// A title enum value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i32),
    String(String),
}

/// One title enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enum {
    pub default_string: String,
    pub default_int: i32,
    pub count: i32,
    pub values: Option<BTreeMap<i32, Value>>,
    pub indexed: Option<Vec<Option<Value>>>,
    pub consumed: usize,
}

impl Default for Enum {
    fn default() -> Self {
        Self {
            default_string: "null".into(),
            default_int: 0,
            count: 0,
            values: None,
            indexed: None,
            consumed: 0,
        }
    }
}

/// Title enum opcodes of this revision.
static ENUM_OPCODES: Table<Enum, Error> = Table::new(
    &[
        // Legacy key/value type ids: must be non-zero.
        Entry::new(span(1, 2), Rule::Custom(check_legacy_type)),
        Entry::new(at(3), Rule::Text(|e, _, v| e.default_string = v)),
        Entry::new(at(4), Rule::Int(|e, _, v| e.default_int = v)),
        Entry::new(span(5, 6), Rule::Custom(read_map)),
        Entry::new(span(7, 8), Rule::Custom(read_indexed)),
        Entry::new(span(101, 102), Rule::Skip(&[Field::Smart])),
    ],
    Unknown::Fail(|_, _| Error::UnsupportedContext("unknown title enum opcode")),
);

fn check_legacy_type(source: Input<Error>, _: &mut Enum, _: Slot) -> Result<()> {
    if source.byte()? == 0 {
        return Err(Error::Invalid("zero enum legacy type"));
    }
    Ok(())
}

/// Sparse values: a count, then `(key, value)` pairs, the value a string for
/// the first opcode and an integer for the second.
fn read_map(source: Input<Error>, e: &mut Enum, slot: Slot) -> Result<()> {
    e.count = i32::from(source.short()?);
    let mut values = BTreeMap::new();
    for _ in 0..e.count {
        let key = source.int()?;
        values.insert(key, read_value(source, slot == 0)?);
    }
    e.values = Some(values);
    Ok(())
}

/// Dense values: a capacity, a count, then `(index, value)` pairs.
fn read_indexed(source: Input<Error>, e: &mut Enum, slot: Slot) -> Result<()> {
    let capacity = usize::from(source.short()?);
    e.count = i32::from(source.short()?);
    let mut indexed = vec![None; capacity];
    for _ in 0..e.count {
        let key = usize::from(source.short()?);
        let value = read_value(source, slot == 0)?;
        *indexed.get_mut(key).ok_or(Error::Invalid("enum index"))? = Some(value);
    }
    e.indexed = Some(indexed);
    Ok(())
}

fn read_value(source: Input<Error>, is_string: bool) -> Result<Value> {
    Ok(if is_string {
        Value::String(source.text()?)
    } else {
        Value::Int(source.int()?)
    })
}

impl Enum {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut e = Self::empty();
        let mut packet = Packet::new(bytes);
        let record = Record {
            kind: "title enum",
            id: -1,
        };
        ENUM_OPCODES.run(&mut packet, &mut e, record)?;
        e.consumed = packet.pos;
        Ok(e)
    }

    pub fn value(&self, key: i32) -> Option<&Value> {
        if let Some(v) = &self.indexed {
            if key < 0 {
                None
            } else {
                v.get(key as usize).and_then(Option::as_ref)
            }
        } else {
            self.values.as_ref().and_then(|m| m.get(&key))
        }
    }
    pub fn string(&self, key: i32) -> Result<&str> {
        match self.value(key) {
            None => Ok(&self.default_string),
            Some(Value::String(s)) => Ok(s),
            _ => Err(Error::Invalid("non-string title enum value")),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Defaults {
    pub enums: [i32; 2],
    pub consumed: usize,
}
impl Defaults {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut p = Packet::new(bytes);
        let mut enums = [-1; 2];
        loop {
            // Any opcode other than 1 ends the list, not only 0.
            if p.byte()? != 1 {
                return Ok(Self {
                    enums,
                    consumed: p.pos,
                });
            }
            enums = [p.smart2()?, p.smart2()?];
        }
    }
}
pub fn install(c: &mut super::appearance::Config, enums: &[Enum; 2]) -> Result<()> {
    let mut titles = BTreeMap::new();
    for (gender, e) in enums.iter().enumerate() {
        // Title ids are nonnegative and at most 32767.
        for id in 0..32768 {
            if e.value(id).is_some() {
                titles.insert((gender as i8, id), e.string(id)?.to_owned());
            }
        }
    }
    c.titles = titles;
    c.default_titles = [
        enums[0].default_string.clone(),
        enums[1].default_string.clone(),
    ];
    Ok(())
}
