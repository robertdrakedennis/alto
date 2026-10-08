//! Named source-to-live variable bindings for imported bit reads. Target
//! definitions are verified before publication and defaults remain checked by
//! the ordinary runtime owner. Source IDs are never a target fallback.
use crate::{
    profile::{Book, digest},
    semantic::Argument,
    variables::{Base, Definitions, Identity},
};
use anyhow::{Context, Result, ensure};
use native910::{
    execution::HostOperation,
    pack::PackArchive,
    script::{Operand, VarRef},
    vars::VarScope,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub source: Base,
    /// Fully qualified registry spelling (`varp.name` or `varc.name`).
    pub target: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub source: Identity,
    pub target_config_sha256: String,
    pub symbols_sha256: String,
    pub bindings: Vec<Binding>,
}
pub struct Symbols {
    hash: String,
    entries: BTreeMap<String, VarRef>,
}
impl Symbols {
    pub fn load(root: &Path) -> Result<Self> {
        Self::parse(
            &std::fs::read_to_string(root.join("varp.sym"))?,
            &std::fs::read_to_string(root.join("varc.sym"))?,
        )
    }
    pub fn parse(player: &str, client: &str) -> Result<Self> {
        let mut entries = BTreeMap::new();
        for (prefix, domain, text) in [
            ("varp", VarScope::Player, player),
            ("varc", VarScope::Client, client),
        ] {
            for line in text
                .lines()
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
            {
                let fields: Vec<_> = line.split('\t').collect();
                ensure!(matches!(fields.len(), 2 | 3), "invalid variable symbol row");
                ensure!(
                    native910::source::is_valid_name(fields[0]),
                    "invalid variable symbol name"
                );
                let name = format!("{prefix}.{}", fields[0]);
                let reference = VarRef {
                    domain,
                    id: fields[1].parse()?,
                    transmog: false,
                };
                ensure!(
                    entries.insert(name, reference).is_none(),
                    "duplicate variable symbol"
                );
            }
        }
        Ok(Self {
            hash: digest(&serde_json::to_vec(&(player, client))?),
            entries,
        })
    }
}
#[derive(Clone, Serialize)]
pub struct Bound {
    pub source: Base,
    pub target_symbol: String,
    pub target_domain: String,
    pub target_id: u16,
    pub target_definition_sha256: String,
    pub default: i32,
}
pub struct Import<'a> {
    pub definitions: &'a Definitions,
    pub symbols: &'a Symbols,
}
pub(crate) struct ResolvedBit {
    pub operand: Operand,
    pub operation: HostOperation,
}
pub(crate) struct Resolved<'a> {
    definitions: &'a Definitions,
    pub bindings: BTreeMap<Base, Bound>,
}
fn bases(
    scripts: &BTreeMap<u32, Vec<u8>>,
    book: &Book,
    definitions: &Definitions,
    database: Option<&crate::database::Definitions>,
) -> Result<BTreeSet<Base>> {
    let normalized = scripts
        .iter()
        .map(|(id, bytes)| Ok((*id, crate::semantic::normalize(bytes, book)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut result = BTreeSet::new();
    for (id, script) in &normalized {
        for (pc, instruction) in script.instructions.iter().enumerate() {
            if let Some(operation) = &instruction.operation
                && let Argument::Varbit { id: bit, .. } = operation.argument
            {
                ensure!(
                    operation.command == "push_varbit",
                    "unreviewed variable bit operation {} in donor {id} @{pc}",
                    operation.command
                );
                result.insert(
                    definitions
                        .bit_contract(bit)
                        .with_context(|| format!("donor {id} @{pc} source varbit {bit}"))?
                        .base,
                );
            }
        }
    }
    for callback in crate::flow::inspect_with_database(&normalized, book, database)?
        .callbacks
        .values()
    {
        if let Some(domain) = callback.transmit_domain {
            ensure!(
                !matches!(
                    callback.status,
                    crate::flow::CallbackStatus::Unresolved { .. }
                ),
                "variable callback traffic is unresolved at {} @{}",
                callback.caller,
                callback.instruction
            );
            for trigger in &callback.triggers {
                let Some(crate::flow::Literal::Int(id)) = trigger.constant else {
                    anyhow::bail!("variable callback trigger needs a proven source identity");
                };
                let base = Base { domain, id };
                definitions.base_contract(base)?;
                result.insert(base);
            }
        }
    }
    Ok(result)
}
impl Plan {
    pub fn prepare(
        import: Import<'_>,
        scripts: &BTreeMap<u32, Vec<u8>>,
        book: &Book,
        pack_root: &Path,
    ) -> Result<Self> {
        Self::prepare_with_database(import, scripts, book, pack_root, None)
    }
    pub fn prepare_with_database(
        import: Import<'_>,
        scripts: &BTreeMap<u32, Vec<u8>>,
        book: &Book,
        pack_root: &Path,
        database: Option<&crate::database::Definitions>,
    ) -> Result<Self> {
        let source = import.definitions.identity()?;
        ensure!(
            source.profile_sha256 == book.sha256,
            "variable definitions use a different source opcode profile"
        );
        Ok(Self {
            source,
            target_config_sha256: digest(&std::fs::read(pack_root.join("client.config.js5"))?),
            symbols_sha256: import.symbols.hash.clone(),
            bindings: bases(scripts, book, import.definitions, database)?
                .into_iter()
                .map(|source| Binding {
                    source,
                    target: None,
                })
                .collect(),
        })
    }
}
impl<'a> Import<'a> {
    pub(crate) fn resolve(
        self,
        plan: &Plan,
        scripts: &BTreeMap<u32, Vec<u8>>,
        book: &Book,
        pack_root: &Path,
        database: Option<&crate::database::Definitions>,
    ) -> Result<Resolved<'a>> {
        ensure!(
            plan.source.profile_sha256 == book.sha256,
            "variable definitions use a different source opcode profile"
        );
        ensure!(
            plan.source == self.definitions.identity()?,
            "plan source variable definitions changed"
        );
        ensure!(
            plan.symbols_sha256 == self.symbols.hash,
            "plan target variable symbols changed"
        );
        let bytes = std::fs::read(pack_root.join("client.config.js5"))?;
        ensure!(
            plan.target_config_sha256 == digest(&bytes),
            "plan target variable config changed"
        );
        let archive = PackArchive::from_bytes(bytes)?;
        let needed = bases(scripts, book, self.definitions, database)?;
        let mut bindings = BTreeMap::new();
        for binding in &plan.bindings {
            ensure!(
                needed.contains(&binding.source),
                "variable binding is outside the selected closure"
            );
            let name = binding.target.as_ref().with_context(|| {
                format!(
                    "source variable {:?} needs a named live binding",
                    binding.source
                )
            })?;
            let reference = self
                .symbols
                .entries
                .get(name)
                .context("target variable symbol is absent")?;
            let definition = &self.definitions.base_contract(binding.source)?;
            ensure!(
                reference.domain.as_label() == definition.domain_name,
                "variable binding changes its state owner"
            );
            let group = native910::config::var_group_id(reference.domain);
            let files = archive
                .group_files(group)?
                .context("target variable group is absent")?;
            let wire = files
                .get(&u32::from(reference.id))
                .context("named target variable definition is absent")?;
            let target = native910::config::decode_var(wire, reference.domain)?;
            ensure!(
                target.data_type.and_then(native910::config::var_value_kind)
                    == Some(native910::config::VarValueKind::Int),
                "target bit base has no integer contract"
            );
            let target_binding = rs910_config::types910::varbits::Binding {
                domain: u8::from(reference.domain),
                id: i32::from(reference.id),
                data_type: target.data_type,
                lifetime: target.lifetime,
                legacy: target.legacy_default_value,
                client_code: i32::from(target.client_code.unwrap_or_default()),
            };
            let rs910_config::types910::variables::Value::Int(default) = target_binding
                .default_value()
                .map_err(|error| anyhow::anyhow!("target variable default: {error:?}"))?
            else {
                anyhow::bail!("target variable default is not an integer")
            };
            ensure!(
                definition.default == default,
                "source and target variable defaults disagree"
            );
            let bound = Bound {
                source: binding.source,
                target_symbol: name.clone(),
                target_domain: reference.domain.as_label().into(),
                target_id: reference.id,
                target_definition_sha256: digest(wire),
                default,
            };
            ensure!(
                bindings.insert(binding.source, bound).is_none(),
                "duplicate source variable binding"
            );
        }
        ensure!(
            bindings.keys().copied().collect::<BTreeSet<_>>() == needed,
            "named bindings must cover all source variable bases"
        );
        Ok(Resolved {
            definitions: self.definitions,
            bindings,
        })
    }
}
impl Resolved<'_> {
    pub fn bit(&self, id: u32, secondary: u8) -> Result<ResolvedBit> {
        let contract = self.definitions.bit_contract(id)?;
        let bound = self
            .bindings
            .get(&contract.base)
            .context("source bit base has no live binding")?;
        Ok(ResolvedBit {
            operand: Operand::VarRef(VarRef {
                domain: VarScope::from_label(&bound.target_domain)?,
                id: bound.target_id,
                transmog: secondary != u8::default(),
            }),
            operation: HostOperation::VariableBit {
                shift: contract.shift,
                mask: contract.mask,
                default: contract.default,
            },
        })
    }
}

impl Resolved<'_> {
    pub fn triggers(&self, callback: &crate::flow::Callback) -> Result<Vec<i32>> {
        let domain = callback
            .transmit_domain
            .context("callback has no verified variable owner")?;
        callback
            .triggers
            .iter()
            .map(|trigger| {
                let Some(crate::flow::Literal::Int(id)) = trigger.constant else {
                    anyhow::bail!("variable trigger identity is unresolved");
                };
                let bound = self
                    .bindings
                    .get(&Base { domain, id })
                    .context("source variable trigger has no named live binding")?;
                ensure!(
                    bound.target_domain == native910::vars::VarScope::Player.as_label(),
                    "callback trigger changes its variable owner"
                );
                Ok(i32::from(bound.target_id))
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) fn verify_trigger_bases(book: &Book) {
    use crate::{flow, wire};
    const CALLER: u32 = i32::MAX as u32;
    const CALLBACK: u32 = i32::MAX as u32 - 1;
    const SOURCE_BASE: i32 = i32::MAX;
    const TRIGGER_PC: usize = 1;
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("../fixtures/variables.json")).unwrap();
    let variable_wire: Vec<u8> = serde_json::from_value(
        fixture["base_decodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "player_theme")
            .unwrap()["wire"]
            .clone(),
    )
    .unwrap();
    let base = Base {
        domain: u8::from(VarScope::Player),
        id: SOURCE_BASE,
    };
    let definitions = Definitions::authored_fixture(
        book,
        include_bytes!("../../../revisions/950/cs2/variables.json"),
        &[(base, variable_wire)],
        &[],
    )
    .unwrap();
    let instruction = |command: &str, operand| wire::Instruction {
        opcode: book
            .profile
            .opcodes
            .iter()
            .find(|row| row.command.as_deref() == Some(command))
            .unwrap()
            .id,
        operand,
    };
    let counts = wire::Counts::default();
    let caller = wire::Script {
        name: Vec::new(),
        args: counts,
        locals: counts,
        switches: Vec::new(),
        code: vec![
            instruction("push_constant", wire::Operand::ConstantInt(CALLBACK as i32)),
            instruction("push_constant", wire::Operand::ConstantInt(base.id)),
            instruction("push_constant", wire::Operand::ConstantInt(i32::from(true))),
            instruction(
                "push_constant",
                wire::Operand::ConstantString(b"Y".to_vec()),
            ),
            instruction("active_transmit_hook", wire::Operand::Byte(u8::default())),
            instruction("return", wire::Operand::Byte(u8::default())),
        ],
    };
    let mut caller = caller;
    let callback = wire::Script {
        name: Vec::new(),
        args: counts,
        locals: counts,
        switches: Vec::new(),
        code: vec![instruction("return", wire::Operand::Byte(u8::default()))],
    };
    let mut scripts = BTreeMap::from([
        (CALLER, wire::encode(&caller, book).unwrap()),
        (CALLBACK, wire::encode(&callback, book).unwrap()),
    ]);
    assert_eq!(
        bases(&scripts, book, &definitions, None).unwrap(),
        BTreeSet::from([base])
    );
    let normalized = scripts
        .iter()
        .map(|(id, bytes)| (*id, crate::semantic::normalize(bytes, book).unwrap()))
        .collect();
    let proof = flow::inspect(&normalized, book).unwrap();
    let callback = proof.callbacks.values().next().unwrap();
    let symbols = Symbols::parse(
        include_str!("../../../revisions/910/symbols/varp.sym"),
        include_str!("../../../revisions/910/symbols/varc.sym"),
    )
    .unwrap();
    let target_symbol = "varp.interface_layout";
    let target_id = symbols.entries[target_symbol].id;
    let resolved = Resolved {
        definitions: &definitions,
        bindings: BTreeMap::from([(
            base,
            Bound {
                source: base,
                target_symbol: target_symbol.into(),
                target_domain: VarScope::Player.as_label().into(),
                target_id,
                target_definition_sha256: String::new(),
                default: i32::default(),
            },
        )]),
    };
    assert_eq!(
        resolved.triggers(callback).unwrap(),
        vec![i32::from(target_id)]
    );
    assert_ne!(base.id, i32::from(target_id));
    let unbound = Resolved {
        definitions: &definitions,
        bindings: BTreeMap::new(),
    };
    assert!(unbound.triggers(callback).is_err());
    caller.args.int = u16::from(true);
    caller.locals = caller.args;
    caller.code[TRIGGER_PC] = instruction("push_int_local", wire::Operand::Int(i32::default()));
    scripts.insert(CALLER, wire::encode(&caller, book).unwrap());
    assert!(bases(&scripts, book, &definitions, None).is_err());
}
