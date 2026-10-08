//! Revision-described enum definitions. Declared script types and actual value
//! tags are separate; lookup retains both sparse and dense representations.
use crate::ui_bytes::Cursor;
use anyhow::{anyhow, Result};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Int(i32),
    Text(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Definition {
    pub input_type: Option<u16>,
    pub output_type: Option<u16>,
    pub integer_default: i32,
    pub text_default: String,
    /// The last block's declared count, independent of retained map entries.
    pub wire_count: u16,
    pub sparse: BTreeMap<i32, Value>,
    pub dense: Vec<Option<Value>>,
    pub ignored_tags: Vec<(usize, u8)>,
    pub consumed: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryError {
    TypeMismatch,
    ValueTagMismatch,
}
impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TypeMismatch => "enum declared type mismatch",
            Self::ValueTagMismatch => "enum value tag mismatch",
        })
    }
}
impl std::error::Error for QueryError {}

impl Definition {
    pub fn lookup(&self, key: i32) -> Option<&Value> {
        if self.dense.is_empty() {
            self.sparse.get(&key)
        } else {
            usize::try_from(key)
                .ok()
                .and_then(|key| self.dense.get(key))
                .and_then(Option::as_ref)
        }
    }
    pub fn query(
        &self,
        input_type: i32,
        output_type: i32,
        key: i32,
        string_type: u16,
    ) -> std::result::Result<Value, QueryError> {
        if self.input_type.map(i32::from) != Some(input_type)
            || self.output_type.map(i32::from) != Some(output_type)
        {
            return Err(QueryError::TypeMismatch);
        }
        let value = self.lookup(key);
        if output_type == i32::from(string_type) {
            match value {
                Some(Value::Text(value)) => Ok(Value::Text(value.clone())),
                None => Ok(Value::Text(self.text_default.clone())),
                _ => Err(QueryError::ValueTagMismatch),
            }
        } else {
            match value {
                Some(Value::Int(value)) => Ok(Value::Int(*value)),
                None => Ok(Value::Int(self.integer_default)),
                _ => Err(QueryError::ValueTagMismatch),
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DefinitionFormat {
    pub end: u8,
    pub input_character: u8,
    pub output_character: u8,
    pub text_default: u8,
    pub integer_default: u8,
    pub sparse_text: u8,
    pub sparse_integer: u8,
    pub dense_text: u8,
    pub dense_integer: u8,
    pub input_serial: u8,
    pub output_serial: u8,
    pub initial_integer: i32,
    pub drop_unassigned_text_bytes: bool,
}

pub struct Codec<'a> {
    pub format: DefinitionFormat,
    pub initial_text: &'a str,
    pub types: &'a BTreeSet<u16>,
    pub legacy_types: &'a BTreeMap<u8, u16>,
}
impl Codec<'_> {
    pub fn empty(&self) -> Definition {
        Definition {
            input_type: None,
            output_type: None,
            integer_default: self.format.initial_integer,
            text_default: self.initial_text.into(),
            wire_count: u16::default(),
            sparse: BTreeMap::new(),
            dense: Vec::new(),
            ignored_tags: Vec::new(),
            consumed: usize::default(),
        }
    }
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
    fn value(&self, cursor: &mut Cursor<'_>, text: bool) -> Result<Value> {
        if text {
            Ok(Value::Text(self.text(cursor)?))
        } else {
            Ok(Value::Int(cursor.g4s()?))
        }
    }
    pub fn decode(&self, bytes: &[u8]) -> Result<Definition> {
        let mut definition = self.empty();
        let mut cursor = Cursor::new(bytes);
        let format = self.format;
        loop {
            let offset = cursor.pos();
            let tag = cursor.g1()?;
            if tag == format.end {
                definition.consumed = cursor.pos();
                return Ok(definition);
            }
            if tag == format.input_character || tag == format.output_character {
                let kind = self.legacy_types.get(&cursor.g1()?).copied();
                if tag == format.input_character {
                    definition.input_type = kind;
                } else {
                    definition.output_type = kind;
                }
            } else if tag == format.input_serial || tag == format.output_serial {
                let serial = u16::try_from(cursor.gsmart1or2()?)?;
                let kind = self.types.contains(&serial).then_some(serial);
                if tag == format.input_serial {
                    definition.input_type = kind;
                } else {
                    definition.output_type = kind;
                }
            } else if tag == format.text_default {
                definition.text_default = self.text(&mut cursor)?;
            } else if tag == format.integer_default {
                definition.integer_default = cursor.g4s()?;
            } else if tag == format.sparse_text || tag == format.sparse_integer {
                definition.wire_count = cursor.g2()?;
                for _ in 0..definition.wire_count {
                    let key = cursor.g4s()?;
                    let value = self.value(&mut cursor, tag == format.sparse_text)?;
                    definition.sparse.insert(key, value);
                }
            } else if tag == format.dense_text || tag == format.dense_integer {
                let capacity = usize::from(cursor.g2()?);
                definition.wire_count = cursor.g2()?;
                // Equal capacity resets all entries. A different positive
                // capacity retains an existing allocation and its values.
                if capacity == definition.dense.len() {
                    definition.dense.fill(None);
                } else if capacity == usize::default() {
                    definition.dense.clear();
                } else if definition.dense.is_empty() {
                    definition.dense.resize(capacity, None);
                }
                for _ in 0..definition.wire_count {
                    let key = usize::from(cursor.g2()?);
                    let value = self.value(&mut cursor, tag == format.dense_text)?;
                    *definition.dense.get_mut(key).ok_or_else(|| {
                        anyhow!("enum dense key {key} exceeds retained capacity")
                    })? = Some(value);
                }
            } else {
                definition.ignored_tags.push((offset, tag));
            }
        }
    }
}
