//! Revision-specific lowering uses reviewed source-handler and target-book facts.
//! Host requirements must be bound before an imported entry can be published.

use crate::{
    profile::{Book, Build, Encoding, StringEncoding, SwitchLookup},
    semantic::{self, Argument, Lane},
};
use anyhow::{Context, Result, ensure};
use native910::{
    execution::HostOperation,
    opcode::{BOOK_BUILD, OpcodeBook},
    script::{CompiledScript, Counts, Instruction, Operand, SwitchCase},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

const ADAPTER_FORMAT: u32 = 1;
const BANK_ARGUMENT_SLOT: u16 = 0;
const FIND_BANK_ARGUMENT_SLOT: u16 = 1;
const FIND_SLOT_ARGUMENT_SLOT: u16 = 2;
const FLAT_CHILD_ARGUMENT_SLOT: u16 = 1;
const CREATE_KIND_ARGUMENT_SLOT: u16 = 1;
const CREATE_CHILD_ARGUMENT_SLOT: u16 = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adapter {
    pub format: u32,
    pub source_build: Build,
    pub source_client_md5: String,
    pub source_string_encoding: StringEncoding,
    pub source_switch_lookup: SwitchLookup,
    pub target_build: u32,
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub callback_policy: Option<CallbackPolicy>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackPolicy {
    pub clear_script_id: i32,
    pub int_event_tokens: Vec<i32>,
    pub string_event_tokens: Vec<String>,
}
impl CallbackPolicy {
    pub fn variable_event_tokens(&self) -> Result<native910::execution::VariableEventTokens> {
        let first = *self
            .int_event_tokens
            .first()
            .context("event token layout is absent")?;
        ensure!(
            self.int_event_tokens
                .iter()
                .enumerate()
                .all(|(slot, token)| first.checked_add(slot as i32) == Some(*token)),
            "event token layout is not contiguous"
        );
        Ok(native910::execution::VariableEventTokens::new(
            first,
            u8::try_from(self.int_event_tokens.len())?,
        )?)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub source_opcode: u16,
    pub source_handler: u64,
    pub source_encoding: Encoding,
    pub source_command: String,
    pub operations: BTreeMap<String, Target>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub command: String,
    pub opcode: u16,
    pub large_operand: bool,
    pub requirement: Requirement,
    #[serde(default, deserialize_with = "deserialize_host_operation")]
    pub host_operation: Option<HostOperation>,
    #[serde(default)]
    pub selector: Option<SelectorPolicy>,
    #[serde(default)]
    pub child_template: Option<TextChildTemplate>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextChildTemplate {
    pub source_kind: u8,
    pub component_type: i32,
    pub colour: i32,
    pub transparency: u8,
    pub aspect: [i32; 2],
    pub font: i32,
    pub monochrome: bool,
    pub line_height: i32,
    pub horizontal_align: i32,
    pub vertical_align: i32,
    pub max_lines: i32,
    pub shadow: bool,
    pub antimacro: bool,
}
impl TextChildTemplate {
    pub fn resource(&self) -> Vec<u8> {
        rs910_config::ui_component_fields::TextChildTemplate {
            source_kind: self.source_kind,
            component_type: self.component_type,
            colour: self.colour,
            transparency: self.transparency,
            aspect: self.aspect,
            font: self.font,
            monochrome: self.monochrome,
            line_height: self.line_height,
            horizontal_align: self.horizontal_align,
            vertical_align: self.vertical_align,
            max_lines: self.max_lines,
            shadow: self.shadow,
            antimacro: self.antimacro,
        }
        .encode_resource()
    }
}

/// Reviewed source interpretation of a component selector byte. Conversion
/// produces the target VM's Boolean selector, independent of the source opcode.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SelectorPolicy {
    EqualsOne,
    Nonzero,
}
impl SelectorPolicy {
    pub fn convert(self, operand: &Operand) -> Result<Operand> {
        let Operand::Byte(value) = operand else {
            anyhow::bail!("component selection needs a byte selector")
        };
        Ok(match self {
            Self::EqualsOne => Operand::Byte(u8::from(*value == u8::from(true))),
            Self::Nonzero => Operand::Byte(u8::from(*value != u8::default())),
        })
    }
}

fn deserialize_host_operation<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<HostOperation>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|value| HostOperation::parse(&value).map_err(serde::de::Error::custom))
        .transpose()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
    Intrinsic,
    VariableBit,
    Enum,
    DatabaseField,
    DatabaseFieldCount,
    ExplicitPlainText,
    ExplicitOperationLabel,
    ExplicitVisibility,
    ExplicitComponentPaint,
    ActiveComponentPaint,
    ActiveComponentText,
    ExplicitComponentText,
    ExplicitRuntimeChildren,
    ExplicitRuntimeChildSlot,
    ExplicitRuntimeChildSelection,
    FlatChildSelection,
    FlatTextChildCreation,
    ExplicitComponentSelection,
    OperationCallback,
    PlayerVariableCallback,
    StringJoin,
}

#[derive(Clone, Debug, Serialize)]
pub struct HostRequirement {
    pub pc: usize,
    pub requirement: Requirement,
}

pub struct Lowered {
    pub script: CompiledScript,
    pub host_requirements: Vec<HostRequirement>,
}

impl Adapter {
    pub fn parse(bytes: &[u8], source: &Book, target: &OpcodeBook) -> Result<Self> {
        let adapter: Self = serde_json::from_slice(bytes)?;
        ensure!(
            adapter.format == ADAPTER_FORMAT,
            "unsupported adapter format"
        );
        ensure!(
            adapter.source_build == source.profile.build
                && adapter.source_client_md5 == source.profile.client_md5,
            "adapter source client mismatch"
        );
        ensure!(
            Some(adapter.source_string_encoding) == source.profile.string_encoding,
            "adapter source charset mismatch"
        );
        ensure!(
            Some(adapter.source_switch_lookup) == source.profile.switch_lookup,
            "adapter source switch policy mismatch"
        );
        ensure!(
            adapter.target_build == BOOK_BUILD,
            "adapter target revision mismatch"
        );
        let mut seen = BTreeMap::new();
        for rule in &adapter.rules {
            ensure!(
                seen.insert(rule.source_opcode, ()).is_none(),
                "duplicate adapter source opcode"
            );
            let registration = source.opcode(rule.source_opcode)?;
            ensure!(
                registration.handler_address == rule.source_handler
                    && registration.encoding == rule.source_encoding
                    && registration.command.as_deref() == Some(rule.source_command.as_str()),
                "adapter source fact mismatch for opcode {}",
                rule.source_opcode
            );
            ensure!(!rule.operations.is_empty(), "empty adapter rule");
            for operation in rule.operations.values() {
                let expected_requirement = match operation.command.as_str() {
                    "push_var" => Some(Requirement::VariableBit),
                    "_enum" => Some(Requirement::Enum),
                    "db_getfield" => Some(Requirement::DatabaseField),
                    "db_getfieldcount" => Some(Requirement::DatabaseFieldCount),
                    "if_settext" => Some(Requirement::ExplicitPlainText),
                    "if_setop" => Some(Requirement::ExplicitOperationLabel),
                    "if_sethide" => Some(Requirement::ExplicitVisibility),
                    "if_setcolour" | "if_setfill" | "if_settrans" => {
                        Some(Requirement::ExplicitComponentPaint)
                    }
                    "cc_setcolour" | "cc_setfill" | "cc_settrans" => {
                        Some(Requirement::ActiveComponentPaint)
                    }
                    "cc_settextfont" | "cc_settextalign" | "cc_setmaxlines"
                    | "cc_getfontgraphic" | "cc_getfontmetrics" => {
                        Some(Requirement::ActiveComponentText)
                    }
                    "if_settextfont" | "if_settextalign" | "if_setmaxlines"
                    | "if_getfontgraphic" | "if_getfontmetrics" => {
                        Some(Requirement::ExplicitComponentText)
                    }
                    "if_setonop" => Some(Requirement::OperationCallback),
                    "cc_setonvartransmit" => Some(Requirement::PlayerVariableCallback),
                    "cc_create" => Some(Requirement::FlatTextChildCreation),
                    "cc_deleteall" => Some(Requirement::ExplicitRuntimeChildren),
                    "if_getnextsubid" => Some(Requirement::ExplicitRuntimeChildSlot),
                    "if_find" => Some(
                        if operation.host_operation == Some(HostOperation::FindComponent) {
                            Requirement::ExplicitComponentSelection
                        } else if matches!(
                            operation.host_operation,
                            Some(HostOperation::FindFlatChild { .. })
                        ) {
                            Requirement::FlatChildSelection
                        } else {
                            Requirement::ExplicitRuntimeChildSelection
                        },
                    ),
                    "join_string" => Some(Requirement::StringJoin),
                    _ => None,
                };
                ensure!(
                    expected_requirement.is_none_or(|expected| expected == operation.requirement),
                    "adapter host requirement mismatch for {}",
                    operation.command
                );
                ensure!(
                    matches!(
                        operation.requirement,
                        Requirement::ExplicitRuntimeChildSelection
                            | Requirement::ExplicitComponentSelection
                            | Requirement::FlatChildSelection
                            | Requirement::FlatTextChildCreation
                            | Requirement::ActiveComponentText
                            | Requirement::ActiveComponentPaint
                            | Requirement::PlayerVariableCallback
                    ) == operation.selector.is_some(),
                    "adapter source selector policy mismatch for {}",
                    operation.command
                );
                ensure!(
                    operation.child_template.is_some()
                        == (operation.requirement == Requirement::FlatTextChildCreation),
                    "constructor template is missing or attached to a non-creation use"
                );
                let expected_operation = match operation.requirement {
                    Requirement::FlatTextChildCreation => {
                        ensure!(
                            matches!(
                                operation.host_operation,
                                Some(HostOperation::CreateFlatTextChild {
                                    kind_argument: CREATE_KIND_ARGUMENT_SLOT,
                                    slot_argument: CREATE_CHILD_ARGUMENT_SLOT,
                                    source: None,
                                    ..
                                })
                            ),
                            "creation needs its kind, child identity and origin contract"
                        );
                        operation.host_operation
                    }
                    Requirement::ActiveComponentText | Requirement::ExplicitComponentText => {
                        ensure!(
                            matches!(operation.host_operation, Some(HostOperation::ComponentText { explicit, property, .. })
                            if explicit == (operation.requirement == Requirement::ExplicitComponentText)
                            && !matches!(property, native910::execution::TextProperty::Font { mapping: native910::execution::FontBinding::Constant(_) | native910::execution::FontBinding::Resource, .. } | native910::execution::TextProperty::ReadFont { initial: Some(_), .. })),
                            "text operation needs its component selection and unbound font contract"
                        );
                        if let Some(HostOperation::ComponentText { property, .. }) =
                            operation.host_operation
                        {
                            ensure!(
                                property.domain().is_none_or(
                                    |domain| domain.spelling() == adapter.source_client_md5
                                ),
                                "font identity belongs to a different source client"
                            );
                        }
                        operation.host_operation
                    }
                    Requirement::ActiveComponentPaint | Requirement::ExplicitComponentPaint => {
                        ensure!(
                            matches!(operation.host_operation, Some(HostOperation::ComponentPaint { explicit, .. }) if explicit == (operation.requirement == Requirement::ExplicitComponentPaint)),
                            "paint operation needs its component selection contract"
                        );
                        operation.host_operation
                    }
                    Requirement::PlayerVariableCallback => {
                        let policy = adapter
                            .callback_policy
                            .as_ref()
                            .context("callback policy absent")?;
                        let Some(crate::profile::AnalysisContract::Hook {
                            explicit_component: Some(false),
                            transmit_domain: Some(domain),
                            codec,
                            clear_script_id,
                            ..
                        }) = &registration.analysis
                        else {
                            anyhow::bail!("variable callback ownership is unreviewed");
                        };
                        ensure!(
                            *domain == u8::from(native910::vars::VarScope::Player)
                                && policy.clear_script_id == *clear_script_id
                                && codec.retained_triggers
                                && matches!(
                                    codec.descriptor_units,
                                    crate::profile::DescriptorUnits::Utf8Bytes
                                )
                                && matches!(
                                    codec.positive_triggers,
                                    crate::profile::PositiveTriggers::TransmitList
                                ),
                            "variable callback codec or owner mismatch"
                        );
                        Some(HostOperation::RetainedPlayerTransmit {
                            pops: [u16::from(true), u16::from(true), u16::default()],
                            event_tokens: policy.variable_event_tokens()?,
                        })
                    }
                    Requirement::ExplicitComponentSelection => Some(HostOperation::FindComponent),
                    Requirement::ExplicitRuntimeChildren => {
                        Some(HostOperation::ClearRuntimeChildren)
                    }
                    Requirement::FlatChildSelection => {
                        ensure!(
                            matches!(
                                operation.host_operation,
                                Some(HostOperation::FindFlatChild {
                                    slot_argument: FLAT_CHILD_ARGUMENT_SLOT,
                                    ..
                                })
                            ),
                            "flat child selection needs its encoded identity argument"
                        );
                        operation.host_operation
                    }
                    Requirement::ExplicitRuntimeChildSelection => {
                        ensure!(
                            matches!(
                                operation.host_operation,
                                Some(HostOperation::FindRuntimeChild {
                                    bank_argument: FIND_BANK_ARGUMENT_SLOT,
                                    slot_argument: FIND_SLOT_ARGUMENT_SLOT,
                                    ..
                                })
                            ),
                            "runtime child selection needs its bank and slot argument contract"
                        );
                        operation.host_operation
                    }
                    Requirement::ExplicitRuntimeChildSlot => {
                        ensure!(
                            matches!(
                                operation.host_operation,
                                Some(HostOperation::NextRuntimeChildSlot {
                                    bank_argument: BANK_ARGUMENT_SLOT,
                                    ..
                                })
                            ),
                            "runtime child slot query needs its bank argument contract"
                        );
                        operation.host_operation
                    }
                    _ => None,
                };
                ensure!(
                    operation.host_operation == expected_operation,
                    "adapter host operation mismatch for {}",
                    operation.command
                );
                if let Some(host_operation) = operation.host_operation {
                    ensure!(
                        operation.command == host_operation.command(),
                        "adapter host operation consumer mismatch"
                    );
                }
                if operation.requirement == Requirement::OperationCallback {
                    let policy = adapter
                        .callback_policy
                        .as_ref()
                        .context("callback policy absent")?;
                    let Some(crate::profile::AnalysisContract::Hook {
                        clear_script_id, ..
                    }) = &registration.analysis
                    else {
                        anyhow::bail!("callback traffic is unreviewed");
                    };
                    ensure!(
                        policy.clear_script_id == *clear_script_id,
                        "callback clear policy mismatch"
                    );
                }
                ensure!(
                    target.opcode_for(&operation.command)? == operation.opcode
                        && target.name(operation.opcode)? == operation.command
                        && target.has_large_operand(operation.opcode) == operation.large_operand,
                    "adapter target fact mismatch for {}",
                    operation.command
                );
            }
        }
        Ok(adapter)
    }

    pub fn lower(
        &self,
        source: &semantic::Script,
        links: &BTreeMap<u32, i32>,
        name: &str,
    ) -> Result<Lowered> {
        self.lower_with_uses(source, links, name, &BTreeMap::new())
    }

    pub(crate) fn target_for(&self, instruction: &semantic::Instruction) -> Result<&Target> {
        let operation = instruction
            .operation
            .as_ref()
            .context("unresolved source operation")?;
        self.rules
            .iter()
            .find(|rule| rule.source_opcode == instruction.wire.opcode)
            .and_then(|rule| rule.operations.get(&operation.command))
            .with_context(|| format!("no reviewed lowering for {}", operation.command))
    }

    pub(crate) fn lower_with_uses(
        &self,
        source: &semantic::Script,
        links: &BTreeMap<u32, i32>,
        name: &str,
        uses: &BTreeMap<usize, i32>,
    ) -> Result<Lowered> {
        ensure!(
            source.build == self.source_build
                && source.client_md5 == self.source_client_md5
                && source.switch_lookup == Some(self.source_switch_lookup),
            "normalized source client mismatch"
        );
        ensure!(
            native910::source::is_valid_name(name),
            "invalid imported procedure name"
        );
        verify_local_initialization(source)?;
        let rules: BTreeMap<_, _> = self
            .rules
            .iter()
            .map(|rule| (rule.source_opcode, rule))
            .collect();
        let mut requirements = Vec::new();
        let book = OpcodeBook::embedded()?;
        let code = source
            .instructions
            .iter()
            .enumerate()
            .map(|(pc, instruction)| {
                let operation = instruction.operation.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "unresolved source opcode {} at {pc}",
                        instruction.wire.opcode
                    )
                })?;
                let target = rules
                    .get(&instruction.wire.opcode)
                    .and_then(|rule| rule.operations.get(&operation.command))
                    .ok_or_else(|| {
                        anyhow::anyhow!("no reviewed lowering for {} at {pc}", operation.command)
                    })?;
                if let Some(bridge) = uses.get(&pc) {
                    return Ok(Instruction {
                        opcode: book.opcode_for("gosub_with_params")?,
                        command: "gosub_with_params".into(),
                        operand: Operand::Script(*bridge),
                    });
                }
                ensure!(
                    target.host_operation.is_none()
                        && !matches!(
                            target.requirement,
                            Requirement::Enum
                                | Requirement::VariableBit
                                | Requirement::DatabaseField
                                | Requirement::DatabaseFieldCount
                        ),
                    "host operation at {pc} needs a use-specific bridge"
                );
                ensure!(
                    !matches!(
                        target.requirement,
                        Requirement::OperationCallback | Requirement::PlayerVariableCallback
                    ),
                    "callback at {pc} needs use-specific script linking"
                );
                let operand = match &operation.argument {
                    Argument::Int(value) => Operand::Int(*value),
                    Argument::Long(value) => Operand::Long(*value),
                    Argument::Text {
                        decoded: Some(value),
                        ..
                    } => Operand::Str(value.clone()),
                    Argument::Local { slot, .. } => Operand::Local(i32::from(*slot)),
                    Argument::Branch(target) => Operand::Branch(i32::try_from(*target)?),
                    Argument::Switch(cases) => Operand::Switch(
                        semantic::selected_cases(cases, source.switch_lookup)?
                            .iter()
                            .map(|case| {
                                Ok(SwitchCase {
                                    value: case.value,
                                    target: i32::try_from(case.target)?,
                                })
                            })
                            .collect::<Result<_>>()?,
                    ),
                    Argument::Script(id) => {
                        Operand::Script(*links.get(id).ok_or_else(|| {
                            anyhow::anyhow!("unbound donor procedure {id} at {pc}")
                        })?)
                    }
                    Argument::Byte(value) => Operand::Byte(*value),
                    Argument::Count(count) => {
                        u16::try_from(*count).context("string join count is out of range")?;
                        Operand::Count(*count)
                    }
                    _ => anyhow::bail!(
                        "operand cannot be represented for {} at {pc}",
                        operation.command
                    ),
                };
                if target.requirement != Requirement::Intrinsic {
                    requirements.push(HostRequirement {
                        pc,
                        requirement: target.requirement,
                    });
                }
                ensure!(
                    native910::semantics::contract(&book, &target.command, &operand)?
                        .retail_dispatch,
                    "target command has no retail dispatch: {}",
                    target.command
                );
                Ok(Instruction {
                    opcode: target.opcode,
                    command: target.command.clone(),
                    operand,
                })
            })
            .collect::<Result<_>>()?;
        let counts = |counts: crate::wire::Counts| Counts {
            int: counts.int,
            obj: counts.object,
            long: counts.long,
        };
        Ok(Lowered {
            script: CompiledScript {
                name: Some(format!("proc,{name}")),
                locals: counts(source.locals),
                args: counts(source.args),
                code,
            },
            host_requirements: requirements,
        })
    }
}

/// Require assignment on every predecessor before reading a non-argument local.
/// Lowering then has no dependency on how a donor reuses its local banks.
fn verify_local_initialization(script: &semantic::Script) -> Result<()> {
    const INT_LANE: usize = 0;
    const OBJECT_LANE: usize = 1;
    const LONG_LANE: usize = 2;
    ensure!(!script.instructions.is_empty(), "empty donor procedure");
    let initial = [
        (0..script.locals.int)
            .map(|slot| slot < script.args.int)
            .collect::<Vec<_>>(),
        (0..script.locals.object)
            .map(|slot| slot < script.args.object)
            .collect::<Vec<_>>(),
        (0..script.locals.long)
            .map(|slot| slot < script.args.long)
            .collect::<Vec<_>>(),
    ];
    let mut before = vec![None; script.instructions.len()];
    before[0] = Some(initial);
    let mut pending = VecDeque::from([0_usize]);
    while let Some(pc) = pending.pop_front() {
        let mut assigned = before[pc]
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing local state"))?;
        let operation = script.instructions[pc].operation.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "unresolved source opcode {} at {pc}",
                script.instructions[pc].wire.opcode
            )
        })?;
        if let Argument::Local { lane, slot } = operation.argument {
            let lane = match lane {
                Lane::Int => INT_LANE,
                Lane::Long => LONG_LANE,
                Lane::Object => OBJECT_LANE,
            };
            let value = assigned[lane]
                .get_mut(usize::from(slot))
                .ok_or_else(|| anyhow::anyhow!("invalid local slot at {pc}"))?;
            if operation.command.starts_with("pop_") {
                *value = true;
            } else {
                ensure!(
                    *value,
                    "non-argument local {slot} is read before definite assignment at {pc}"
                );
            }
        }
        let mut successors = Vec::new();
        if operation.command != "return" {
            if operation.command != "branch" && pc + 1 < before.len() {
                successors.push(pc + 1);
            }
            match &operation.argument {
                Argument::Branch(target) => successors.push(*target),
                Argument::Switch(cases) => successors.extend(
                    semantic::selected_cases(cases, script.switch_lookup)?
                        .iter()
                        .map(|case| case.target),
                ),
                _ => {}
            }
        }
        for successor in successors {
            let state = before
                .get_mut(successor)
                .ok_or_else(|| anyhow::anyhow!("invalid successor at {pc}"))?;
            let changed = if let Some(state) = state {
                let mut changed = false;
                for (existing, incoming) in state.iter_mut().zip(&assigned) {
                    for (existing, incoming) in existing.iter_mut().zip(incoming) {
                        let merged = *existing && *incoming;
                        changed |= merged != *existing;
                        *existing = merged;
                    }
                }
                changed
            } else {
                *state = Some(assigned.clone());
                true
            };
            if changed {
                pending.push_back(successor);
            }
        }
    }
    Ok(())
}
