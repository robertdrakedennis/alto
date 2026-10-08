//! Exact-client enum inspection over checksum-verified donor definitions.
use crate::{
    corpus,
    profile::{Book, Build, digest},
};
use anyhow::{Context, Result, ensure};
use native910::js5::{ArchiveIndex, decompress};
use rs910_config::ui_enum_schema::{Codec, Definition, DefinitionFormat, Value as EnumValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const PROFILE_FORMAT: u32 = 1;
const INDEX_ARCHIVE: u32 = 255;

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireFormat {
    end: u8,
    input_character: u8,
    output_character: u8,
    text_default: u8,
    integer_default: u8,
    sparse_text: u8,
    sparse_integer: u8,
    dense_text: u8,
    dense_integer: u8,
    input_serial: u8,
    output_serial: u8,
    initial_integer: i32,
    drop_unassigned_text_bytes: bool,
}
impl From<WireFormat> for DefinitionFormat {
    fn from(value: WireFormat) -> Self {
        Self {
            end: value.end,
            input_character: value.input_character,
            output_character: value.output_character,
            text_default: value.text_default,
            integer_default: value.integer_default,
            sparse_text: value.sparse_text,
            sparse_integer: value.sparse_integer,
            dense_text: value.dense_text,
            dense_integer: value.dense_integer,
            input_serial: value.input_serial,
            output_serial: value.output_serial,
            initial_integer: value.initial_integer,
            drop_unassigned_text_bytes: value.drop_unassigned_text_bytes,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeRow {
    id: u16,
    legacy_char: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    format: u32,
    build: Build,
    client_md5: String,
    script_index_sha256: String,
    enum_archive: u32,
    enum_index_sha256: String,
    file_bits: u32,
    string_type: u16,
    definition: WireFormat,
    initial_text: String,
    type_catalog_kernel_sha256: String,
    constructor_kernel_sha256: String,
    decoder_kernel_sha256: String,
    types: Vec<TypeRow>,
}
struct Schema {
    profile: Profile,
    types: BTreeSet<u16>,
    legacy_types: BTreeMap<u8, u16>,
}
impl Schema {
    fn parse(bytes: &[u8], book: &Book) -> Result<Self> {
        let profile: Profile = serde_json::from_slice(bytes)?;
        ensure!(
            profile.format == PROFILE_FORMAT,
            "unsupported enum profile format"
        );
        ensure!(
            profile.build == book.profile.build
                && profile.client_md5 == book.profile.client_md5
                && profile.script_index_sha256 == book.profile.script_index_sha256,
            "enum profile does not match the exact script client and index"
        );
        ensure!(
            profile.file_bits < i32::BITS,
            "invalid enum partition width"
        );
        ensure!(
            !profile.type_catalog_kernel_sha256.is_empty()
                && !profile.constructor_kernel_sha256.is_empty()
                && !profile.decoder_kernel_sha256.is_empty(),
            "missing enum recording identity"
        );
        let mut types = BTreeSet::new();
        let mut legacy_types = BTreeMap::new();
        for row in &profile.types {
            ensure!(types.insert(row.id), "duplicate script type {}", row.id);
            // Registration retains the first type with each legacy character.
            legacy_types.entry(row.legacy_char).or_insert(row.id);
        }
        ensure!(
            types.contains(&profile.string_type),
            "unknown enum string type"
        );
        Ok(Self {
            profile,
            types,
            legacy_types,
        })
    }
    fn codec(&self) -> Codec<'_> {
        Codec {
            format: self.profile.definition.into(),
            initial_text: &self.profile.initial_text,
            types: &self.types,
            legacy_types: &self.legacy_types,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub profile_sha256: String,
    pub schema_sha256: String,
    pub enum_index_sha256: Option<String>,
    pub resource_sha256: String,
}

pub struct Definitions {
    pub library: rs910_config::ui_enum_resource::Library,
    build: Build,
    client_md5: String,
    profile_sha256: String,
    schema_sha256: String,
    index_sha256: Option<String>,
    hashes: BTreeMap<i32, String>,
    lengths: BTreeMap<i32, usize>,
}
impl Definitions {
    pub fn load(root: &Path, book: &Book, schema: &[u8]) -> Result<Self> {
        let schema_model = Schema::parse(schema, book)?;
        let profile = &schema_model.profile;
        corpus::load_index(root, book)?;
        let container = std::fs::read(
            root.join(INDEX_ARCHIVE.to_string())
                .join(format!("{}.dat", profile.enum_archive)),
        )?;
        ensure!(
            digest(&container) == profile.enum_index_sha256,
            "enum index does not match the exact profile"
        );
        let index = ArchiveIndex::decode(&decompress(&container)?)?;
        let codec = schema_model.codec();
        let mut definitions = BTreeMap::new();
        let mut hashes = BTreeMap::new();
        let mut lengths = BTreeMap::new();
        let file_mask = (u32::from(true) << profile.file_bits) - u32::from(true);
        for group in &index.group_id {
            ensure!(
                *group <= (i32::MAX as u32) >> profile.file_bits,
                "enum group exceeds source identity width"
            );
            for (file, bytes) in corpus::load_group(root, profile.enum_archive, &index, *group)? {
                ensure!(file <= file_mask, "enum file exceeds partition width");
                let id = i32::try_from((*group << profile.file_bits) | file)?;
                let definition = codec.decode(&bytes).with_context(|| format!("enum {id}"))?;
                definitions.insert(id, definition);
                hashes.insert(id, digest(&bytes));
                lengths.insert(id, bytes.len());
            }
        }
        Ok(Self {
            library: rs910_config::ui_enum_resource::Library {
                definitions,
                string_type: profile.string_type,
            },
            build: profile.build,
            client_md5: profile.client_md5.clone(),
            profile_sha256: book.sha256.clone(),
            schema_sha256: digest(schema),
            index_sha256: Some(profile.enum_index_sha256.clone()),
            hashes,
            lengths,
        })
    }
    pub fn identity(&self) -> Result<Identity> {
        Ok(Identity {
            profile_sha256: self.profile_sha256.clone(),
            schema_sha256: self.schema_sha256.clone(),
            enum_index_sha256: self.index_sha256.clone(),
            resource_sha256: digest(&self.library.encode_resource()?),
        })
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn authored_fixture(
        book: &Book,
        schema: &[u8],
        populate: impl FnOnce(&mut rs910_config::ui_enum_resource::Library, &Codec<'_>) -> Result<()>,
    ) -> Result<Self> {
        let schema_model = Schema::parse(schema, book)?;
        let mut library = rs910_config::ui_enum_resource::Library {
            string_type: schema_model.profile.string_type,
            definitions: BTreeMap::new(),
        };
        populate(&mut library, &schema_model.codec())?;
        Ok(Self {
            library,
            build: book.profile.build,
            client_md5: book.profile.client_md5.clone(),
            profile_sha256: book.sha256.clone(),
            schema_sha256: digest(schema),
            index_sha256: None,
            hashes: BTreeMap::new(),
            lengths: BTreeMap::new(),
        })
    }
    pub fn report(&self, selected: Option<i32>, query: Option<Query>) -> Result<Value> {
        let entries: Vec<_> = self
            .library
            .definitions
            .iter()
            .filter(|(id, _)| selected.is_none_or(|selected| selected == **id))
            .map(|(id, definition)| {
                let mut item = definition_json(definition);
                item["id"] = json!(id);
                item["sha256"] = json!(self.hashes.get(id));
                item["trailing_bytes"] = json!(
                    self.lengths
                        .get(id)
                        .map(|length| length - definition.consumed)
                );
                item
            })
            .collect();
        ensure!(
            selected.is_none() || !entries.is_empty(),
            "selected enum is absent"
        );
        let query = query
            .map(|query| -> Result<_> {
                let id = selected.context("enum query requires a selected identity")?;
                let definition = &self.library.definitions[&id];
                let value = definition.query(
                    query.input_type,
                    query.output_type,
                    query.key,
                    self.library.string_type,
                )?;
                Ok(
                    json!({"input_type":query.input_type,"output_type":query.output_type,
                "key":query.key,"value":value_json(&value)}),
                )
            })
            .transpose()?;
        Ok(
            json!({"format":PROFILE_FORMAT,"build":self.build,"client_md5":self.client_md5,
            "profile_sha256":self.profile_sha256,"schema_sha256":self.schema_sha256,
            "enum_index_sha256":self.index_sha256,"string_type":self.library.string_type,
            "enum_count":self.library.definitions.len(),"resource_sha256":digest(&self.library.encode_resource()?),
            "definitions":entries,"query":query}),
        )
    }
}
pub struct Query {
    pub input_type: i32,
    pub output_type: i32,
    pub key: i32,
}
fn value_json(value: &EnumValue) -> Value {
    match value {
        EnumValue::Int(value) => json!({"kind":"int","value":value}),
        EnumValue::Text(value) => json!({"kind":"text","value":value}),
    }
}
fn definition_json(definition: &Definition) -> Value {
    json!({"input_type":definition.input_type,"output_type":definition.output_type,
        "int_default":value_json(&EnumValue::Int(definition.integer_default)),
        "text_default":value_json(&EnumValue::Text(definition.text_default.clone())),
        "wire_count":definition.wire_count,
        "sparse":definition.sparse.iter().map(|(key,value)|json!([key,value_json(value)])).collect::<Vec<_>>(),
        "dense":definition.dense.iter().map(|value|value.as_ref().map(value_json)).collect::<Vec<_>>(),
        "ignored_tags":definition.ignored_tags,"consumed":definition.consumed})
}

#[cfg(test)]
pub(crate) fn verify(book: &Book) {
    use rs910_config::ui_enum_schema::QueryError;
    let schema = Schema::parse(
        include_bytes!("../../../revisions/950/cs2/enums.json"),
        book,
    )
    .unwrap();
    let codec = schema.codec();
    let fixture: Value = serde_json::from_slice(include_bytes!("../fixtures/enums.json")).unwrap();
    assert_eq!(fixture["client_md5"], book.profile.client_md5);
    let constructor = &fixture["constructor"];
    let empty = codec.empty();
    assert_eq!(json!(empty.integer_default), constructor["integer_default"]);
    assert_eq!(json!(empty.text_default), constructor["text_default"]);
    assert_eq!(
        empty.input_type.is_some(),
        constructor["input_type_present"].as_bool().unwrap()
    );
    assert_eq!(
        empty.output_type.is_some(),
        constructor["output_type_present"].as_bool().unwrap()
    );
    assert_eq!(
        empty.dense.len(),
        constructor["dense_capacity"].as_u64().unwrap() as usize
    );
    assert_eq!(json!(empty.wire_count), constructor["wire_count"]);
    for case in fixture["decodes"].as_array().unwrap() {
        let bytes: Vec<u8> = serde_json::from_value(case["wire"].clone()).unwrap();
        let definition = codec.decode(&bytes).unwrap();
        let library = rs910_config::ui_enum_resource::Library {
            string_type: schema.profile.string_type,
            definitions: BTreeMap::from([(i32::default(), definition.clone())]),
        };
        let resource = library.encode_resource().unwrap();
        let decoded = rs910_config::ui_enum_resource::Library::decode_resource(&resource).unwrap();
        assert_eq!(decoded, library);
        assert_eq!(decoded.encode_resource().unwrap(), resource);
        let mut appended = resource.clone();
        appended.push(u8::default());
        assert!(rs910_config::ui_enum_resource::Library::decode_resource(&appended).is_err());
        assert!(
            rs910_config::ui_enum_resource::Library::decode_resource(
                &resource[..resource.len() - usize::from(true)]
            )
            .is_err()
        );
        let mut actual = definition_json(&definition);
        actual.as_object_mut().unwrap().remove("ignored_tags");
        assert_eq!(actual, case["result"], "{}", case["name"]);
        // The end marker belongs to the generic decode loop; trailing bytes
        // remain observable. Missing markers and partial payloads are errors.
        assert!(
            codec
                .decode(&bytes[..definition.consumed - usize::from(true)])
                .is_err()
        );
    }
    let decode_value = |value: &Value| -> Option<EnumValue> {
        if value.is_null() || value["kind"] == "hole" {
            return None;
        }
        Some(match value["kind"].as_str().unwrap() {
            "int" => EnumValue::Int(serde_json::from_value(value["value"].clone()).unwrap()),
            "text" => EnumValue::Text(serde_json::from_value(value["value"].clone()).unwrap()),
            _ => unreachable!(),
        })
    };
    for case in fixture["queries"].as_array().unwrap() {
        let input = &case["input"];
        let mut definition = codec.empty();
        definition.input_type = serde_json::from_value(input["input_type"].clone()).unwrap();
        definition.output_type = serde_json::from_value(input["output_type"].clone()).unwrap();
        let Some(EnumValue::Int(default)) = decode_value(&input["int_default"]) else {
            unreachable!()
        };
        definition.integer_default = default;
        let Some(EnumValue::Text(default)) = decode_value(&input["text_default"]) else {
            unreachable!()
        };
        definition.text_default = default;
        definition.dense = input["dense"]
            .as_array()
            .unwrap()
            .iter()
            .map(decode_value)
            .collect();
        for pair in input["sparse"].as_array().unwrap() {
            let key = serde_json::from_value(pair[0].clone()).unwrap();
            definition
                .sparse
                .insert(key, decode_value(&pair[1]).unwrap());
        }
        let args: Vec<i32> = serde_json::from_value(input["args"].clone()).unwrap();
        const QUERY_ARGUMENTS: usize = 4;
        const INPUT_TYPE: usize = 0;
        const OUTPUT_TYPE: usize = 1;
        const KEY: usize = 3;
        let query = &args[args.len() - QUERY_ARGUMENTS..];
        let result = definition.query(
            query[INPUT_TYPE],
            query[OUTPUT_TYPE],
            query[KEY],
            schema.profile.string_type,
        );
        let events = case["result"]["events"].as_array().unwrap();
        if events.iter().any(|event| event[0] == "type_error") {
            assert_eq!(result, Err(QueryError::TypeMismatch), "{}", case["name"]);
        } else if events.iter().any(|event| event[0] == "abort") {
            assert_eq!(
                result,
                Err(QueryError::ValueTagMismatch),
                "{}",
                case["name"]
            );
        } else {
            match result.unwrap() {
                EnumValue::Int(value) => {
                    assert_eq!(
                        json!(value),
                        case["result"]["ints"]
                            .as_array()
                            .unwrap()
                            .last()
                            .unwrap()
                            .clone()
                    );
                }
                EnumValue::Text(value) => {
                    assert_eq!(
                        json!(value),
                        events.iter().find(|event| event[0] == "object").unwrap()[1]
                    );
                }
            }
        }
        verify_traffic(book, &args, schema.profile.string_type);
    }
}

#[cfg(test)]
fn verify_traffic(book: &Book, args: &[i32], string_type: u16) {
    use crate::{flow, semantic, wire};
    const OUTPUT_ARGUMENT: usize = 2;
    const ID_ARGUMENT: usize = 3;
    const SOURCE_LOCAL: i32 = 0;
    const ONE_LOCAL: u16 = 1;
    const INTEGER_LANE: usize = 0;
    const OBJECT_LANE: usize = 1;
    const ONE_OUTPUT: u16 = 1;
    let id = |command: &str| {
        book.profile
            .opcodes
            .iter()
            .find(|row| row.command.as_deref() == Some(command))
            .unwrap()
            .id
    };
    let mut source = wire::Script {
        name: Vec::new(),
        locals: wire::Counts::default(),
        args: wire::Counts::default(),
        code: args
            .iter()
            .map(|value| wire::Instruction {
                opcode: id("push_constant"),
                operand: wire::Operand::ConstantInt(*value),
            })
            .collect(),
        switches: Vec::new(),
    };
    source.code.push(wire::Instruction {
        opcode: id("enum"),
        operand: wire::Operand::Byte(u8::default()),
    });
    source.code.push(wire::Instruction {
        opcode: id("return"),
        operand: wire::Operand::Byte(u8::default()),
    });
    let script_id = u32::try_from(args[ID_ARGUMENT]).unwrap();
    let inspect = |source: &wire::Script| {
        let bytes = wire::encode(source, book).unwrap();
        let normalized = semantic::normalize(&bytes, book).unwrap();
        flow::inspect(&BTreeMap::from([(script_id, normalized)]), book).unwrap()
    };
    let mut expected = [u16::default(); 3];
    expected[if args[OUTPUT_ARGUMENT] == i32::from(string_type) {
        OBJECT_LANE
    } else {
        INTEGER_LANE
    }] = ONE_OUTPUT;
    let inspection = inspect(&source);
    assert!(inspection.diagnostics.is_empty());
    assert_eq!(
        inspection.enum_uses.values().next().unwrap().pushes,
        Some(expected)
    );
    source.locals.int = ONE_LOCAL;
    source.args.int = ONE_LOCAL;
    source.code[ID_ARGUMENT] = wire::Instruction {
        opcode: id("push_int_local"),
        operand: wire::Operand::Int(SOURCE_LOCAL),
    };
    let inspection = inspect(&source);
    assert!(inspection.diagnostics.is_empty());
    let usage = inspection.enum_uses.values().next().unwrap();
    assert!(usage.enum_id.is_none());
    assert_eq!(usage.pushes, Some(expected));
    source.code[OUTPUT_ARGUMENT] = source.code[ID_ARGUMENT].clone();
    let inspection = inspect(&source);
    assert!(!inspection.diagnostics.is_empty());
    assert!(
        inspection
            .enum_uses
            .values()
            .next()
            .unwrap()
            .pushes
            .is_none()
    );
}
