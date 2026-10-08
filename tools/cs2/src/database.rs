//! Exact-client database inspection over checksum-verified donor definitions.
use crate::{
    corpus,
    profile::{Book, Build, digest},
};
use anyhow::{Context, Result, ensure};
use native910::js5::{ArchiveIndex, decompress};
use rs910_config::ui_db_schema::{
    Codec, Database, DefinitionFormat, FieldLayout, FieldValue, ScriptType,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

const DATABASE_PROFILE_FORMAT: u32 = 1;
const INDEX_ARCHIVE: u32 = 255;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    end: u8,
    table_name: u8,
    table_columns: u8,
    reserved_header_bytes: usize,
    end_column: u8,
    end_columns: u8,
    column_types: u8,
    column_defaults: u8,
    column_name: u8,
    row_columns: u8,
    row_table: u8,
    drop_unassigned_text_bytes: bool,
}
impl From<Definition> for DefinitionFormat {
    fn from(value: Definition) -> Self {
        Self {
            end: value.end,
            table_name: value.table_name,
            table_columns: value.table_columns,
            reserved_header_bytes: value.reserved_header_bytes,
            end_column: value.end_column,
            end_columns: value.end_columns,
            column_types: value.column_types,
            column_defaults: value.column_defaults,
            column_name: value.column_name,
            row_columns: value.row_columns,
            row_table: value.row_table,
            drop_unassigned_text_bytes: value.drop_unassigned_text_bytes,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackedField {
    table_shift: u32,
    column_shift: u32,
    column_mask: u32,
    selector_mask: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeRow {
    id: u16,
    base: String,
    default: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    format: u32,
    build: Build,
    client_md5: String,
    script_index_sha256: String,
    config_archive: u32,
    config_index_sha256: String,
    table_group: u32,
    row_group: u32,
    definition: Definition,
    field: PackedField,
    type_catalog_kernel_sha256: String,
    types: Vec<TypeRow>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub profile_sha256: String,
    pub schema_sha256: String,
    pub config_index_sha256: Option<String>,
    pub resource_sha256: String,
}

/// Verified definitions and schemas used by inspection and imported consumers.
pub struct Definitions {
    database: Database,
    build: Build,
    client_md5: String,
    profile_sha256: String,
    script_index_sha256: String,
    schema_sha256: String,
    config_index_sha256: Option<String>,
    table_hashes: BTreeMap<u32, String>,
    row_hashes: BTreeMap<i32, String>,
}
impl Definitions {
    fn schema(bytes: &[u8], book: &Book) -> Result<(Profile, Database, DefinitionFormat)> {
        let profile: Profile = serde_json::from_slice(bytes)?;
        ensure!(
            profile.format == DATABASE_PROFILE_FORMAT,
            "unsupported database profile format"
        );
        ensure!(
            profile.build == book.profile.build
                && profile.client_md5 == book.profile.client_md5
                && profile.script_index_sha256 == book.profile.script_index_sha256,
            "database profile does not match the exact script client and index"
        );
        ensure!(
            !profile.type_catalog_kernel_sha256.is_empty(),
            "missing type catalog recording identity"
        );
        let layout = FieldLayout::new(
            profile.field.table_shift,
            profile.field.column_shift,
            profile.field.column_mask,
            profile.field.selector_mask,
        )?;
        let mut types = BTreeMap::new();
        for row in &profile.types {
            let default = match row.base.as_str() {
                "int" => FieldValue::Int(serde_json::from_value(row.default.clone())?),
                "long" => FieldValue::Long(serde_json::from_value(row.default.clone())?),
                "text" => FieldValue::Text(serde_json::from_value(row.default.clone())?),
                "coordinate" => {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Coordinate {
                        level: i32,
                        position_bits: [u32; 3],
                    }
                    let coordinate: Coordinate = serde_json::from_value(row.default.clone())?;
                    FieldValue::Coordinate {
                        level: coordinate.level,
                        position: coordinate.position_bits.map(f32::from_bits),
                    }
                }
                base => anyhow::bail!("unknown script base type {base}"),
            };
            ensure!(
                types
                    .insert(
                        row.id,
                        ScriptType {
                            base: default.base_type(),
                            default
                        }
                    )
                    .is_none(),
                "duplicate script type {}",
                row.id
            );
        }
        let definition = profile.definition.clone();
        Ok((
            profile,
            Database {
                layout,
                types,
                tables: BTreeMap::new(),
                rows: BTreeMap::new(),
            },
            definition.into(),
        ))
    }
    pub fn load(root: &Path, book: &Book, schema: &[u8]) -> Result<Self> {
        let (profile, mut database, format) = Self::schema(schema, book)?;
        corpus::load_index(root, book)?;
        let container = std::fs::read(
            root.join(INDEX_ARCHIVE.to_string())
                .join(format!("{}.dat", profile.config_archive)),
        )?;
        ensure!(
            digest(&container) == profile.config_index_sha256,
            "config index does not match the database profile"
        );
        let index = ArchiveIndex::decode(&decompress(&container)?)?;
        let codec = Codec {
            format,
            types: &database.types,
        };
        let mut table_hashes = BTreeMap::new();
        for (id, bytes) in
            corpus::load_group(root, profile.config_archive, &index, profile.table_group)?
        {
            let table = codec
                .table(&bytes)
                .with_context(|| format!("DB table {id}"))?;
            table_hashes.insert(id, digest(&bytes));
            database.tables.insert(id, table);
        }
        let mut row_hashes = BTreeMap::new();
        for (id, bytes) in
            corpus::load_group(root, profile.config_archive, &index, profile.row_group)?
        {
            let row = codec.row(&bytes).with_context(|| format!("DB row {id}"))?;
            let id = i32::try_from(id)?;
            row_hashes.insert(id, digest(&bytes));
            database.rows.insert(id, row);
        }
        Ok(Self {
            database,
            build: book.profile.build,
            client_md5: book.profile.client_md5.clone(),
            profile_sha256: book.sha256.clone(),
            script_index_sha256: book.profile.script_index_sha256.clone(),
            schema_sha256: digest(schema),
            config_index_sha256: Some(profile.config_index_sha256),
            table_hashes,
            row_hashes,
        })
    }
    /// Independently authored data for the existing replay. No cache or client
    /// bytecode is read, and this constructor is absent from production tools.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn authored_fixture(
        book: &Book,
        schema: &[u8],
        populate: impl FnOnce(&mut Database) -> Result<()>,
    ) -> Result<Self> {
        let (_, mut database, _) = Self::schema(schema, book)?;
        populate(&mut database)?;
        Ok(Self {
            database,
            build: book.profile.build,
            client_md5: book.profile.client_md5.clone(),
            profile_sha256: book.sha256.clone(),
            script_index_sha256: book.profile.script_index_sha256.clone(),
            schema_sha256: digest(schema),
            config_index_sha256: None,
            table_hashes: BTreeMap::new(),
            row_hashes: BTreeMap::new(),
        })
    }
    pub fn database(&self) -> &Database {
        &self.database
    }
    pub fn identity(&self) -> Result<Identity> {
        Ok(Identity {
            profile_sha256: self.profile_sha256.clone(),
            schema_sha256: self.schema_sha256.clone(),
            config_index_sha256: self.config_index_sha256.clone(),
            resource_sha256: digest(&self.database.encode_resource()?),
        })
    }
    pub fn flow_fields(&self) -> Result<BTreeMap<i32, native910::dataflow::TypedTraffic>> {
        use rs910_config::ui_db_schema::BaseType;
        const INTEGER_LANE: usize = 0;
        const OBJECT_LANE: usize = 1;
        const LONG_LANE: usize = 2;
        const FIELD_INPUTS: u16 = 3;
        const ONE_VALUE: u16 = 1;
        let mut fields = BTreeMap::new();
        for (table, schema) in &self.database.tables {
            for (column, schema) in schema.columns.iter().enumerate() {
                if schema.types.is_empty() {
                    continue;
                }
                for selector in 0..=schema.types.len() {
                    let Ok(field) = self.database.layout.pack(*table, column, selector) else {
                        continue;
                    };
                    let mut pushes = [u16::default(); 3];
                    let mut strings = Vec::new();
                    let mut known = true;
                    for id in self.database.field_types(field)? {
                        let Some(kind) = self.database.types.get(id) else {
                            known = false;
                            break;
                        };
                        let lane = match kind.base {
                            BaseType::Int => INTEGER_LANE,
                            BaseType::Long => LONG_LANE,
                            BaseType::Text | BaseType::Coordinate => OBJECT_LANE,
                        };
                        pushes[lane] = pushes[lane]
                            .checked_add(ONE_VALUE)
                            .context("database tuple exceeds stack arity")?;
                        if lane == OBJECT_LANE {
                            strings.push(kind.base == BaseType::Text);
                        }
                    }
                    if known {
                        fields.insert(
                            field,
                            native910::dataflow::TypedTraffic {
                                effect: native910::semantics::Effect::Fixed {
                                    pops: [FIELD_INPUTS, 0, 0],
                                    pushes,
                                },
                                nonnull_strings: strings,
                            },
                        );
                    }
                }
            }
        }
        Ok(fields)
    }
    pub fn report(&self, query: Option<Query>) -> Result<Value> {
        let tables: Vec<_> = self.database.tables.iter().map(|(id, table)| {
            let columns: Vec<_> = table.columns.iter().map(|column| json!({
                "types": column.types, "name": column.name,
                "defaults": column.defaults.iter().map(value_json).collect::<Vec<_>>()
            })).collect();
            json!({"id": id, "sha256": self.table_hashes.get(id), "name": table.name,
                "reserved_headers": table.reserved_headers, "columns": columns, "ignored_tags": table.ignored_tags})
        }).collect();
        let rows: Vec<_> = self.database.rows.iter().map(|(id, row)| {
            let columns: Vec<_> = row.columns.iter().map(|column| json!({
                "types": column.types, "values": column.values.iter().map(value_json).collect::<Vec<_>>()
            })).collect();
            json!({"id": id, "sha256": self.row_hashes.get(id), "table": row.table,
                "columns": columns, "ignored_tags": row.ignored_tags})
        }).collect();
        let query = query
            .map(|query| -> Result<_> {
                let field = self.database.layout.unpack(query.field);
                let mut values = Vec::new();
                self.database
                    .visit_field(query.row, query.field, query.index, |value| {
                        values.push(value_json(value));
                        Ok(())
                    })?;
                Ok(
                    json!({"row":query.row,"field":query.field,"index":query.index,
                "table":field.table,"column":field.column,"selector":field.selector,
                "types":self.database.field_types(query.field)?,
                "count":self.database.field_count(query.row,query.field),"values":values}),
                )
            })
            .transpose()?;
        Ok(json!({"build":self.build,"client_md5":self.client_md5,
            "profile_sha256":self.profile_sha256,"schema_sha256":self.schema_sha256,
            "script_index_sha256":self.script_index_sha256,"config_index_sha256":self.config_index_sha256,
            "tables":tables,"rows":rows,"query":query}))
    }
}

#[derive(Clone, Copy)]
pub struct Query {
    pub row: i32,
    pub field: i32,
    pub index: i32,
}

fn value_json(value: &FieldValue) -> Value {
    match value {
        FieldValue::Int(value) => json!({"kind":"int","value":value}),
        FieldValue::Long(value) => json!({"kind":"long","value":value}),
        FieldValue::Text(value) => json!({"kind":"text","value":value}),
        FieldValue::Coordinate { level, position } => json!({"kind":"coordinate","level":level,
            "position_bits":position.map(f32::to_bits)}),
    }
}

#[cfg(test)]
pub(crate) fn verify(book: &Book) {
    use rs910_config::ui_db_schema::{Column, Row, RowColumn, Table};
    let schema = include_bytes!("../../../revisions/950/cs2/database.json");
    let fixture: Value =
        serde_json::from_slice(include_bytes!("../fixtures/database.json")).unwrap();
    let (_, mut database, format) = Definitions::schema(schema, book).unwrap();
    assert_eq!(fixture["client_md5"], book.profile.client_md5);
    let decode_value = |value: &Value| -> FieldValue {
        match value["kind"].as_str().unwrap() {
            "int" => FieldValue::Int(serde_json::from_value(value["value"].clone()).unwrap()),
            "long" => FieldValue::Long(serde_json::from_value(value["value"].clone()).unwrap()),
            "text" => FieldValue::Text(serde_json::from_value(value["value"].clone()).unwrap()),
            "coordinate" => {
                let bits: [u32; 3] =
                    serde_json::from_value(value["position_bits"].clone()).unwrap();
                FieldValue::Coordinate {
                    level: value["level"].as_i64().unwrap() as i32,
                    position: bits.map(f32::from_bits),
                }
            }
            _ => unreachable!(),
        }
    };
    for case in fixture["cases"].as_array().unwrap() {
        let input = &case["input"];
        let table_id = input["table"].as_u64().unwrap() as u32;
        let row_id = input["row"].as_i64().unwrap() as i32;
        let column = input["column"].as_u64().unwrap() as usize;
        let mut table = Table::default();
        let mut row = Row::default();
        table
            .columns
            .resize_with(column + usize::from(true), Column::default);
        row.columns
            .resize_with(column + usize::from(true), RowColumn::default);
        let types: Vec<u16> = serde_json::from_value(input["types"].clone()).unwrap();
        table.columns[column].types = types.clone();
        table.columns[column].defaults = input["defaults"]
            .as_array()
            .unwrap()
            .iter()
            .map(decode_value)
            .collect();
        row.columns[column].values = input["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(decode_value)
            .collect();
        database.tables.insert(table_id, table);
        database.rows.insert(row_id, row);
        let packed = case["packed_field"].as_i64().unwrap() as i32;
        let unknown = input["unknown_base"]
            .as_u64()
            .map(|index| types[index as usize]);
        let saved = unknown.map(|serial| (serial, database.types.remove(&serial).unwrap()));
        let resource = database.encode_resource().unwrap();
        let round_trip = Database::decode_resource(&resource).unwrap();
        assert_eq!(resource, round_trip.encode_resource().unwrap());
        database = round_trip;
        let mut ints = Vec::new();
        let mut longs = Vec::new();
        let result = database.visit_field(
            row_id,
            packed,
            input["index"].as_i64().unwrap() as i32,
            |value| {
                match value {
                    FieldValue::Int(value) => ints.push(*value),
                    FieldValue::Long(value) => longs.push(*value),
                    _ => unreachable!(),
                }
                Ok(())
            },
        );
        assert_eq!(
            result.is_err(),
            case["field"]["failed"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            serde_json::to_value(ints).unwrap(),
            case["field"]["ints"],
            "{}",
            case["name"]
        );
        assert_eq!(
            serde_json::to_value(longs).unwrap(),
            case["field"]["longs"],
            "{}",
            case["name"]
        );
        assert_eq!(
            database.field_count(row_id, packed) as i64,
            case["count"]["ints"][0].as_i64().unwrap(),
            "{}",
            case["name"]
        );
        if let Some((serial, script_type)) = saved {
            database.types.insert(serial, script_type);
        }
    }
    let codec = Codec {
        format,
        types: &database.types,
    };
    let data = &fixture["codec"];
    let table_bytes: Vec<u8> = serde_json::from_value(data["table"].clone()).unwrap();
    let row_bytes: Vec<u8> = serde_json::from_value(data["row"].clone()).unwrap();
    let table = codec.table(&table_bytes).unwrap();
    let row = codec.row(&row_bytes).unwrap();
    let expected: Vec<_> = data["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(decode_value)
        .collect();
    assert_eq!(
        table.name.as_ref().unwrap(),
        data["table_name"].as_str().unwrap()
    );
    assert_eq!(
        table.columns[0].name.as_ref().unwrap(),
        data["column_name"].as_str().unwrap()
    );
    assert_eq!(
        serde_json::to_value(&table.reserved_headers[0]).unwrap(),
        data["reserved"]
    );
    assert_eq!(
        serde_json::to_value(&table.columns[0].types).unwrap(),
        data["types"]
    );
    assert_eq!(table.columns[0].defaults, expected);
    assert_eq!(row.columns[0].values, expected);
    assert_eq!(
        row.table.unwrap(),
        data["row_table"].as_i64().unwrap() as i32
    );
    assert_eq!(
        table.columns[1].types[0],
        data["unknown_type"].as_u64().unwrap() as u16
    );
    assert_eq!(row.columns[1].types, table.columns[1].types);
    assert!(row.columns[1].values.is_empty());
    let fixture_table = data["row_table"].as_i64().unwrap() as u32;
    database.tables.insert(fixture_table, table.clone());
    database.rows.insert(fixture_table as i32, row.clone());
    let image = database.encode_resource().unwrap();
    assert_eq!(
        image,
        Database::decode_resource(&image)
            .unwrap()
            .encode_resource()
            .unwrap()
    );
    assert!(Database::decode_resource(&image[..image.len() - usize::from(true)]).is_err());
    let mut trailing = image.clone();
    trailing.push(u8::default());
    assert!(Database::decode_resource(&trailing).is_err());
    let reset_table: Vec<u8> = serde_json::from_value(data["reset_table"].clone()).unwrap();
    let reset_row: Vec<u8> = serde_json::from_value(data["reset_row"].clone()).unwrap();
    assert!(codec.table(&reset_table).unwrap().columns.iter().all(
        |column| column.types.is_empty() && column.defaults.is_empty() && column.name.is_none()
    ));
    let reset_row = codec.row(&reset_row).unwrap();
    assert_eq!(reset_row.table, row.table);
    assert!(
        reset_row
            .columns
            .iter()
            .all(|column| column.types.is_empty() && column.values.is_empty())
    );

    assert!(
        codec
            .table(&table_bytes[..table_bytes.len() - usize::from(true)])
            .is_err()
    );
    assert!(
        codec
            .row(&row_bytes[..row_bytes.len() - usize::from(true)])
            .is_err()
    );
    let mut other_book = book.profile.clone();
    other_book.build.minor += u32::from(true);
    let other_book = Book::parse(&serde_json::to_vec(&other_book).unwrap()).unwrap();
    assert!(Definitions::schema(schema, &other_book).is_err());
}
