//! Typed source inspection uses reviewed traffic facts. Its private projection
//! is never encoded or executed as a target script.
use crate::{
    profile::{AnalysisContract, Book},
    semantic::{self, Argument, Lane},
};
use anyhow::{Result, ensure};
use native910::{
    config::ConfigTypes,
    dataflow::{self, AnalysisOptions, Constant, EntryContext, LocalDefaults, Value},
    script::{CompiledScript, Counts, Instruction, Operand, SwitchCase},
    semantics::Effect,
};
use serde::Serialize;
use std::collections::BTreeMap;

const INTEGER_LANE: usize = 0;
const OBJECT_LANE: usize = 1;
const LONG_LANE: usize = 2;
const STRING_DESCRIPTOR: u16 = b's' as u16;
const LONG_DESCRIPTOR: u16 = b'l' as u16;
const ONE_VALUE: usize = 1;
const ENUM_KEY_DEPTH: usize = 0;
const ENUM_ID_DEPTH: usize = 1;
const ENUM_OUTPUT_DEPTH: usize = 2;
const ENUM_INPUT_DEPTH: usize = 3;
const UNSUPPORTED_COMMAND: &str = "unreviewed_source_traffic";

pub struct Inspection {
    pub callbacks: BTreeMap<(u32, usize), Callback>,
    pub diagnostics: Vec<Diagnostic>,
    pub database_fields: BTreeMap<(u32, usize), DatabaseField>,
    pub enum_uses: BTreeMap<(u32, usize), EnumUse>,
    pub font_uses: BTreeMap<(u32, usize), FontUse>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FontAccess {
    Read,
    Write,
}

#[derive(Serialize)]
pub struct FontUse {
    pub access: FontAccess,
    pub caller: u32,
    pub instruction: usize,
    pub explicit_component: bool,
    pub font: Option<ValueFact>,
    pub component: Option<ValueFact>,
}

#[derive(Serialize)]
pub struct EnumUse {
    pub caller: u32,
    pub instruction: usize,
    pub input_type: Option<i32>,
    pub output_type: Option<i32>,
    pub enum_id: Option<i32>,
    pub key: Option<i32>,
    pub pushes: Option<[u16; 3]>,
}

#[derive(Serialize)]
pub struct DatabaseField {
    pub caller: u32,
    pub instruction: usize,
    pub field: Option<i32>,
    pub types: Vec<u16>,
    pub pushes: Option<[u16; 3]>,
}

#[derive(Serialize)]
pub struct Diagnostic {
    pub script: u32,
    pub instruction: usize,
    pub opcode: Option<u16>,
    pub operation: Option<String>,
    pub reason: String,
}

#[derive(Serialize)]
pub struct Callback {
    pub caller: u32,
    pub instruction: usize,
    pub opcode: u16,
    pub status: CallbackStatus,
    pub descriptor: Option<String>,
    pub head: Option<ValueFact>,
    pub component: Option<ValueFact>,
    /// Values copied by the installer, before the event runtime routes actual
    /// object tags and substitutes event tokens into callback local slots.
    pub arguments: Vec<CallbackArgument>,
    pub trigger_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transmit_domain: Option<u8>,
    pub triggers: Vec<ValueFact>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallbackStatus {
    Installed { callee: u32 },
    Cleared,
    Dormant,
    Trapped,
    Unreachable,
    Unresolved { reason: String },
}

#[derive(Serialize)]
pub struct CallbackArgument {
    pub descriptor_unit: u16,
    pub stack_lane: Lane,
    pub value: ValueFact,
}

#[derive(Serialize)]
pub struct ValueFact {
    pub constant: Option<Literal>,
    pub nonnull_string: bool,
    pub argument: Option<FormalArgument>,
    pub definitions: Vec<usize>,
}

#[derive(Serialize)]
pub struct FormalArgument {
    pub lane: Lane,
    pub slot: u16,
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Literal {
    Int(i32),
    String(String),
    Long(i64),
    Null,
}

impl From<&Value> for ValueFact {
    fn from(value: &Value) -> Self {
        Self {
            constant: value.constant.as_ref().map(|constant| match constant {
                Constant::Int(value) => Literal::Int(*value),
                Constant::String(value) => Literal::String(value.clone()),
                Constant::Long(value) => Literal::Long(*value),
                Constant::Null => Literal::Null,
            }),
            nonnull_string: value.nonnull_string,
            argument: value.argument.map(|(lane, slot)| FormalArgument {
                lane: lane_name(lane),
                slot,
            }),
            definitions: value.definitions.iter().copied().collect(),
        }
    }
}

fn lane_name(lane: usize) -> Lane {
    match lane {
        INTEGER_LANE => Lane::Int,
        OBJECT_LANE => Lane::Object,
        _ => Lane::Long,
    }
}

pub fn inspect(scripts: &BTreeMap<u32, semantic::Script>, book: &Book) -> Result<Inspection> {
    inspect_with_database(scripts, book, None)
}
pub fn inspect_with_database(
    scripts: &BTreeMap<u32, semantic::Script>,
    book: &Book,
    database: Option<&crate::database::Definitions>,
) -> Result<Inspection> {
    let mut options = AnalysisOptions {
        local_defaults: book
            .profile
            .analysis_locals
            .map_or(LocalDefaults::Unknown, Into::into),
        ..AnalysisOptions::default()
    };
    for opcode in &book.profile.opcodes {
        match &opcode.analysis {
            Some(AnalysisContract::Enum { string_type }) => {
                options.enum_string_types.insert(opcode.id, *string_type);
            }
            Some(AnalysisContract::DatabaseField) => {
                if let Some(database) = database {
                    options
                        .database_fields
                        .insert(opcode.id, database.flow_fields()?);
                }
            }
            Some(AnalysisContract::Hook { codec, .. }) => {
                options.hooks.insert(opcode.id, (*codec).into());
            }
            Some(AnalysisContract::Fixed { pops, pushes }) => {
                options.traffic.insert(
                    opcode.id,
                    Effect::Fixed {
                        pops: *pops,
                        pushes: *pushes,
                    },
                );
            }
            _ => {}
        }
    }
    let projected = scripts
        .iter()
        .map(|(id, script)| Ok((i32::try_from(*id)?, project(script, book)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let configs = ConfigTypes::empty();
    let (summaries, values) =
        dataflow::infer_summaries_with_options(&projected, &configs, &options);
    let mut inspection = Inspection {
        callbacks: BTreeMap::new(),
        diagnostics: Vec::new(),
        database_fields: BTreeMap::new(),
        enum_uses: BTreeMap::new(),
        font_uses: BTreeMap::new(),
    };
    for (id, source) in scripts {
        let script = &projected[&i32::try_from(*id)?];
        let analysis = dataflow::analyze_with_options(
            script,
            &projected,
            &summaries,
            &values,
            &configs,
            &EntryContext::default(),
            &options,
        );
        if let Some((instruction, reason)) = analysis.failure {
            inspection.diagnostics.push(Diagnostic {
                script: *id,
                instruction,
                opcode: source
                    .instructions
                    .get(instruction)
                    .map(|instruction| instruction.wire.opcode),
                operation: source
                    .instructions
                    .get(instruction)
                    .and_then(|instruction| instruction.operation.as_ref())
                    .map(|operation| operation.command.clone()),
                reason: format!("{reason:?}"),
            });
        }
        for (pc, instruction) in source.instructions.iter().enumerate() {
            let Some(operation) = &instruction.operation else {
                continue;
            };
            if matches!(
                operation.command.as_str(),
                "db_getfield" | "db_getfieldcount"
            ) {
                let field = analysis.before[pc].as_ref().and_then(|state| {
                    state.int_from_top(usize::from(operation.command == "db_getfield"))
                });
                let types = field
                    .zip(database)
                    .and_then(|(field, database)| database.database().field_types(field).ok())
                    .map_or_else(Vec::new, <[u16]>::to_vec);
                let pushes = if operation.command == "db_getfieldcount" {
                    Some([ONE_VALUE as u16, 0, 0])
                } else {
                    field
                        .and_then(|field| {
                            options
                                .database_fields
                                .get(&instruction.wire.opcode)
                                .and_then(|fields| fields.get(&field))
                        })
                        .and_then(|traffic| match traffic.effect {
                            Effect::Fixed { pushes, .. } => Some(pushes),
                            _ => None,
                        })
                };
                inspection.database_fields.insert(
                    (*id, pc),
                    DatabaseField {
                        caller: *id,
                        instruction: pc,
                        field,
                        types,
                        pushes,
                    },
                );
            }
            if matches!(
                operation.command.as_str(),
                "cc_settextfont"
                    | "if_settextfont"
                    | "cc_getfontgraphic"
                    | "if_getfontgraphic"
                    | "cc_getfontmetrics"
                    | "if_getfontmetrics"
            ) {
                let explicit_component = operation.command.starts_with("if_");
                let reading = matches!(
                    operation.command.as_str(),
                    "cc_getfontgraphic"
                        | "if_getfontgraphic"
                        | "cc_getfontmetrics"
                        | "if_getfontmetrics"
                );
                let state = analysis.before[pc].as_ref();
                let value = |depth: usize| {
                    state.and_then(|state| {
                        let ints = &state.stacks[INTEGER_LANE];
                        ints.get(ints.len().checked_sub(ONE_VALUE + depth)?)
                            .map(ValueFact::from)
                    })
                };
                inspection.font_uses.insert(
                    (*id, pc),
                    FontUse {
                        access: if reading {
                            FontAccess::Read
                        } else {
                            FontAccess::Write
                        },
                        caller: *id,
                        instruction: pc,
                        explicit_component,
                        font: (!reading)
                            .then(|| value(usize::from(explicit_component)))
                            .flatten(),
                        component: explicit_component
                            .then(|| value(usize::default()))
                            .flatten(),
                    },
                );
            }
            if operation.command == "enum" {
                let state = analysis.before[pc].as_ref();
                let reaching = |depth| state.and_then(|state| state.int_from_top(depth));
                let pushes = state.and_then(|state| {
                    match dataflow::resolved_effect_with_options(
                        &script.code[pc],
                        state,
                        &configs,
                        &options,
                    ) {
                        Effect::Fixed { pushes, .. } => Some(pushes),
                        _ => None,
                    }
                });
                inspection.enum_uses.insert(
                    (*id, pc),
                    EnumUse {
                        caller: *id,
                        instruction: pc,
                        input_type: reaching(ENUM_INPUT_DEPTH),
                        output_type: reaching(ENUM_OUTPUT_DEPTH),
                        enum_id: reaching(ENUM_ID_DEPTH),
                        key: reaching(ENUM_KEY_DEPTH),
                        pushes,
                    },
                );
            }
            if !book.opcode(instruction.wire.opcode)?.is_hook() {
                continue;
            }
            let mut callback = Callback {
                caller: *id,
                instruction: pc,
                opcode: instruction.wire.opcode,
                status: CallbackStatus::Unresolved {
                    reason: "hook traffic is unreviewed".into(),
                },
                descriptor: None,
                head: None,
                component: None,
                arguments: Vec::new(),
                trigger_count: None,
                transmit_domain: None,
                triggers: Vec::new(),
            };
            if let Some((blocked_pc, reason)) = analysis.failure {
                callback.status = CallbackStatus::Unresolved {
                    reason: format!("source flow blocked at {blocked_pc}: {reason:?}"),
                };
            } else if let Some(state) = &analysis.before[pc] {
                if let Some(contract @ AnalysisContract::Hook { .. }) =
                    &book.opcode(instruction.wire.opcode)?.analysis
                {
                    recover(
                        &mut callback,
                        &script.code[pc],
                        state,
                        contract,
                        &configs,
                        &options,
                    )?;
                }
            } else {
                callback.status = CallbackStatus::Unreachable;
            }
            inspection.callbacks.insert((*id, pc), callback);
        }
    }
    Ok(inspection)
}

fn project(script: &semantic::Script, book: &Book) -> Result<CompiledScript> {
    let counts = |counts: crate::wire::Counts| Counts {
        int: counts.int,
        obj: counts.object,
        long: counts.long,
    };
    let code = script
        .instructions
        .iter()
        .map(|instruction| {
            let mut projected = Instruction {
                opcode: instruction.wire.opcode,
                command: UNSUPPORTED_COMMAND.into(),
                operand: Operand::Byte(u8::default()),
            };
            let row = book.opcode(instruction.wire.opcode)?;
            let Some(operation) = &instruction.operation else {
                return Ok(projected);
            };
            let Some(contract) = &row.analysis else {
                return Ok(projected);
            };
            let supported = match contract {
                AnalysisContract::Core => core_command(&operation.command),
                AnalysisContract::DatabaseField => operation.command == "db_getfield",
                AnalysisContract::Enum { .. } => operation.command == "enum",
                AnalysisContract::Fixed { .. } => true,
                AnalysisContract::Hook { .. } => {
                    contract.hook_component(&operation.command).is_some()
                }
            };
            if !supported {
                return Ok(projected);
            }
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
                    semantic::selected_cases(cases, script.switch_lookup)?
                        .iter()
                        .map(|case| {
                            Ok(SwitchCase {
                                value: case.value,
                                target: i32::try_from(case.target)?,
                            })
                        })
                        .collect::<Result<_>>()?,
                ),
                Argument::Script(id) => Operand::Script(i32::try_from(*id)?),
                Argument::Count(count) if u16::try_from(*count).is_ok() => Operand::Count(*count),
                Argument::Byte(value) => Operand::Byte(*value),
                _ if matches!(contract, AnalysisContract::Fixed { .. }) => {
                    Operand::Byte(u8::default())
                }
                _ => return Ok(projected),
            };
            projected.command = if let Some(explicit) = contract.hook_component(&operation.command)
            {
                // Addressing and traffic are source facts. The event identity
                // does not affect the private stack projection.
                if explicit { "if_setonop" } else { "cc_setonop" }.into()
            } else if matches!(
                contract,
                AnalysisContract::Fixed { .. }
                    | AnalysisContract::DatabaseField
                    | AnalysisContract::Enum { .. }
            ) {
                "reviewed_source_traffic".into()
            } else {
                operation.command.clone()
            };
            projected.operand = operand;
            Ok(projected)
        })
        .collect::<Result<_>>()?;
    Ok(CompiledScript {
        name: None,
        locals: counts(script.locals),
        args: counts(script.args),
        code,
    })
}

fn core_command(command: &str) -> bool {
    matches!(
        command,
        "push_constant_int"
            | "push_constant_string"
            | "push_long_constant"
            | "push_int_local"
            | "pop_int_local"
            | "push_string_local"
            | "pop_string_local"
            | "push_long_local"
            | "pop_long_local"
            | "switch"
            | "gosub_with_params"
            | "return"
            | "add"
            | "subtract"
            | "multiply"
            | "join_string"
            | "if_settext"
            | "if_setop"
            | "cc_deleteall"
            | "if_sethide"
    ) || native910::script::is_branch_command(command)
}

fn recover(
    callback: &mut Callback,
    instruction: &Instruction,
    state: &dataflow::State,
    contract: &AnalysisContract,
    configs: &ConfigTypes,
    options: &AnalysisOptions,
) -> Result<()> {
    let AnalysisContract::Hook {
        codec,
        clear_script_id,
        inactive_script_ids,
        transmit_domain,
        ..
    } = contract
    else {
        return Ok(());
    };
    let Some(Constant::String(descriptor)) = state.stacks[OBJECT_LANE]
        .last()
        .and_then(|value| value.constant.as_ref())
    else {
        return Ok(());
    };
    callback.descriptor = Some(descriptor.clone());
    callback.transmit_domain = *transmit_domain;
    let explicit = usize::from(instruction.command.starts_with("if_"));
    if explicit != 0 {
        callback.component = state.stacks[INTEGER_LANE].last().map(ValueFact::from);
    }
    let effect = dataflow::resolved_effect_with_options(instruction, state, configs, options);
    if descriptor.ends_with('Y') {
        callback.trigger_count = state.int_from_top(explicit);
    }
    if effect == Effect::Trap {
        callback.status = CallbackStatus::Trapped;
        return Ok(());
    }
    let Effect::Fixed { pops, .. } = effect else {
        return Ok(());
    };
    let mut starts = [0; 3];
    for (lane, count) in pops.iter().enumerate() {
        starts[lane] = state.stacks[lane]
            .len()
            .checked_sub(usize::from(*count))
            .ok_or_else(|| anyhow::anyhow!("verified hook stack underflow"))?;
    }
    let head = &state.stacks[INTEGER_LANE][starts[INTEGER_LANE]];
    callback.head = Some(ValueFact::from(head));
    let codec: dataflow::HookCodec = (*codec).into();
    let units = codec
        .signature_units(descriptor, callback.trigger_count, None)
        .ok_or_else(|| anyhow::anyhow!("fixed hook traffic has an unresolved descriptor"))?;
    let mut cursors = starts;
    cursors[INTEGER_LANE] += ONE_VALUE;
    for unit in units {
        let lane = match unit {
            STRING_DESCRIPTOR => OBJECT_LANE,
            LONG_DESCRIPTOR => LONG_LANE,
            _ => INTEGER_LANE,
        };
        let value = state.stacks[lane]
            .get(cursors[lane])
            .ok_or_else(|| anyhow::anyhow!("verified hook argument is absent"))?;
        callback.arguments.push(CallbackArgument {
            descriptor_unit: unit,
            stack_lane: lane_name(lane),
            value: ValueFact::from(value),
        });
        cursors[lane] += ONE_VALUE;
    }
    if let Some(count) = callback.trigger_count {
        let trigger_count = usize::try_from(count.max(0))?;
        let end = cursors[INTEGER_LANE]
            .checked_add(trigger_count)
            .ok_or_else(|| anyhow::anyhow!("hook trigger range overflow"))?;
        ensure!(
            end < state.stacks[INTEGER_LANE].len(),
            "hook trigger count is absent"
        );
        callback.triggers = state.stacks[INTEGER_LANE][cursors[INTEGER_LANE]..end]
            .iter()
            .map(ValueFact::from)
            .collect();
    }
    callback.status = match head.constant {
        Some(Constant::Int(id)) if id == *clear_script_id => CallbackStatus::Cleared,
        Some(Constant::Int(id)) if inactive_script_ids.contains(&id) => CallbackStatus::Dormant,
        Some(Constant::Int(id)) if u32::try_from(id).is_ok() => CallbackStatus::Installed {
            callee: u32::try_from(id)?,
        },
        _ => CallbackStatus::Unresolved {
            reason: "callback head is not a proven script ID".into(),
        },
    };
    Ok(())
}

#[cfg(test)]
pub(crate) fn verify_callbacks(book: &Book) {
    use crate::{
        import910::{DependencyGapKind, inspect_closure},
        wire,
    };
    const CALLER: u32 = 1;
    const CALLEE: u32 = 2;
    const OTHER_CALLEE: u32 = 3;
    const HELPER: u32 = 4;
    const COMPONENT_SLOT: i32 = 0;
    const HEAD_SLOT: i32 = 1;
    const TWO_INT_LOCALS: u16 = 2;
    const ONE_ARGUMENT: u16 = 1;
    const FIRST_ARGUMENT: i32 = 11;
    const SECOND_ARGUMENT: i32 = 12;
    const ORDINARY_VALUE: i32 = 13;
    const SCRAMBLE_SHIFT: u16 = 5000;
    const CALLBACK_PC: usize = 9;
    let id = |name: &str| {
        book.profile
            .opcodes
            .iter()
            .find(|row| row.command.as_deref() == Some(name))
            .unwrap()
            .id
    };
    let instruction = |name: &str, operand| wire::Instruction {
        opcode: id(name),
        operand,
    };
    let push_int = |value: i32| instruction("push_constant", wire::Operand::ConstantInt(value));
    let push_text =
        |bytes: &[u8]| instruction("push_constant", wire::Operand::ConstantString(bytes.into()));
    let return_instruction = || instruction("return", wire::Operand::Byte(u8::default()));
    let setter = || instruction("if_setonop", wire::Operand::Byte(u8::default()));
    let component = || instruction("push_int_local", wire::Operand::Int(COMPONENT_SLOT));
    let head_slot = || instruction("push_int_local", wire::Operand::Int(HEAD_SLOT));
    let make = |code| wire::Script {
        name: vec![],
        locals: wire::Counts {
            int: TWO_INT_LOCALS,
            object: ONE_ARGUMENT,
            long: ONE_ARGUMENT,
        },
        args: wire::Counts {
            int: ONE_ARGUMENT,
            object: ONE_ARGUMENT,
            long: ONE_ARGUMENT,
        },
        code,
        switches: vec![],
    };
    let callee = make(vec![return_instruction()]);
    let mut caller = make(vec![
        push_int(i32::try_from(CALLEE).unwrap()),
        instruction("pop_int_local", wire::Operand::Int(HEAD_SLOT)),
        head_slot(), // An ordinary integer use survives independently of the hook head.
        head_slot(),
        component(),
        instruction("push_string_local", wire::Operand::Int(COMPONENT_SLOT)),
        instruction("push_long_local", wire::Operand::Int(COMPONENT_SLOT)),
        push_text(b"isl"),
        component(),
        setter(),
        return_instruction(),
    ]);
    let encode = |script: &wire::Script| wire::encode(script, book).unwrap();
    let inspect_caller = |script: &wire::Script| {
        inspect_closure(
            &BTreeMap::from([(CALLER, encode(script)), (CALLEE, encode(&callee))]),
            book,
        )
        .unwrap()
    };
    let inspection = inspect_caller(&caller);
    assert!(inspection.dependency_complete);
    assert!(inspection.source_analysis.is_empty());
    let callback = &inspection.callbacks[0];
    assert_eq!(callback.instruction, CALLBACK_PC);
    assert!(matches!(
        callback.status,
        CallbackStatus::Installed { callee: CALLEE }
    ));
    assert_eq!(callback.head.as_ref().unwrap().definitions, vec![0]);
    assert_eq!(
        callback
            .component
            .as_ref()
            .unwrap()
            .argument
            .as_ref()
            .unwrap()
            .lane,
        Lane::Int
    );
    for (argument, lane) in callback
        .arguments
        .iter()
        .zip([Lane::Int, Lane::Object, Lane::Long])
    {
        assert_eq!(argument.stack_lane, lane);
        assert_eq!(argument.value.argument.as_ref().unwrap().lane, lane);
        assert_eq!(
            argument.value.argument.as_ref().unwrap().slot,
            u16::default()
        );
    }
    assert!(!callback.arguments[OBJECT_LANE].value.nonnull_string);
    assert!(callback.arguments[OBJECT_LANE].value.constant.is_none());
    assert_eq!(
        wire::encode(&inspection.scripts[&CALLER].to_wire(), book).unwrap(),
        encode(&caller)
    );

    let missing = inspect_closure(&BTreeMap::from([(CALLER, encode(&caller))]), book).unwrap();
    assert!(matches!(
        missing.unresolved_dependencies[0].reason,
        DependencyGapKind::MissingCallbackCallee
    ));
    let mut scrambled = book.profile.clone();
    for row in &mut scrambled.opcodes {
        row.id += SCRAMBLE_SHIFT
    }
    let scrambled = Book::parse(&serde_json::to_vec(&scrambled).unwrap()).unwrap();
    let shifted = |script: &wire::Script| {
        let mut script = script.clone();
        for instruction in &mut script.code {
            instruction.opcode += SCRAMBLE_SHIFT
        }
        wire::encode(&script, &scrambled).unwrap()
    };
    let shifted = inspect_closure(
        &BTreeMap::from([(CALLER, shifted(&caller)), (CALLEE, shifted(&callee))]),
        &scrambled,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&shifted.callbacks[0].arguments).unwrap(),
        serde_json::to_value(&callback.arguments).unwrap()
    );
    assert!(matches!(
        shifted.callbacks[0].status,
        CallbackStatus::Installed { callee: CALLEE }
    ));

    let AnalysisContract::Hook {
        clear_script_id,
        inactive_script_ids,
        ..
    } = book
        .opcode(id("if_setonop"))
        .unwrap()
        .analysis
        .as_ref()
        .unwrap()
    else {
        panic!("hook contract")
    };
    for (head, dormant) in [(*clear_script_id, false), (inactive_script_ids[0], true)] {
        caller.code[0] = push_int(head);
        let inspection = inspect_caller(&caller);
        assert!(inspection.dependency_complete);
        assert!(if dormant {
            matches!(inspection.callbacks[0].status, CallbackStatus::Dormant)
        } else {
            matches!(inspection.callbacks[0].status, CallbackStatus::Cleared)
        });
    }
    // Both a formal head and a reused non-argument local remain unknown.
    for head in [component(), head_slot()] {
        caller.code[0] = head;
        let inspection = inspect_caller(&caller);
        assert!(matches!(
            inspection.callbacks[0].status,
            CallbackStatus::Unresolved { .. }
        ));
        assert!(
            inspection.callbacks[0]
                .head
                .as_ref()
                .unwrap()
                .constant
                .is_none()
        );
    }

    // UTF-8 descriptor bytes each consume an integer; a UTF-16 projection
    // would mistake FIRST_ARGUMENT for this callback's head.
    caller = make(vec![
        push_int(i32::try_from(CALLEE).unwrap()),
        push_int(FIRST_ARGUMENT),
        push_int(SECOND_ARGUMENT),
        push_text(&[0xe9]),
        component(),
        setter(),
        return_instruction(),
    ]);
    let inspection = inspect_caller(&caller);
    let callback = &inspection.callbacks[0];
    assert!(matches!(
        callback.status,
        CallbackStatus::Installed { callee: CALLEE }
    ));
    assert_eq!(
        callback
            .arguments
            .iter()
            .map(|argument| argument.descriptor_unit)
            .collect::<Vec<_>>(),
        "é".bytes().map(u16::from).collect::<Vec<_>>()
    );

    for count in [i32::default(), i32::MIN] {
        caller = make(vec![
            push_int(i32::try_from(CALLEE).unwrap()),
            push_int(FIRST_ARGUMENT),
            push_int(SECOND_ARGUMENT),
            push_int(count),
            push_text(b"iY"),
            component(),
            setter(),
            return_instruction(),
        ]);
        let inspection = inspect_caller(&caller);
        let callback = &inspection.callbacks[0];
        assert!(matches!(
            callback.status,
            CallbackStatus::Installed { callee: CALLEE }
        ));
        assert_eq!(callback.trigger_count, Some(count));
        assert_eq!(
            callback
                .arguments
                .iter()
                .map(|argument| argument.descriptor_unit)
                .collect::<Vec<_>>(),
            b"iY".iter().copied().map(u16::from).collect::<Vec<_>>()
        );
        assert!(callback.triggers.is_empty());
    }
    // A positive trigger count traps even when the eventual head would clear.
    for head in [i32::try_from(CALLEE).unwrap(), *clear_script_id] {
        caller = make(vec![
            push_int(head),
            push_int(FIRST_ARGUMENT),
            push_int(SECOND_ARGUMENT),
            push_int(i32::from(ONE_ARGUMENT)),
            push_text(b"iY"),
            component(),
            setter(),
            return_instruction(),
        ]);
        let inspection = inspect_caller(&caller);
        assert!(inspection.dependency_complete);
        assert!(matches!(
            inspection.callbacks[0].status,
            CallbackStatus::Trapped
        ));
    }

    // A callee reading reused storage cannot acquire an invented zero summary.
    let helper = make(vec![head_slot(), return_instruction()]);
    caller = make(vec![
        instruction(
            "gosub_with_params",
            wire::Operand::Int(i32::try_from(HELPER).unwrap()),
        ),
        push_text(b""),
        component(),
        setter(),
        return_instruction(),
    ]);
    let mut helper = helper;
    helper.args = wire::Counts::default();
    let inspection = inspect_closure(
        &BTreeMap::from([(CALLER, encode(&caller)), (HELPER, encode(&helper))]),
        book,
    )
    .unwrap();
    assert!(matches!(
        inspection.callbacks[0].status,
        CallbackStatus::Unresolved { .. }
    ));
    assert!(
        inspection.callbacks[0]
            .head
            .as_ref()
            .unwrap()
            .constant
            .is_none()
    );
    helper.code[0] = push_int(i32::try_from(CALLEE).unwrap());
    let inspection = inspect_closure(
        &BTreeMap::from([
            (CALLER, encode(&caller)),
            (HELPER, encode(&helper)),
            (CALLEE, encode(&callee)),
        ]),
        book,
    )
    .unwrap();
    assert!(inspection.dependency_complete);
    assert!(matches!(
        inspection.callbacks[0].status,
        CallbackStatus::Installed { callee: CALLEE }
    ));

    // Agreeing branches retain both origins; disagreeing heads stay unresolved.
    const RIGHT_BRANCH_OFFSET: i32 = 2;
    const JOIN_BRANCH_OFFSET: i32 = 1;
    for right in [CALLEE, OTHER_CALLEE] {
        caller = make(vec![
            component(),
            instruction("branch_if_true", wire::Operand::Int(RIGHT_BRANCH_OFFSET)),
            push_int(i32::try_from(CALLEE).unwrap()),
            instruction("branch", wire::Operand::Int(JOIN_BRANCH_OFFSET)),
            push_int(i32::try_from(right).unwrap()),
            push_text(b""),
            component(),
            setter(),
            return_instruction(),
        ]);
        let inspection = inspect_caller(&caller);
        if right == CALLEE {
            assert!(matches!(
                inspection.callbacks[0].status,
                CallbackStatus::Installed { callee: CALLEE }
            ));
            assert_eq!(
                inspection.callbacks[0].head.as_ref().unwrap().definitions,
                vec![2, 4]
            );
        } else {
            assert!(matches!(
                inspection.callbacks[0].status,
                CallbackStatus::Unresolved { .. }
            ));
        }
    }
    let unknown = book
        .profile
        .opcodes
        .iter()
        .find(|row| row.command.is_none() && row.encoding == crate::profile::Encoding::Byte)
        .unwrap()
        .id;
    caller = make(vec![
        push_int(i32::try_from(CALLEE).unwrap()),
        push_text(b""),
        component(),
        setter(),
        wire::Instruction {
            opcode: unknown,
            operand: wire::Operand::Byte(u8::default()),
        },
        return_instruction(),
    ]);
    let inspection = inspect_caller(&caller);
    assert!(matches!(
        inspection.callbacks[0].status,
        CallbackStatus::Unresolved { .. }
    ));
    assert!(inspection.callbacks[0].head.is_none());
    assert!(!inspection.source_analysis.is_empty());
    // An unknown descriptor is diagnosed rather than consuming a guessed shape.
    caller.code[1] = instruction("push_string_local", wire::Operand::Int(COMPONENT_SLOT));
    caller.code.remove(4);
    let inspection = inspect_caller(&caller);
    assert!(matches!(
        inspection.callbacks[0].status,
        CallbackStatus::Unresolved { .. }
    ));
    caller = make(vec![
        instruction("branch", wire::Operand::Int(i32::from(ONE_ARGUMENT))),
        setter(),
        return_instruction(),
    ]);
    let inspection = inspect_caller(&caller);
    assert!(inspection.dependency_complete);
    assert!(matches!(
        inspection.callbacks[0].status,
        CallbackStatus::Unreachable
    ));

    // Unreviewed identities cannot borrow analysis rules from a matching name.
    let mut unreviewed = book.profile.clone();
    unreviewed
        .opcodes
        .iter_mut()
        .find(|row| row.id == id("push_constant"))
        .unwrap()
        .analysis = None;
    let unreviewed = Book::parse(&serde_json::to_vec(&unreviewed).unwrap()).unwrap();
    caller = make(vec![
        push_int(ORDINARY_VALUE),
        push_text(b""),
        component(),
        setter(),
        return_instruction(),
    ]);
    let inspection =
        inspect_closure(&BTreeMap::from([(CALLER, encode(&caller))]), &unreviewed).unwrap();
    assert!(matches!(
        inspection.callbacks[0].status,
        CallbackStatus::Unresolved { .. }
    ));
}

#[cfg(test)]
pub(crate) fn verify_retained_callbacks(book: &Book) {
    use crate::{import910::inspect_closure, wire};
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct Recording {
        client_md5: String,
        cases: Vec<Case>,
        child_selection: ChildRecording,
    }
    #[derive(Deserialize)]
    struct ChildRecording {
        client_md5: String,
        cases: Vec<ChildCase>,
    }
    #[derive(Deserialize)]
    struct ChildCase {
        input: ChildInput,
        result: ChildObserved,
    }
    #[derive(Deserialize)]
    struct ChildInput {
        component: i32,
        child: i32,
        selector: u8,
    }
    #[derive(Deserialize)]
    struct ChildObserved {
        ints: Vec<i32>,
    }
    #[derive(Deserialize)]
    struct Case {
        name: String,
        input: Inputs,
        result: Observed,
    }
    #[derive(Deserialize)]
    struct Inputs {
        descriptor: String,
        wire: Vec<u8>,
        count: Option<i32>,
        prior: Vec<i32>,
        new: Vec<i32>,
    }
    #[derive(Deserialize)]
    struct Observed {
        descriptor: Vec<u16>,
        ints: Vec<i32>,
        triggers: Vec<i32>,
        objects: i32,
    }
    const CALLER: u32 = 1;
    const CALLEE: u32 = 2;
    const ONE_ARGUMENT: u16 = 1;
    const ARGUMENT_VALUE: i32 = 11;
    let record: Recording =
        serde_json::from_slice(include_bytes!("../fixtures/hook_descriptors.json")).unwrap();
    assert_eq!(record.client_md5, book.profile.client_md5);
    let row = book
        .profile
        .opcodes
        .iter()
        .find(|row| row.command.as_deref() == Some("active_transmit_hook"))
        .unwrap();
    let Some(AnalysisContract::Hook { codec, .. }) = &row.analysis else {
        panic!("missing recorded hook contract")
    };
    let codec: dataflow::HookCodec = (*codec).into();
    let instruction = |name: &str, operand| wire::Instruction {
        opcode: book
            .profile
            .opcodes
            .iter()
            .find(|row| row.command.as_deref() == Some(name))
            .unwrap()
            .id,
        operand,
    };
    let push = |value| instruction("push_constant", wire::Operand::ConstantInt(value));
    let ret = || instruction("return", wire::Operand::Byte(u8::default()));
    assert_eq!(record.child_selection.client_md5, book.profile.client_md5);
    let find_row = book
        .profile
        .opcodes
        .iter()
        .find(|row| row.command.as_deref() == Some("cc_find"))
        .unwrap();
    let Some(AnalysisContract::Fixed { pops, pushes }) = &find_row.analysis else {
        panic!("selection traffic")
    };
    let options = AnalysisOptions {
        traffic: BTreeMap::from([(
            find_row.id,
            Effect::Fixed {
                pops: *pops,
                pushes: *pushes,
            },
        )]),
        ..AnalysisOptions::default()
    };
    let configs = ConfigTypes::empty();
    let native = native910::opcode::OpcodeBook::embedded().unwrap();
    let adapter = crate::lower910::Adapter::parse(
        include_bytes!("../../../revisions/950/cs2/950-1-to-910.json"),
        book,
        &native,
    )
    .unwrap();
    const CHILD_USE: usize = 3;
    for case in record.child_selection.cases {
        let source = wire::Script {
            name: Vec::new(),
            args: wire::Counts::default(),
            locals: wire::Counts::default(),
            switches: Vec::new(),
            code: vec![
                push(case.result.ints[0]),
                push(case.input.component),
                push(case.input.child),
                instruction("cc_find", wire::Operand::Byte(case.input.selector)),
                ret(),
            ],
        };
        let source = semantic::normalize(&wire::encode(&source, book).unwrap(), book).unwrap();
        let target = adapter.target_for(&source.instructions[CHILD_USE]).unwrap();
        let generated = crate::bridge::generate(
            crate::bridge::Request {
                font: None,
                target,
                operand: &native910::script::Operand::Byte(case.input.selector),
                callback: None,
                triggers: None,
                component: None,
                policy: None,
                links: &BTreeMap::new(),
                name: "recorded_flat_child",
                database: None,
                enums: None,
                variable: None,
            },
            &native,
        )
        .unwrap();
        assert_eq!(generated.script.args.int, pops[INTEGER_LANE]);
        assert_eq!(
            generated.script.code[generated.requirement.pc].operand,
            native910::script::Operand::Byte(u8::from(case.input.selector != u8::default()))
        );
        let script = project(&source, book).unwrap();
        let analysis = dataflow::analyze_with_options(
            &script,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &configs,
            &EntryContext::default(),
            &options,
        );
        assert!(analysis.failure.is_none());
        let state = analysis.before.last().unwrap().as_ref().unwrap();
        assert_eq!(state.stacks[INTEGER_LANE].len(), case.result.ints.len());
        assert_eq!(
            state.stacks[INTEGER_LANE][0].constant,
            Some(Constant::Int(case.result.ints[0]))
        );
        assert!(
            state.stacks[INTEGER_LANE]
                .last()
                .unwrap()
                .constant
                .is_none()
        );
        let wire = source.to_wire();
        assert_eq!(
            wire.code[wire.code.len() - usize::from(ONE_ARGUMENT) - usize::from(ONE_ARGUMENT)]
                .operand,
            wire::Operand::Byte(case.input.selector)
        );
    }
    for case in record.cases {
        let input = case.input;
        let units = codec
            .signature_units(
                &input.descriptor,
                input.count,
                Some(!input.prior.is_empty()),
            )
            .unwrap();
        assert_eq!(units, case.result.descriptor, "{}", case.name);
        let positive = input.descriptor.ends_with('Y')
            && input.count.is_some_and(|count| count > i32::default());
        assert_eq!(
            if positive { &input.new } else { &input.prior },
            &case.result.triggers,
            "{}",
            case.name
        );
        assert_eq!(case.result.objects, i32::default());
        let flow_units = codec.signature_units(&input.descriptor, input.count, None);
        assert_eq!(flow_units.is_some(), positive, "{}", case.name);
        let argument_count = u16::try_from(units.len()).unwrap();
        let mut code = case
            .result
            .ints
            .iter()
            .copied()
            .map(push)
            .collect::<Vec<_>>();
        code.push(push(i32::try_from(CALLEE).unwrap()));
        code.extend((u16::default()..argument_count).map(|_| push(ARGUMENT_VALUE)));
        code.extend(input.new.iter().copied().map(push));
        if let Some(count) = input.count {
            code.push(push(count));
        }
        code.push(instruction(
            "push_constant",
            wire::Operand::ConstantString(input.wire),
        ));
        let setter_pc = code.len();
        code.push(wire::Instruction {
            opcode: row.id,
            operand: wire::Operand::Byte(u8::default()),
        });
        code.push(ret());
        let source = wire::Script {
            name: Vec::new(),
            args: wire::Counts::default(),
            locals: wire::Counts::default(),
            code,
            switches: Vec::new(),
        };
        let args = wire::Counts {
            int: argument_count,
            ..wire::Counts::default()
        };
        let target = wire::Script {
            name: Vec::new(),
            args,
            locals: args,
            code: vec![ret()],
            switches: Vec::new(),
        };
        let scripts = BTreeMap::from([
            (CALLER, wire::encode(&source, book).unwrap()),
            (CALLEE, wire::encode(&target, book).unwrap()),
        ]);
        let inspection = inspect_closure(&scripts, book).unwrap();
        let callback = &inspection.callbacks[0];
        assert_eq!(callback.instruction, setter_pc);
        assert!(callback.component.is_none());
        if positive {
            assert!(inspection.dependency_complete, "{}", case.name);
            assert!(matches!(
                callback.status,
                CallbackStatus::Installed { callee: CALLEE }
            ));
            assert_eq!(callback.arguments.len(), usize::from(argument_count));
            assert_eq!(callback.triggers.len(), input.new.len());
            assert_eq!(callback.trigger_count, input.count);
            assert_eq!(inspection.callbacks.len(), usize::from(ONE_ARGUMENT));
            let missing =
                inspect_closure(&BTreeMap::from([(CALLER, scripts[&CALLER].clone())]), book)
                    .unwrap();
            assert!(!missing.dependency_complete);
        } else {
            assert!(!inspection.dependency_complete, "{}", case.name);
            assert!(matches!(callback.status, CallbackStatus::Unresolved { .. }));
        }
    }
}
