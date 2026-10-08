//! Enum, struct and param configs and the script commands that read them.
//! Cache data is installed over the type defaults. Duplicate enum keys keep
//! the final value but retain the original value count; struct parameters keep
//! the first duplicate entry.
use crate::{cache::Pack, utf16_text::Text};
use anyhow::{Context, Result};
use native910::config::{self as wire, ConfigValue, EnumValues};
use std::{cell::OnceCell, collections::BTreeMap};
#[derive(Clone, Default)]
pub struct Param {
    pub string: bool,
    pub integer: i32,
    pub text: Option<Text>,
}
impl Param {
    /// Decode a param type; legacy type registration is shared with enum types.
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let p = native910::config::decode_param(bytes)?;
        let kind = if let Some(v) = p.kind {
            crate::ui_configs::serial(v)
        } else if let Some(v) = p.kind_legacy {
            crate::ui_legacy_types::legacy(v)?
        } else {
            None
        };
        Ok(Self {
            string: kind == Some(36),
            integer: p.default_int.unwrap_or(0),
            text: p.default_string.map(|s| s.encode_utf16().collect()),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scalar {
    Int(i32),
    String(Text),
}
impl From<ConfigValue> for Scalar {
    fn from(v: ConfigValue) -> Self {
        match v {
            ConfigValue::Int(v) => Self::Int(v),
            ConfigValue::Str(v) => Self::String(v.encode_utf16().collect()),
        }
    }
}
#[derive(Clone, Debug)]
pub enum Storage {
    Empty,
    Sparse(BTreeMap<i32, Scalar>),
    Dense(Vec<Option<Scalar>>),
}
#[derive(Clone, Debug)]
pub struct Enum {
    pub input: Option<i32>,
    pub output: Option<i32>,
    pub default_int: i32,
    pub default_string: Text,
    pub count: i32,
    pub storage: Storage,
    pub reverse: OnceCell<BTreeMap<Scalar, Vec<i32>>>,
}
impl Default for Enum {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            default_int: 0,
            default_string: "null".encode_utf16().collect(),
            count: 0,
            storage: Storage::Empty,
            reverse: OnceCell::new(),
        }
    }
}
/// Smart serial IDs resolve through the serializable enums, never through their
/// base-type default. The existing table includes every serial id.
pub fn serial(id: u16) -> Option<i32> {
    u8::try_from(id)
        .ok()
        .filter(|id| crate::types910::script_types::script_type(*id).is_some())
        .map(i32::from)
}
fn kind(smart: Option<u16>, legacy: Option<u8>) -> Result<Option<i32>> {
    if let Some(v) = smart {
        Ok(serial(v))
    } else if let Some(v) = legacy {
        crate::ui_legacy_types::legacy(v)
    } else {
        Ok(None)
    }
}
impl Enum {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let e = wire::decode_enum(bytes)?;
        let mut out = Self {
            input: kind(e.input_type, e.input_legacy)?,
            output: kind(e.output_type, e.output_legacy)?,
            default_int: e.default_int.unwrap_or(0),
            default_string: e
                .default_string
                .unwrap_or_else(|| "null".into())
                .encode_utf16()
                .collect(),
            ..Self::default()
        };
        out.storage = match e.values {
            None => Storage::Empty,
            Some(EnumValues::SparseInt(rows) | EnumValues::SparseString(rows)) => {
                out.count = rows.len() as i32;
                Storage::Sparse(rows.into_iter().map(|r| (r.key, r.value.into())).collect())
            }
            Some(
                EnumValues::DenseInt { capacity, slots }
                | EnumValues::DenseString { capacity, slots },
            ) => {
                out.count = slots.len() as i32;
                let mut a = vec![None; capacity as usize];
                for s in slots {
                    *a.get_mut(s.index as usize)
                        .context("dense enum index out of range")? = Some(s.value.into());
                }
                Storage::Dense(a)
            }
        };
        Ok(out)
    }
    pub fn entries(&self) -> Vec<(i32, &Scalar)> {
        match &self.storage {
            Storage::Empty => vec![],
            Storage::Sparse(m) => m.iter().map(|(&k, v)| (k, v)).collect(),
            Storage::Dense(a) => a
                .iter()
                .enumerate()
                .filter_map(|(i, v)| v.as_ref().map(|v| (i as i32, v)))
                .collect(),
        }
    }
    pub fn value(&self, key: i32) -> Option<&Scalar> {
        match &self.storage {
            Storage::Empty => None,
            Storage::Sparse(m) => m.get(&key),
            Storage::Dense(a) => usize::try_from(key)
                .ok()
                .and_then(|i| a.get(i))
                .and_then(Option::as_ref),
        }
    }
    pub fn integer(&self, key: i32) -> Result<i32> {
        match self.value(key) {
            None => Ok(self.default_int),
            Some(Scalar::Int(v)) => Ok(*v),
            _ => anyhow::bail!("enum value is not Integer"),
        }
    }
    pub fn string(&self, key: i32) -> Result<&[u16]> {
        match self.value(key) {
            None => Ok(&self.default_string),
            Some(Scalar::String(v)) => Ok(v),
            _ => anyhow::bail!("enum value is not String"),
        }
    }
    pub fn reverse(&self, value: &Scalar) -> Option<&[i32]> {
        if self.count == 0 {
            return None;
        }
        self.reverse
            .get_or_init(|| {
                let mut m: BTreeMap<Scalar, Vec<i32>> = BTreeMap::new();
                for (k, v) in self.entries() {
                    m.entry(v.clone()).or_default().push(k);
                }
                m
            })
            .get(value)
            .map(Vec::as_slice)
    }
}
#[derive(Clone, Debug, Default)]
pub struct Struct {
    pub params: Vec<(i32, Scalar)>,
}
impl Struct {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            params: wire::decode_struct(bytes)?
                .params
                .into_iter()
                .map(|r| (r.param as i32, r.value.into()))
                .collect(),
        })
    }
    pub fn integer(&self, key: i32, default: i32) -> Result<i32> {
        match self.params.iter().find(|p| p.0 == key).map(|p| &p.1) {
            None => Ok(default),
            Some(Scalar::Int(v)) => Ok(*v),
            _ => anyhow::bail!("struct parameter is not an integer"),
        }
    }
    pub fn string<'a>(&'a self, key: i32, default: Option<&'a [u16]>) -> Result<Option<&'a [u16]>> {
        match self.params.iter().find(|p| p.0 == key).map(|p| &p.1) {
            None => Ok(default),
            Some(Scalar::String(v)) => Ok(Some(v)),
            _ => anyhow::bail!("struct parameter is not a string"),
        }
    }
}
#[derive(Default)]
pub struct Configs {
    pub enums: BTreeMap<i32, Enum>,
    pub structs: BTreeMap<i32, Struct>,
    pub struct_count: i32,
    empty_enum: Enum,
    empty_struct: Struct,
}
impl Configs {
    pub fn load(pack: &Pack) -> Result<Self> {
        let mut s = Self::default();
        let (_, enums) = records(pack, "enum.config", 8)?;
        for (id, b) in enums {
            s.enums
                .insert(id, Enum::decode(&b).with_context(|| format!("enum {id}"))?);
        }
        let (count, structs) = records(pack, "struct.config", 5)?;
        s.struct_count = count;
        for (id, b) in structs {
            s.structs.insert(
                id,
                Struct::decode(&b).with_context(|| format!("struct {id}"))?,
            );
        }
        Ok(s)
    }
    /// An absent file yields a default enum.
    pub fn enumeration(&self, id: i32) -> &Enum {
        self.enums.get(&id).unwrap_or(&self.empty_enum)
    }
    /// Negative IDs share a default; positive IDs
    /// index the declared archive capacity and fail when outside that array.
    pub fn structure(&self, id: i32) -> Result<&Struct> {
        anyhow::ensure!(
            id < self.struct_count,
            "struct ID outside preloaded capacity"
        );
        Ok(self.structs.get(&id).unwrap_or(&self.empty_struct))
    }
    pub fn dispatch(
        &self,
        params: &BTreeMap<i32, Param>,
        command: &str,
        i: &mut Vec<i32>,
        s: &mut Vec<String>,
    ) -> Option<Result<native910::vm::Value>> {
        let &(_, ni, ns) = COMMANDS.iter().find(|c| c.0 == command)?;
        Some((|| {
            anyhow::ensure!(
                i.len() >= ni && s.len() >= ns,
                "config command stack underflow"
            );
            let i = i.split_off(i.len() - ni);
            let s = s.split_off(s.len() - ns);
            use native910::vm::Value as V;
            let string = |v: &[u16]| -> Result<V> { Ok(V::Str(String::from_utf16(v)?)) };
            if command == "struct_param" {
                let fallback = Param::default();
                let p = params.get(&i[1]).unwrap_or(&fallback);
                let st = self.structure(i[0])?;
                return if p.string {
                    match st.string(i[1], p.text.as_deref())? {
                        Some(value) => string(value),
                        // The stock client leaves a null object on the stack;
                        // this VM lane is string-backed, and the string join
                        // renders that value as the literal "null" when a
                        // script joins it. Preserve that observable result
                        // instead of aborting the host at the config lookup.
                        None => Ok(V::Str("null".into())),
                    }
                } else {
                    Ok(V::Int(st.integer(i[1], p.integer)?))
                };
            }
            if command == "enum_getoutputcount" {
                return Ok(V::Int(self.enumeration(i[0]).count));
            }
            if command == "enum_string" {
                return string(self.enumeration(i[0]).string(i[1])?);
            }
            if command == "_enum" {
                let e = self.enumeration(i[2]);
                anyhow::ensure!(
                    e.input == Some(i[0]) && e.output == Some(i[1]),
                    "enum input/output type mismatch"
                );
                return if i[1] == 36 {
                    string(e.string(i[3])?)
                } else {
                    Ok(V::Int(e.integer(i[3])?))
                };
            }
            let is_string = command.ends_with("_string");
            let index = command.starts_with("enum_getreverseindex");
            let (id, out_type, in_type, value, ordinal) = if is_string {
                if index {
                    (
                        i[1],
                        36,
                        Some(i[0]),
                        Scalar::String(s[0].encode_utf16().collect()),
                        i[2],
                    )
                } else {
                    (
                        i[0],
                        36,
                        None,
                        Scalar::String(s[0].encode_utf16().collect()),
                        0,
                    )
                }
            } else if index {
                (i[2], i[0], Some(i[1]), Scalar::Int(i[3]), i[4])
            } else {
                (i[1], i[0], None, Scalar::Int(i[2]), 0)
            };
            anyhow::ensure!(id != -1, "enum sentinel ID");
            let e = self.enumeration(id);
            anyhow::ensure!(
                e.output == Some(out_type) && in_type.is_none_or(|t| e.input == Some(t)),
                "enum reverse type mismatch"
            );
            let keys = e.reverse(&value);
            let result = if index {
                *keys
                    .and_then(|a| usize::try_from(ordinal).ok().and_then(|n| a.get(n)))
                    .context("enum reverse index")?
            } else if command.starts_with("enum_hasoutput") {
                keys.is_some() as i32
            } else {
                keys.map_or(0, |a| a.len() as i32)
            };
            Ok(V::Int(result))
        })())
    }
}
pub const COMMANDS: &[(&str, usize, usize)] = &[
    ("enum_string", 2, 0),
    ("_enum", 4, 0),
    ("enum_hasoutput", 3, 0),
    ("enum_hasoutput_string", 1, 1),
    ("enum_getoutputcount", 1, 0),
    ("enum_getreversecount", 3, 0),
    ("enum_getreversecount_string", 1, 1),
    ("enum_getreverseindex", 5, 0),
    ("enum_getreverseindex_string", 3, 1),
    ("struct_param", 2, 0),
];
/// Read every file of a multi-group config archive, keyed by config id
/// (`group << bits | file`). Mirrors the existing shared config
/// loader, exported here for enum/struct providers and their raw-cache oracle.
pub fn records(pack: &Pack, name: &str, bits: u32) -> Result<(i32, BTreeMap<i32, Vec<u8>>)> {
    let index = pack.read_archive_index(name)?;
    let last = *index.group_id.last().context("empty config archive")?;
    let n = index.file_count_for_group(last)?;
    let count = (last << bits)
        + if n == 0 {
            0
        } else {
            index.file_id_for_group_index(last, n - 1)? + 1
        };
    let mut records = BTreeMap::new();
    for group in index.group_id.iter().copied() {
        if index.file_count_for_group(group)? == 0 {
            continue;
        }
        for (file, b) in pack.read_group(name, group)? {
            anyhow::ensure!(file < (1 << bits), "config file outside group size");
            records.insert(((group << bits) | file) as i32, b);
        }
    }
    Ok((count as i32, records))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native910::vm::Value;

    #[test]
    fn struct_string_param_null_default_reaches_the_vm_as_null_text() {
        let mut configs = Configs {
            struct_count: 1,
            ..Default::default()
        };
        configs.structs.insert(0, Struct::default());
        let mut params = BTreeMap::new();
        params.insert(
            7,
            Param {
                string: true,
                integer: 0,
                text: None,
            },
        );
        let mut ints = vec![0, 7];
        let mut strings = Vec::new();
        let result = configs
            .dispatch(&params, "struct_param", &mut ints, &mut strings)
            .expect("struct_param owner")
            .expect("null string default is a valid result");
        assert_eq!(result, Value::Str("null".into()));
        assert!(ints.is_empty());
        assert!(strings.is_empty());
    }
}
