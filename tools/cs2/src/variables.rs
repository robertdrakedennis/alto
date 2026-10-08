//! Exact-client variable definitions and script dependencies. A donor identity
//! is a source reference; it is never an implicit binding to live target state.
use crate::{
    corpus, import910,
    profile::{Book, Build, digest},
    semantic::Argument,
};
use anyhow::{Context, Result, ensure};
use native910::js5::{ArchiveIndex, decompress};
use rs910_config::ui_bytes::Cursor;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

const PROFILE_FORMAT: u32 = 1;
const INDEX_ARCHIVE: u32 = 255;

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct VariableFormat {
    end: u8,
    data_type: u8,
    lifetime: u8,
    transmit_level: u8,
    client_code: u8,
    clear_legacy: u8,
    flag: u8,
    initial_lifetime: u8,
    initial_transmit_level: u8,
    initial_client_code: u16,
    initial_legacy: bool,
    initial_flag: bool,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct BitFormat {
    end: u8,
    base: u8,
    range: u8,
    flag: u8,
    initial_start: u8,
    initial_end: u8,
    initial_flag: bool,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Getter {
    pub shift_mask: u32,
    pub range_end_bias: u32,
}
impl Getter {
    pub fn project(self, base: i32, start: u8, end: u8) -> i32 {
        let width = u32::from(end)
            .wrapping_add(self.range_end_bias)
            .wrapping_sub(u32::from(start))
            & self.shift_mask;
        let mask = u32::from(true)
            .wrapping_shl(width)
            .wrapping_sub(u32::from(true));
        base.wrapping_shr(u32::from(start) & self.shift_mask) & mask as i32
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Domain {
    id: u8,
    name: String,
    group: u32,
    legacy_types: Vec<u16>,
    legacy_default: Value,
    bit_getter_reviewed: bool,
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
    varbit_group: u32,
    variable_definition: VariableFormat,
    bit_definition: BitFormat,
    getter: Getter,
    domains: Vec<Domain>,
    type_catalog_kernel_sha256: String,
    kernels: BTreeMap<String, String>,
    types: Vec<TypeRow>,
}
struct Schema {
    profile: Profile,
    domains: BTreeMap<u8, usize>,
    types: BTreeMap<u16, usize>,
}
impl Schema {
    fn parse(bytes: &[u8], book: &Book) -> Result<Self> {
        let profile: Profile = serde_json::from_slice(bytes)?;
        ensure!(
            profile.format == PROFILE_FORMAT,
            "unsupported variable profile format"
        );
        ensure!(
            profile.build == book.profile.build
                && profile.client_md5 == book.profile.client_md5
                && profile.script_index_sha256 == book.profile.script_index_sha256,
            "variable profile does not match the exact script client and index"
        );
        ensure!(
            profile.getter.shift_mask == i32::BITS - u32::from(true),
            "unsupported variable word width"
        );
        ensure!(
            !profile.type_catalog_kernel_sha256.is_empty()
                && !profile.kernels.is_empty()
                && profile.kernels.values().all(|hash| !hash.is_empty()),
            "missing variable recording identity"
        );
        let mut domains = BTreeMap::new();
        for (index, domain) in profile.domains.iter().enumerate() {
            ensure!(
                domains.insert(domain.id, index).is_none(),
                "duplicate variable domain"
            );
        }
        let mut types = BTreeMap::new();
        for (index, row) in profile.types.iter().enumerate() {
            ensure!(
                types.insert(row.id, index).is_none(),
                "duplicate script type"
            );
        }
        Ok(Self {
            profile,
            domains,
            types,
        })
    }
    fn variable(&self, bytes: &[u8]) -> Result<Variable> {
        let f = self.profile.variable_definition;
        let mut result = Variable {
            data_type: None,
            lifetime: f.initial_lifetime,
            transmit_level: f.initial_transmit_level,
            client_code: f.initial_client_code,
            legacy: f.initial_legacy,
            flag: f.initial_flag,
            ignored_tags: Vec::new(),
            consumed: usize::default(),
        };
        let mut p = Cursor::new(bytes);
        loop {
            let offset = p.pos();
            let tag = p.g1()?;
            if tag == f.end {
                result.consumed = p.pos();
                return Ok(result);
            }
            if tag == f.data_type {
                let id = u16::from(p.g1()?);
                result.data_type = self.types.contains_key(&id).then_some(id);
            } else if tag == f.lifetime {
                result.lifetime = p.g1()?;
            } else if tag == f.transmit_level {
                result.transmit_level = p.g1()?;
            } else if tag == f.client_code {
                result.client_code = p.g2()?;
            } else if tag == f.clear_legacy {
                result.legacy = false;
            } else if tag == f.flag {
                result.flag = true;
            } else {
                result.ignored_tags.push((offset, tag));
            }
        }
    }
    fn bit(&self, bytes: &[u8], lookup: impl Fn(Base) -> Lookup) -> BitDecode {
        let f = self.profile.bit_definition;
        let mut definition = Bit {
            binding: None,
            base_references: Vec::new(),
            start: f.initial_start,
            end: f.initial_end,
            flag: f.initial_flag,
            ignored_tags: Vec::new(),
            consumed: usize::default(),
        };
        let mut p = Cursor::new(bytes);
        let decoded = (|| -> Result<()> {
            loop {
                let offset = p.pos();
                let tag = p.g1()?;
                if tag == f.end {
                    return Ok(());
                }
                if tag == f.base {
                    let base = Base {
                        domain: p.g1()?,
                        id: p.gsmart2or4s()?,
                    };
                    definition.base_references.push(base);
                    match lookup(base) {
                        Lookup::Resolved => definition.binding = Some(base),
                        Lookup::UnknownDomain => {}
                        Lookup::MissingBase => {
                            definition.binding = None;
                            anyhow::bail!("base variable missing");
                        }
                    }
                } else if tag == f.range {
                    definition.start = p.g1()?;
                    definition.end = p.g1()?;
                } else if tag == f.flag {
                    definition.flag = true;
                } else {
                    definition.ignored_tags.push((offset, tag));
                }
            }
        })();
        definition.consumed = p.pos();
        BitDecode {
            definition,
            error: decoded.err().map(|error| error.to_string()),
        }
    }
    fn default_selection(&self, domain: u8, variable: &Variable) -> Option<(&str, Value)> {
        let domain = &self.profile.domains[*self.domains.get(&domain)?];
        let id = variable.data_type?;
        if variable.legacy && domain.legacy_types.contains(&id) {
            Some(("legacy", domain.legacy_default.clone()))
        } else {
            Some((
                "type_default",
                self.profile.types[*self.types.get(&id)?].default.clone(),
            ))
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Variable {
    pub data_type: Option<u16>,
    pub lifetime: u8,
    pub transmit_level: u8,
    pub client_code: u16,
    pub legacy: bool,
    pub flag: bool,
    pub ignored_tags: Vec<(usize, u8)>,
    pub consumed: usize,
}
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct Base {
    pub domain: u8,
    pub id: i32,
}
#[derive(Clone, Debug, Serialize)]
pub struct Bit {
    pub binding: Option<Base>,
    pub base_references: Vec<Base>,
    pub start: u8,
    pub end: u8,
    pub flag: bool,
    pub ignored_tags: Vec<(usize, u8)>,
    pub consumed: usize,
}
#[derive(Clone, Debug, Serialize)]
pub struct BitDecode {
    pub definition: Bit,
    pub error: Option<String>,
}
enum Lookup {
    Resolved,
    UnknownDomain,
    MissingBase,
}
struct Entry<T> {
    value: T,
    sha256: String,
    bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub profile_sha256: String,
    pub schema_sha256: String,
    pub config_index_sha256: Option<String>,
    pub definitions_sha256: String,
}
pub struct BaseContract {
    pub domain_name: String,
    pub default: i32,
}
pub struct BitContract {
    pub base: Base,
    pub domain_name: String,
    pub shift: u8,
    pub mask: i32,
    pub default: i32,
}
/// Source config identities are preserved at their full indexed width.
pub struct Definitions {
    schema: Schema,
    profile_sha256: String,
    schema_sha256: String,
    variables: BTreeMap<Base, Entry<Variable>>,
    bits: BTreeMap<u32, Entry<BitDecode>>,
    index_sha256: Option<String>,
}
impl Definitions {
    pub fn load(root: &Path, book: &Book, bytes: &[u8]) -> Result<Self> {
        let schema = Schema::parse(bytes, book)?;
        corpus::load_index(root, book)?;
        let p = &schema.profile;
        let container = std::fs::read(
            root.join(INDEX_ARCHIVE.to_string())
                .join(format!("{}.dat", p.config_archive)),
        )?;
        ensure!(
            digest(&container) == p.config_index_sha256,
            "config index does not match the variable profile"
        );
        let index = ArchiveIndex::decode(&decompress(&container)?)?;
        let mut variables = BTreeMap::new();
        for domain in &p.domains {
            for (id, bytes) in corpus::load_group(root, p.config_archive, &index, domain.group)? {
                let base = Base {
                    domain: domain.id,
                    id: i32::try_from(id)?,
                };
                let value = schema
                    .variable(&bytes)
                    .with_context(|| format!("variable {}:{id}", domain.name))?;
                variables.insert(
                    base,
                    Entry {
                        value,
                        sha256: digest(&bytes),
                        bytes: bytes.len(),
                    },
                );
            }
        }
        let mut bits = BTreeMap::new();
        for (id, bytes) in corpus::load_group(root, p.config_archive, &index, p.varbit_group)? {
            let value = schema.bit(&bytes, |base| {
                if !schema.domains.contains_key(&base.domain) {
                    Lookup::UnknownDomain
                } else if variables.contains_key(&base) {
                    Lookup::Resolved
                } else {
                    Lookup::MissingBase
                }
            });
            bits.insert(
                id,
                Entry {
                    value,
                    sha256: digest(&bytes),
                    bytes: bytes.len(),
                },
            );
        }
        Ok(Self {
            index_sha256: Some(schema.profile.config_index_sha256.clone()),
            schema,
            profile_sha256: book.sha256.clone(),
            schema_sha256: digest(bytes),
            variables,
            bits,
        })
    }
    pub fn identity(&self) -> Result<Identity> {
        let bases: Vec<_> = self
            .variables
            .iter()
            .map(|(base, entry)| (base, &entry.sha256))
            .collect();
        let bits: Vec<_> = self
            .bits
            .iter()
            .map(|(id, entry)| (id, &entry.sha256))
            .collect();
        Ok(Identity {
            profile_sha256: self.profile_sha256.clone(),
            schema_sha256: self.schema_sha256.clone(),
            config_index_sha256: self.index_sha256.clone(),
            definitions_sha256: digest(&serde_json::to_vec(&(bases, bits))?),
        })
    }
    pub fn base_contract(&self, base: Base) -> Result<BaseContract> {
        let variable = &self
            .variables
            .get(&base)
            .context("source variable is absent")?
            .value;
        let domain = &self.schema.profile.domains[*self
            .schema
            .domains
            .get(&base.domain)
            .context("source variable domain is absent")?];
        let kind = variable
            .data_type
            .and_then(|id| self.schema.types.get(&id))
            .map(|index| &self.schema.profile.types[*index]);
        ensure!(
            kind.is_some_and(|kind| kind.base == "int"),
            "source variable has no integer contract"
        );
        let (_, default) = self
            .schema
            .default_selection(base.domain, variable)
            .context("source variable default is unresolved")?;
        Ok(BaseContract {
            domain_name: domain.name.clone(),
            default: serde_json::from_value(default)?,
        })
    }
    /// Only reviewed getters become executable contracts; source identities
    /// and declaration payloads remain independent of target revision IDs.
    pub fn bit_contract(&self, id: u32) -> Result<BitContract> {
        let decoded = &self.bits.get(&id).context("source varbit is absent")?.value;
        ensure!(decoded.error.is_none(), "source varbit decode failed");
        let bit = &decoded.definition;
        let base = bit
            .binding
            .context("source varbit has no resolved base variable")?;
        let domain = &self.schema.profile.domains[self.schema.domains[&base.domain]];
        ensure!(
            domain.bit_getter_reviewed,
            "source domain bit getter is unreviewed"
        );
        let variable = &self.variables[&base].value;
        let kind = variable
            .data_type
            .and_then(|id| self.schema.types.get(&id))
            .map(|index| &self.schema.profile.types[*index]);
        ensure!(
            kind.is_some_and(|kind| kind.base == "int"),
            "source bit base has no integer contract"
        );
        let (_, default) = self
            .schema
            .default_selection(base.domain, variable)
            .context("source variable default is unresolved")?;
        let getter = self.schema.profile.getter;
        let width = u32::from(bit.end)
            .wrapping_add(getter.range_end_bias)
            .wrapping_sub(u32::from(bit.start))
            & getter.shift_mask;
        Ok(BitContract {
            base,
            domain_name: domain.name.clone(),
            shift: u8::try_from(u32::from(bit.start) & getter.shift_mask)?,
            mask: u32::from(true)
                .wrapping_shl(width)
                .wrapping_sub(u32::from(true)) as i32,
            default: serde_json::from_value(default)?,
        })
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn authored_fixture(
        book: &Book,
        bytes: &[u8],
        bases: &[(Base, Vec<u8>)],
        bits: &[(u32, Vec<u8>)],
    ) -> Result<Self> {
        let schema = Schema::parse(bytes, book)?;
        let mut variables = BTreeMap::new();
        for (base, wire) in bases {
            ensure!(
                schema.domains.contains_key(&base.domain),
                "unknown authored source domain"
            );
            ensure!(
                variables
                    .insert(
                        *base,
                        Entry {
                            value: schema.variable(wire)?,
                            sha256: digest(wire),
                            bytes: wire.len()
                        }
                    )
                    .is_none(),
                "duplicate authored variable"
            );
        }
        let mut definitions = BTreeMap::new();
        for (id, wire) in bits {
            let value = schema.bit(wire, |base| {
                if !schema.domains.contains_key(&base.domain) {
                    Lookup::UnknownDomain
                } else if variables.contains_key(&base) {
                    Lookup::Resolved
                } else {
                    Lookup::MissingBase
                }
            });
            ensure!(
                definitions
                    .insert(
                        *id,
                        Entry {
                            value,
                            sha256: digest(wire),
                            bytes: wire.len()
                        }
                    )
                    .is_none(),
                "duplicate authored varbit"
            );
        }
        Ok(Self {
            schema,
            profile_sha256: book.sha256.clone(),
            schema_sha256: digest(bytes),
            index_sha256: None,
            variables,
            bits: definitions,
        })
    }
    fn variable_report(&self, base: Base) -> Value {
        let Some(entry) = self.variables.get(&base) else {
            return json!({"source":base,"definition_present":false});
        };
        let domain = &self.schema.profile.domains[self.schema.domains[&base.domain]];
        let kind = entry
            .value
            .data_type
            .and_then(|id| self.schema.types.get(&id))
            .map(|index| &self.schema.profile.types[*index]);
        let default = self.schema.default_selection(base.domain, &entry.value);
        json!({"source":base,"definition_present":true,"domain_name":domain.name,"definition":entry.value,
            "sha256":entry.sha256,"trailing_bytes":entry.bytes-entry.value.consumed,
            "base_type":kind.map(|kind|&kind.base),"default_selection":default.as_ref().map(|(selection,_)|selection),
            "default":default.map(|(_,value)|value),"bit_getter_reviewed":domain.bit_getter_reviewed})
    }
    fn bit_report(&self, id: u32, query: Option<i32>) -> Result<Value> {
        let entry = self.bits.get(&id).context("source varbit is absent")?;
        let definition = &entry.value.definition;
        let base = definition
            .binding
            .and_then(|base| self.variables.get(&base).map(|entry| (base, entry)));
        let base_report = base.map(|(base, _)| self.variable_report(base));
        let projected = query
            .map(|value| -> Result<_> {
                ensure!(entry.value.error.is_none(), "source varbit decode failed");
                let (base, _) = base.context("source varbit has no resolved base variable")?;
                let domain = &self.schema.profile.domains[self.schema.domains[&base.domain]];
                ensure!(
                    domain.bit_getter_reviewed,
                    "source domain bit getter is unreviewed"
                );
                let variable = &self.variables[&base].value;
                let kind = variable
                    .data_type
                    .and_then(|id| self.schema.types.get(&id))
                    .map(|index| &self.schema.profile.types[*index]);
                ensure!(
                    kind.is_some_and(|kind| kind.base == "int"),
                    "source base value has no integer contract"
                );
                Ok(self
                    .schema
                    .profile
                    .getter
                    .project(value, definition.start, definition.end))
            })
            .transpose()?;
        Ok(
            json!({"id":id,"sha256":entry.sha256,"definition":definition,"decode_error":entry.value.error,
            "trailing_bytes":entry.bytes.saturating_sub(definition.consumed),"base_variable":base_report,
            "query":query.map(|base|json!({"base_value":base,"value":projected}))}),
        )
    }
    pub fn report(
        &self,
        selected: Option<u32>,
        query: Option<i32>,
        scripts: Option<&BTreeMap<u32, Vec<u8>>>,
        book: &Book,
    ) -> Result<Value> {
        let mut uses = Vec::new();
        let mut gaps = Vec::new();
        let mut selected_ids = std::collections::BTreeSet::new();
        if let Some(id) = selected {
            selected_ids.insert(id);
        }
        if let Some(scripts) = scripts {
            for (caller, bytes) in scripts {
                let script = crate::semantic::normalize(bytes, book)?;
                for (instruction, step) in script.instructions.iter().enumerate() {
                    if let Some(operation) = &step.operation {
                        match operation.argument {
                            Argument::Varbit { id, secondary } => {
                                if selected.is_none() || selected == Some(id) {
                                    selected_ids.insert(id);
                                }
                                let gap = match self.bits.get(&id) {
                                    None => Some("source_varbit_missing"),
                                    Some(entry) if entry.value.error.is_some() => {
                                        Some("source_varbit_decode_failed")
                                    }
                                    Some(entry) if entry.value.definition.binding.is_none() => {
                                        Some("source_varbit_unbound")
                                    }
                                    Some(_) if operation.command != "push_varbit" => {
                                        Some("source_bit_operation_unreviewed")
                                    }
                                    Some(entry) => {
                                        let base = entry
                                            .value
                                            .definition
                                            .binding
                                            .expect("guarded binding");
                                        let domain = &self.schema.profile.domains
                                            [self.schema.domains[&base.domain]];
                                        if !domain.bit_getter_reviewed {
                                            Some("source_domain_getter_unreviewed")
                                        } else if self.variables[&base].value.data_type.is_none() {
                                            Some("source_base_type_unresolved")
                                        } else {
                                            None
                                        }
                                    }
                                };
                                if let Some(reason) = gap {
                                    gaps.push(json!({"caller":caller,"instruction":instruction,"varbit":id,"reason":reason}));
                                }
                                uses.push(json!({"caller":caller,"instruction":instruction,"source_sha256":script.source_sha256,
                                    "opcode":step.wire.opcode,"command":operation.command,"varbit":id,"secondary":secondary,
                                    "registry":if secondary==u8::default(){"primary"}else{"secondary"},"definition_present":self.bits.contains_key(&id)}));
                            }
                            Argument::Variable {
                                domain,
                                id,
                                secondary,
                            } => {
                                let base = Base {
                                    domain,
                                    id: i32::from(id),
                                };
                                let reason = match self.variables.get(&base) {
                                    None => Some("source_variable_missing"),
                                    Some(entry) if entry.value.data_type.is_none() => {
                                        Some("source_variable_type_unresolved")
                                    }
                                    _ => None,
                                };
                                if let Some(reason) = reason {
                                    gaps.push(json!({"caller":caller,"instruction":instruction,"base_variable":base,"reason":reason}));
                                }
                                uses.push(json!({"caller":caller,"instruction":instruction,"source_sha256":script.source_sha256,
                                    "opcode":step.wire.opcode,"command":operation.command,"base_variable":self.variable_report(base),"secondary":secondary,
                                    "registry":if secondary==u8::default(){"primary"}else{"secondary"},
                                    "definition_present":self.variables.contains_key(&base)}));
                            }
                            _ => {}
                        }
                    }
                }
            }
        } else if selected.is_none() {
            selected_ids.extend(self.bits.keys());
        }
        let definitions = selected_ids
            .into_iter()
            .map(|id| {
                if scripts.is_some() && selected.is_none() && !self.bits.contains_key(&id) {
                    Ok(json!({"id":id,"definition_present":false}))
                } else {
                    self.bit_report(id, query)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let closure = scripts
            .map(|scripts| import910::inspect_closure(scripts, book))
            .transpose()?;
        Ok(
            json!({"format":PROFILE_FORMAT,"build":self.schema.profile.build,"client_md5":self.schema.profile.client_md5,
            "profile_sha256":self.profile_sha256,"schema_sha256":self.schema_sha256,
            "config_index_sha256":self.schema.profile.config_index_sha256,"script_index_sha256":self.schema.profile.script_index_sha256,
            "variable_count":self.variables.len(),"varbit_count":self.bits.len(),
            "decode_failures":self.bits.values().filter(|entry|entry.value.error.is_some()).count(),
            "unbound_definitions":self.bits.values().filter(|entry|entry.value.definition.binding.is_none()).count(),
            "ignored_variable_tags":self.variables.values().map(|entry|entry.value.ignored_tags.len()).sum::<usize>(),
            "ignored_varbit_tags":self.bits.values().map(|entry|entry.value.definition.ignored_tags.len()).sum::<usize>(),
            "variable_trailing_bytes":self.variables.values().map(|entry|entry.bytes-entry.value.consumed).sum::<usize>(),
            "varbit_trailing_bytes":self.bits.values().map(|entry|entry.bytes.saturating_sub(entry.value.definition.consumed)).sum::<usize>(),
            "untyped_variables":self.variables.values().filter(|entry|entry.value.data_type.is_none()).count(),
            "getter":self.schema.profile.getter,"definitions":definitions,"script_uses":uses,
            "source_binding_gaps":gaps,"script_dependency_complete":closure.as_ref().map(|closure|closure.dependency_complete),
            "unresolved_dependencies":closure.as_ref().map(|closure|&closure.unresolved_dependencies)}),
        )
    }
}

#[cfg(test)]
pub(crate) fn verify(book: &Book) {
    let schema = Schema::parse(
        include_bytes!("../../../revisions/950/cs2/variables.json"),
        book,
    )
    .unwrap();
    let fixture: Value =
        serde_json::from_slice(include_bytes!("../fixtures/variables.json")).unwrap();
    assert_eq!(fixture["client_md5"], book.profile.client_md5);
    for (name, hash) in &schema.profile.kernels {
        assert_eq!(&fixture["kernels"][name], hash);
    }
    for case in fixture["base_decodes"].as_array().unwrap() {
        let bytes: Vec<u8> = serde_json::from_value(case["wire"].clone()).unwrap();
        let variable = schema.variable(&bytes).unwrap();
        let mut actual = serde_json::to_value(&variable).unwrap();
        actual.as_object_mut().unwrap().remove("ignored_tags");
        let mut expected = case["result"].clone();
        expected.as_object_mut().unwrap().remove("events");
        assert_eq!(actual, expected, "{}", case["name"]);
        assert!(
            schema
                .variable(&bytes[..bytes.len() - usize::from(true)])
                .is_err()
                || variable.consumed < bytes.len()
        );
    }
    for case in fixture["default_selectors"].as_array().unwrap() {
        let mut variable = schema
            .variable(&[schema.profile.variable_definition.end])
            .unwrap();
        variable.data_type = Some(case["input"]["type"].as_u64().unwrap() as u16);
        variable.legacy = case["input"]["legacy"].as_bool().unwrap();
        let name = if case["input"]["client"].as_bool().unwrap() {
            "client"
        } else {
            "player"
        };
        let domain = schema
            .profile
            .domains
            .iter()
            .find(|domain| domain.name == name)
            .unwrap();
        assert_eq!(
            schema.default_selection(domain.id, &variable).unwrap().0,
            case["result"]["selected"]
        );
    }
    for case in fixture["decodes"].as_array().unwrap() {
        let bytes: Vec<u8> = serde_json::from_value(case["wire"].clone()).unwrap();
        let decoded = schema.bit(&bytes, |base| {
            if !schema.domains.contains_key(&base.domain) {
                Lookup::UnknownDomain
            } else if case["options"]["returned_base"].as_bool() == Some(false) {
                Lookup::MissingBase
            } else {
                Lookup::Resolved
            }
        });
        let expected = &case["result"];
        assert_eq!(json!(decoded.definition.start), expected["start"]);
        assert_eq!(json!(decoded.definition.end), expected["end"]);
        assert_eq!(json!(decoded.definition.flag), expected["flag"]);
        assert_eq!(json!(decoded.definition.consumed), expected["consumed"]);
        assert_eq!(json!(decoded.error.is_some()), expected["error"]);
        // Retention is checked below with a real preceding resolved binding.
        if expected["binding"] != "retained" {
            assert_eq!(
                decoded.definition.binding.is_some(),
                expected["binding"] == "resolved"
            );
        }
    }
    let resolved = fixture["decodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "server_binding")
        .unwrap();
    let retained = fixture["decodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "missing_domain_retains")
        .unwrap();
    let mut bytes: Vec<u8> = serde_json::from_value(resolved["wire"].clone()).unwrap();
    bytes.pop();
    bytes.extend(serde_json::from_value::<Vec<u8>>(retained["wire"].clone()).unwrap());
    let decoded = schema.bit(&bytes, |base| {
        if schema.domains.contains_key(&base.domain) {
            Lookup::Resolved
        } else {
            Lookup::UnknownDomain
        }
    });
    assert!(decoded.error.is_none());
    assert!(decoded.definition.binding.is_some());
    for case in fixture["gets"].as_array().unwrap() {
        verify_vm_get(&schema, case, &fixture);
        if let Some(expected) = case["result"]["value"].as_i64() {
            let input = &case["input"];
            assert_eq!(
                i64::from(schema.profile.getter.project(
                    input["base"].as_i64().unwrap() as i32,
                    input["start"].as_u64().unwrap() as u8,
                    input["end"].as_u64().unwrap() as u8
                )),
                expected,
                "{}",
                case["name"]
            );
        }
    }
}

#[cfg(test)]
fn verify_vm_get(schema: &Schema, case: &Value, fixture: &Value) {
    use native910::{
        execution::{HostOperation, Role, Specification, Table, decode_accounting},
        opcode::OpcodeBook,
        runtime::RuntimeHost,
        script::{CompiledScript, Counts, Instruction, Operand, VarRef},
        vars::VarScope,
        vm::{Programs, Session, Value as VmValue, VarLane, Vm},
    };
    const ADAPTER_ID: i32 = i32::MAX;
    const ROOT_ID: i32 = ADAPTER_ID - 1;
    const VARIABLE_ID: u16 = u16::MAX;
    let input = &case["input"];
    let domain = if input["client"].as_bool() == Some(true) {
        VarScope::Client
    } else {
        VarScope::Player
    };
    let start = input["start"].as_u64().unwrap() as u8;
    let end = input["end"].as_u64().unwrap() as u8;
    let operation = HostOperation::VariableBit {
        shift: u8::try_from(u32::from(start) & schema.profile.getter.shift_mask).unwrap(),
        mask: schema.profile.getter.project(-i32::from(true), start, end),
        default: i32::default(),
    };
    assert_eq!(
        HostOperation::parse(&operation.spelling()).unwrap(),
        operation
    );
    let book = OpcodeBook::embedded().unwrap();
    let instruction = |command: &str, operand| Instruction {
        opcode: book.opcode_for(command).unwrap(),
        command: command.into(),
        operand,
    };
    let mut adapter = CompiledScript {
        name: Some("proc,recorded_bit_read".into()),
        args: Counts::default(),
        locals: Counts::default(),
        code: vec![
            instruction(
                operation.command(),
                Operand::VarRef(VarRef {
                    domain,
                    id: VARIABLE_ID,
                    transmog: false,
                }),
            ),
            instruction("return", Operand::Byte(u8::default())),
        ],
    };
    let mut execution = Table::default();
    let bytes = execution
        .bind_import(
            ADAPTER_ID,
            &mut adapter,
            &book,
            Specification {
                role: Role::Adapter,
                resource: None,
                adapter_calls: BTreeMap::new(),
                host_operations: BTreeMap::from([(usize::default(), operation)]),
            },
        )
        .unwrap();
    let accounting = decode_accounting(
        ADAPTER_ID,
        &bytes,
        execution.group_bytes(ADAPTER_ID).as_deref(),
        &adapter,
    )
    .unwrap();
    let prefix = fixture["pushes"][0]["result"]["ints"][0].as_i64().unwrap() as i32;
    let mut root = CompiledScript {
        name: Some("proc,recorded_bit_caller".into()),
        args: Counts::default(),
        locals: Counts::default(),
        code: vec![
            instruction("push_constant_string", Operand::Int(prefix)),
            instruction("gosub_with_params", Operand::Script(ADAPTER_ID)),
            instruction("return", Operand::Byte(u8::default())),
        ],
    };
    execution
        .bind_import(
            ROOT_ID,
            &mut root,
            &book,
            Specification {
                role: Role::Source,
                resource: None,
                host_operations: BTreeMap::new(),
                adapter_calls: BTreeMap::from([(usize::from(true), ADAPTER_ID)]),
            },
        )
        .unwrap();
    let programs = Programs {
        scripts: std::collections::HashMap::from([(ROOT_ID, root.clone()), (ADAPTER_ID, adapter)]),
        accounting: std::collections::HashMap::from([
            (ROOT_ID, execution.accounting(ROOT_ID)),
            (ADAPTER_ID, accounting),
        ]),
    };
    let mut host = RuntimeHost::default();
    host.definitions.insert((domain, VARIABLE_ID), VarLane::Int);
    let base = input["base"].as_i64().unwrap() as i32;
    host.variables.insert(
        (domain, VARIABLE_ID, false),
        if input["tag"].as_u64() == Some(u64::from(true)) {
            VmValue::Long(i64::from(base))
        } else {
            VmValue::Int(base)
        },
    );
    let mut session = Session::for_script(ROOT_ID, &root, &[], None).unwrap();
    let mut vm = Vm::new(&mut host, &programs);
    let failed = loop {
        match vm.step(&mut session) {
            Ok(true) => break false,
            Ok(false) => {}
            Err(_) => break true,
        }
    };
    assert_eq!(
        failed,
        case["result"]["value"].is_null(),
        "{}",
        case["name"]
    );
    let expected = std::iter::once(prefix)
        .chain(case["result"]["value"].as_i64().map(|value| value as i32))
        .collect::<Vec<_>>();
    assert_eq!(session.snapshot().ints, expected, "{}", case["name"]);
}
