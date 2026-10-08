//! A generated procedure consumes one original host use without rewriting its
//! producers. Original instruction indices and branch destinations stay stable.
use crate::{
    flow::{Callback, CallbackStatus},
    import910::ComponentBinding,
    lower910::{CallbackPolicy, HostRequirement, Requirement, Target},
    semantic::Lane,
};
use anyhow::{Context, Result, ensure};
use native910::{
    opcode::OpcodeBook,
    script::{CompiledScript, Counts, Instruction, Operand},
};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};

const INTEGER_LANE: usize = 0;
const OBJECT_LANE: usize = 1;
const LONG_LANE: usize = 2;
const ONE_VALUE: u16 = 1;
const HEAD_SLOT: u16 = 0;

#[derive(Serialize)]
pub struct BridgeReport {
    pub source: u32,
    pub instruction: usize,
    pub target: i32,
    pub name: String,
    pub target_sha256: String,
    pub component: Option<String>,
    pub callback_source: Option<u32>,
    pub callback_target: Option<i32>,
    pub host_operation: Option<String>,
    pub resource_sha256: Option<String>,
}

#[derive(Clone)]
pub(crate) enum FontUse {
    Write(native910::execution::FontMapping),
    Mapped(Arc<[u8]>),
    Initial(native910::execution::InitialFontMapping),
}

pub(crate) struct Request<'a> {
    pub target: &'a Target,
    pub operand: &'a Operand,
    pub callback: Option<&'a Callback>,
    pub triggers: Option<&'a [i32]>,
    pub component: Option<&'a ComponentBinding>,
    pub policy: Option<&'a CallbackPolicy>,
    pub links: &'a BTreeMap<u32, i32>,
    pub name: &'a str,
    pub database: Option<DatabaseUse<'a>>,
    pub enums: Option<EnumUse<'a>>,
    pub variable: Option<crate::variable_bindings::ResolvedBit>,
    pub font: Option<FontUse>,
}

pub(crate) struct EnumUse<'a> {
    pub site: &'a crate::flow::EnumUse,
    pub definitions: &'a crate::enums::Definitions,
    pub resource: Arc<[u8]>,
}

pub(crate) struct DatabaseUse<'a> {
    pub site: &'a crate::flow::DatabaseField,
    pub definitions: &'a crate::database::Definitions,
    pub resource: Arc<[u8]>,
}

pub(crate) struct Generated {
    pub script: CompiledScript,
    pub requirement: HostRequirement,
    pub declaration: Option<(usize, ComponentBinding)>,
    pub callback: Option<(u32, i32)>,
    pub host_operation: Option<native910::execution::HostOperation>,
    pub resource: Option<Arc<[u8]>>,
}

fn instruction(book: &OpcodeBook, command: &str, operand: Operand) -> Result<Instruction> {
    Ok(Instruction {
        opcode: book.opcode_for(command)?,
        command: command.into(),
        operand,
    })
}

fn local(book: &OpcodeBook, lane: usize, slot: u16) -> Result<Instruction> {
    let command = match lane {
        INTEGER_LANE => "push_int_local",
        OBJECT_LANE => "push_string_local",
        _ => "push_long_local",
    };
    instruction(book, command, Operand::Local(i32::from(slot)))
}

fn increment(value: &mut u16) -> Result<()> {
    *value = value
        .checked_add(ONE_VALUE)
        .context("host use is too wide")?;
    Ok(())
}

pub(crate) fn generate(request: Request<'_>, book: &OpcodeBook) -> Result<Generated> {
    let Request {
        target,
        operand,
        callback,
        triggers,
        component,
        policy,
        links,
        name,
        database,
        enums,
        variable,
        font,
    } = request;
    if target.requirement == Requirement::VariableBit {
        ensure!(
            component.is_none() && callback.is_none() && database.is_none() && enums.is_none(),
            "unexpected variable binding"
        );
        let variable = variable.context("bit read has no named live binding")?;
        return Ok(Generated {
            script: CompiledScript {
                name: Some(format!("proc,{name}")),
                args: Counts::default(),
                locals: Counts::default(),
                code: vec![
                    instruction(book, &target.command, variable.operand)?,
                    instruction(book, "return", Operand::Byte(u8::default()))?,
                ],
            },
            requirement: HostRequirement {
                pc: usize::default(),
                requirement: Requirement::VariableBit,
            },
            declaration: None,
            callback: None,
            host_operation: Some(variable.operation),
            resource: None,
        });
    }
    ensure!(variable.is_none(), "unexpected variable proof");
    if target.requirement == Requirement::Enum {
        ensure!(
            component.is_none() && callback.is_none() && database.is_none(),
            "unexpected enum binding"
        );
        let enums = enums.context("enum use has no exact definitions")?;
        let output_type = enums
            .site
            .output_type
            .context("enum output type is not proven at this use")?;
        let operation = native910::execution::HostOperation::Enum {
            output_type,
            string_type: enums.definitions.library.string_type,
        };
        let native910::semantics::Effect::Fixed { pushes, .. } = operation.effect(operand) else {
            unreachable!()
        };
        ensure!(
            enums.site.pushes == Some(pushes),
            "enum resource and source traffic disagree"
        );
        const ARGUMENTS: u16 = 4;
        let args = Counts {
            int: ARGUMENTS,
            ..Counts::default()
        };
        let mut code = (0..ARGUMENTS)
            .map(|slot| local(book, INTEGER_LANE, slot))
            .collect::<Result<Vec<_>>>()?;
        let consumer = code.len();
        code.push(instruction(book, &target.command, operand.clone())?);
        code.push(instruction(book, "return", Operand::Byte(u8::default()))?);
        return Ok(Generated {
            script: CompiledScript {
                name: Some(format!("proc,{name}")),
                args,
                locals: args,
                code,
            },
            requirement: HostRequirement {
                pc: consumer,
                requirement: Requirement::Enum,
            },
            declaration: None,
            callback: None,
            host_operation: Some(operation),
            resource: Some(enums.resource),
        });
    }
    ensure!(enums.is_none(), "unexpected enum proof");
    if matches!(
        target.requirement,
        Requirement::DatabaseField | Requirement::DatabaseFieldCount
    ) {
        ensure!(
            component.is_none() && callback.is_none(),
            "unexpected database component or callback binding"
        );
        return database_bridge(
            target,
            operand,
            name,
            database.context("database use has no exact schema")?,
            book,
        );
    }
    ensure!(database.is_none(), "unexpected database proof");
    if matches!(
        target.requirement,
        Requirement::ActiveComponentText | Requirement::ExplicitComponentText
    ) {
        return text_bridge(target, operand, name, component, font, book);
    }
    ensure!(font.is_none(), "font binding on a non-text use");
    if target.requirement == Requirement::ActiveComponentPaint {
        ensure!(
            component.is_none() && callback.is_none(),
            "active paint has no explicit component binding"
        );
        let args = Counts {
            int: ONE_VALUE,
            ..Counts::default()
        };
        let operand = target
            .selector
            .context("active paint selection policy missing")?
            .convert(operand)?;
        return Ok(Generated {
            script: CompiledScript {
                name: Some(format!("proc,{name}")),
                args,
                locals: args,
                code: vec![
                    local(book, INTEGER_LANE, HEAD_SLOT)?,
                    instruction(book, &target.command, operand)?,
                    instruction(book, "return", Operand::Byte(u8::default()))?,
                ],
            },
            requirement: HostRequirement {
                pc: usize::from(ONE_VALUE),
                requirement: target.requirement,
            },
            declaration: None,
            callback: None,
            host_operation: target.host_operation,
            resource: None,
        });
    }
    if target.requirement == Requirement::PlayerVariableCallback {
        ensure!(
            component.is_none(),
            "variable callback uses its active component"
        );
        return player_callback_bridge(
            PlayerCallbackInput {
                target,
                operand,
                callback: callback.context("variable callback has no source traffic proof")?,
                triggers: triggers.context("variable callback has no named trigger bindings")?,
                policy: policy.context("callback policy absent")?,
                links,
                name,
            },
            book,
        );
    }
    ensure!(triggers.is_none(), "unexpected trigger proof");
    let mut code = Vec::new();
    let mut mapped_callback = None;
    let (args, component_slot) = if target.requirement == Requirement::OperationCallback {
        let callback = callback.context("callback use has no source stack proof")?;
        let policy = policy.context("callback compatibility policy is absent")?;
        ensure!(
            callback.descriptor.is_some(),
            "callback descriptor is unresolved"
        );
        ensure!(
            callback.trigger_count.is_none_or(|count| count <= 0),
            "callback trigger path cannot be represented"
        );
        let head = match callback.status {
            CallbackStatus::Installed { callee } => {
                let target = *links
                    .get(&callee)
                    .context("callback procedure is unlinked")?;
                mapped_callback = Some((callee, target));
                target
            }
            CallbackStatus::Cleared => policy.clear_script_id,
            _ => anyhow::bail!("callback has no portable installation state"),
        };
        // Every input still enters this bridge, including the old head and
        // descriptor. Only their meaning at the setter is replaced.
        let mut counts = [ONE_VALUE, u16::default(), u16::default()];
        let mut cursors = counts;
        let mut descriptor = String::new();
        code.push(instruction(
            book,
            "push_constant_string",
            Operand::Int(head),
        )?);
        for argument in &callback.arguments {
            let (lane, unit) = match argument.stack_lane {
                Lane::Int => (INTEGER_LANE, 'i'),
                Lane::Object => (OBJECT_LANE, 's'),
                Lane::Long => (LONG_LANE, 'l'),
            };
            code.push(local(book, lane, cursors[lane])?);
            increment(&mut cursors[lane])?;
            increment(&mut counts[lane])?;
            descriptor.push(unit);
        }
        // The source plain-hook codec pops a nonpositive trailing-Y count,
        // while retaining Y as an integer argument. The bridge consumes that
        // count and emits an ordinary, byte-equivalent typed signature.
        if callback.trigger_count.is_some() {
            increment(&mut counts[INTEGER_LANE])?;
        }
        let component_slot = counts[INTEGER_LANE];
        increment(&mut counts[INTEGER_LANE])?;
        increment(&mut counts[OBJECT_LANE])?;
        code.push(instruction(
            book,
            "push_constant_string",
            Operand::Str(descriptor),
        )?);
        (
            Counts {
                int: counts[INTEGER_LANE],
                obj: counts[OBJECT_LANE],
                long: counts[LONG_LANE],
            },
            component_slot,
        )
    } else {
        ensure!(
            callback.is_none(),
            "unexpected callback proof for a component use"
        );
        let args = match target.requirement {
            Requirement::ExplicitPlainText => Counts {
                int: ONE_VALUE,
                obj: ONE_VALUE,
                ..Counts::default()
            },
            Requirement::ExplicitOperationLabel => Counts {
                int: ONE_VALUE + ONE_VALUE,
                obj: ONE_VALUE,
                ..Counts::default()
            },
            Requirement::ExplicitRuntimeChildSelection | Requirement::FlatTextChildCreation => {
                Counts {
                    int: ONE_VALUE + ONE_VALUE + ONE_VALUE,
                    ..Counts::default()
                }
            }
            Requirement::ExplicitRuntimeChildren | Requirement::ExplicitComponentSelection => {
                Counts {
                    int: ONE_VALUE,
                    ..Counts::default()
                }
            }
            Requirement::FlatChildSelection
            | Requirement::ExplicitVisibility
            | Requirement::ExplicitComponentPaint
            | Requirement::ExplicitRuntimeChildSlot => Counts {
                int: ONE_VALUE + ONE_VALUE,
                ..Counts::default()
            },
            _ => anyhow::bail!("instruction has no bindable component use"),
        };
        let component_slot = if matches!(
            target.requirement,
            Requirement::ExplicitRuntimeChildSelection
                | Requirement::FlatChildSelection
                | Requirement::FlatTextChildCreation
        ) {
            HEAD_SLOT
        } else {
            args.int - ONE_VALUE
        };
        if !matches!(
            target.requirement,
            Requirement::ExplicitRuntimeChildSlot
                | Requirement::ExplicitRuntimeChildSelection
                | Requirement::FlatChildSelection
                | Requirement::FlatTextChildCreation
        ) {
            for slot in HEAD_SLOT..component_slot {
                code.push(local(book, INTEGER_LANE, slot)?);
            }
        }
        for slot in HEAD_SLOT..args.obj {
            code.push(local(book, OBJECT_LANE, slot)?);
        }
        (args, component_slot)
    };
    let declaration = if let Some(component) = component {
        let pc = code.len();
        code.push(instruction(
            book,
            "push_constant_string",
            Operand::Int(component.packed),
        )?);
        Some((pc, component.clone()))
    } else {
        code.push(local(book, INTEGER_LANE, component_slot)?);
        None
    };
    ensure!(
        native910::semantics::contract(book, &target.command, operand)?.retail_dispatch,
        "generated host use has no retail dispatch"
    );
    let setter_pc = code.len();
    // Selector interpretation is a source revision fact. The original wire
    // byte remains in the donor; only this generated native consumer is adapted.
    let operand = target
        .selector
        .map_or_else(|| Ok(operand.clone()), |policy| policy.convert(operand))?;
    code.push(Instruction {
        opcode: target.opcode,
        command: target.command.clone(),
        operand,
    });
    code.push(instruction(book, "return", Operand::Byte(u8::default()))?);
    Ok(Generated {
        script: CompiledScript {
            name: Some(format!("proc,{name}")),
            args,
            locals: args,
            code,
        },
        requirement: HostRequirement {
            pc: setter_pc,
            requirement: target.requirement,
        },
        declaration,
        callback: mapped_callback,
        host_operation: target.host_operation,
        resource: target
            .child_template
            .as_ref()
            .map(|template| Arc::from(template.resource())),
    })
}

fn text_bridge(
    target: &Target,
    operand: &Operand,
    name: &str,
    component: Option<&ComponentBinding>,
    font: Option<FontUse>,
    book: &OpcodeBook,
) -> Result<Generated> {
    use native910::execution::{HostOperation, TextProperty};
    let Some(HostOperation::ComponentText {
        mut property,
        text_type,
        explicit,
    }) = target.host_operation
    else {
        anyhow::bail!("text use has no consumer contract");
    };
    let mut resource = None;
    match (&mut property, font) {
        (TextProperty::Font { mapping, .. }, Some(FontUse::Write(bound))) => {
            *mapping = native910::execution::FontBinding::Constant(bound)
        }
        (TextProperty::Font { mapping, .. }, Some(FontUse::Mapped(bytes))) => {
            *mapping = native910::execution::FontBinding::Resource;
            resource = Some(bytes);
        }
        (TextProperty::Font { .. }, _) => {
            anyhow::bail!("font use needs a named target frame style")
        }
        (TextProperty::ReadFont { initial, .. }, Some(FontUse::Initial(bound))) => {
            *initial = Some(bound)
        }
        (TextProperty::ReadFont { .. }, None) => {}
        (_, None) => {}
        _ => anyhow::bail!("font binding does not match its consumer"),
    }
    ensure!(
        explicit || component.is_none(),
        "active text has no explicit component binding"
    );
    let args = Counts {
        int: property.integer_arguments() + u16::from(explicit),
        ..Counts::default()
    };
    let mut code = Vec::new();
    for slot in HEAD_SLOT..property.integer_arguments() {
        code.push(local(book, INTEGER_LANE, slot)?);
    }
    let declaration = if explicit {
        if let Some(component) = component {
            let pc = code.len();
            code.push(instruction(
                book,
                "push_constant_string",
                Operand::Int(component.packed),
            )?);
            Some((pc, component.clone()))
        } else {
            code.push(local(book, INTEGER_LANE, args.int - ONE_VALUE)?);
            None
        }
    } else {
        None
    };
    let consumer = code.len();
    let operand = if explicit {
        operand.clone()
    } else {
        target
            .selector
            .context("active text selector missing")?
            .convert(operand)?
    };
    code.push(instruction(book, &target.command, operand)?);
    code.push(instruction(book, "return", Operand::Byte(u8::default()))?);
    Ok(Generated {
        script: CompiledScript {
            name: Some(format!("proc,{name}")),
            args,
            locals: args,
            code,
        },
        requirement: HostRequirement {
            pc: consumer,
            requirement: target.requirement,
        },
        declaration,
        callback: None,
        host_operation: Some(HostOperation::ComponentText {
            property,
            text_type,
            explicit,
        }),
        resource,
    })
}

struct PlayerCallbackInput<'a> {
    target: &'a Target,
    operand: &'a Operand,
    callback: &'a Callback,
    triggers: &'a [i32],
    policy: &'a CallbackPolicy,
    links: &'a BTreeMap<u32, i32>,
    name: &'a str,
}
fn player_callback_bridge(input: PlayerCallbackInput<'_>, book: &OpcodeBook) -> Result<Generated> {
    let PlayerCallbackInput {
        target,
        operand,
        callback,
        triggers,
        policy,
        links,
        name,
    } = input;
    // A positive Y replaces the retained vector independently of prior state.
    // Unknown retained state cannot be silently treated as an empty vector.
    let descriptor = callback
        .descriptor
        .as_ref()
        .context("callback descriptor is unresolved")?;
    let count = callback
        .trigger_count
        .context("retained callback needs a proven replacement trigger list")?;
    ensure!(
        count > i32::default()
            && usize::try_from(count)? == triggers.len()
            && descriptor.ends_with('Y')
            && callback.triggers.len() == triggers.len(),
        "retained callback trigger replacement is unresolved"
    );
    let (head, mapped) = match callback.status {
        CallbackStatus::Installed { callee } => {
            let head = *links
                .get(&callee)
                .context("callback procedure is unlinked")?;
            (head, Some((callee, head)))
        }
        CallbackStatus::Cleared => (policy.clear_script_id, None),
        _ => anyhow::bail!("callback has no portable installation state"),
    };
    let mut counts = [ONE_VALUE, u16::default(), u16::default()];
    let mut cursors = counts;
    let mut code = vec![instruction(
        book,
        "push_constant_string",
        Operand::Int(head),
    )?];
    for argument in &callback.arguments {
        let lane = match argument.stack_lane {
            Lane::Int => INTEGER_LANE,
            Lane::Object => OBJECT_LANE,
            Lane::Long => LONG_LANE,
        };
        code.push(local(book, lane, cursors[lane])?);
        increment(&mut cursors[lane])?;
        increment(&mut counts[lane])?;
    }
    for trigger in triggers {
        code.push(instruction(
            book,
            "push_constant_string",
            Operand::Int(*trigger),
        )?);
        increment(&mut counts[INTEGER_LANE])?;
    }
    code.push(local(book, INTEGER_LANE, counts[INTEGER_LANE])?);
    increment(&mut counts[INTEGER_LANE])?;
    increment(&mut counts[OBJECT_LANE])?;
    code.push(instruction(
        book,
        "push_constant_string",
        Operand::Str(descriptor.clone()),
    )?);
    let consumer = code.len();
    let operand = target
        .selector
        .context("variable callback selector missing")?
        .convert(operand)?;
    code.push(instruction(book, &target.command, operand)?);
    code.push(instruction(book, "return", Operand::Byte(u8::default()))?);
    let args = Counts {
        int: counts[INTEGER_LANE],
        obj: counts[OBJECT_LANE],
        long: counts[LONG_LANE],
    };
    Ok(Generated {
        script: CompiledScript {
            name: Some(format!("proc,{name}")),
            args,
            locals: args,
            code,
        },
        requirement: HostRequirement {
            pc: consumer,
            requirement: Requirement::PlayerVariableCallback,
        },
        declaration: None,
        callback: mapped,
        host_operation: Some(
            native910::execution::HostOperation::RetainedPlayerTransmit {
                pops: counts,
                event_tokens: policy.variable_event_tokens()?,
            },
        ),
        resource: None,
    })
}

fn database_bridge(
    target: &Target,
    operand: &Operand,
    name: &str,
    database: DatabaseUse<'_>,
    book: &OpcodeBook,
) -> Result<Generated> {
    use native910::execution::HostOperation;
    use rs910_config::ui_db_schema::BaseType;
    const FIELD_INPUTS: u16 = 3;
    const COUNT_INPUTS: u16 = 2;
    let field = database
        .site
        .field
        .context("database packed field is not constant at this use")?;
    let (inputs, host_operation) = if target.requirement == Requirement::DatabaseField {
        let types = database.definitions.database().field_types(field)?;
        for id in types {
            let kind = database
                .definitions
                .database()
                .types
                .get(id)
                .context("database script type is unknown")?;
            ensure!(
                kind.base != BaseType::Coordinate,
                "coordinate database fields require tagged VM objects"
            );
        }
        (
            FIELD_INPUTS,
            HostOperation::DatabaseField {
                field,
                pushes: database
                    .site
                    .pushes
                    .context("database tuple traffic is unproven")?,
            },
        )
    } else {
        // Count ignores selectors; field presence is checked through the layout.
        let address = database.definitions.database().layout.unpack(field);
        ensure!(
            database
                .definitions
                .database()
                .tables
                .get(&address.table)
                .and_then(|table| table.columns.get(address.column))
                .is_some(),
            "database count has no field schema"
        );
        (COUNT_INPUTS, HostOperation::DatabaseFieldCount { field })
    };
    let args = Counts {
        int: inputs,
        ..Counts::default()
    };
    let mut code = (0..inputs)
        .map(|slot| local(book, INTEGER_LANE, slot))
        .collect::<Result<Vec<_>>>()?;
    let consumer = code.len();
    code.push(instruction(book, &target.command, operand.clone())?);
    code.push(instruction(book, "return", Operand::Byte(u8::default()))?);
    Ok(Generated {
        script: CompiledScript {
            name: Some(format!("proc,{name}")),
            args,
            locals: args,
            code,
        },
        requirement: HostRequirement {
            pc: consumer,
            requirement: target.requirement,
        },
        declaration: None,
        callback: None,
        host_operation: Some(host_operation),
        resource: Some(database.resource),
    })
}

#[cfg(test)]
pub(crate) fn verify(book: &crate::profile::Book) {
    use crate::{flow, import910, lower910::Adapter, semantic, wire};
    use native910::{config::ConfigTypes, dataflow};
    const CALLER: u32 = 1;
    const CALLEE: u32 = 2;
    const TARGET_CALLER: i32 = 11;
    const TARGET_CALLEE: i32 = 12;
    const TARGET_BRIDGE: i32 = 13;
    const SOURCE_COMPONENT: i32 = i32::MAX;
    const TARGET_COMPONENT: i32 = i32::MAX - 1;
    const CALLBACK_INTEGER: i32 = 42;
    const SECOND_INTEGER: i32 = 43;
    const CALLBACK_LONG: i64 = i64::MIN;
    const CALLBACK_TEXT: &str = "copied callback value";
    const HEAD_LOCAL: i32 = 0;
    let target = OpcodeBook::embedded().unwrap();
    let adapter = Adapter::parse(
        include_bytes!("../../../revisions/950/cs2/950-1-to-910.json"),
        book,
        &target,
    )
    .unwrap();
    let policy = adapter.callback_policy.as_ref().unwrap();
    let opcode = |name: &str| {
        book.profile
            .opcodes
            .iter()
            .find(|row| row.command.as_deref() == Some(name))
            .unwrap()
            .id
    };
    let source_instruction = |name: &str, operand| wire::Instruction {
        opcode: opcode(name),
        operand,
    };
    let integer = |value| source_instruction("push_constant", wire::Operand::ConstantInt(value));
    let component = ComponentBinding {
        font: None,
        symbol: "fixture/frame".into(),
        packed: TARGET_COMPONENT,
        plain_text: false,
        container: true,
    };
    let links = BTreeMap::from([(CALLER, TARGET_CALLER), (CALLEE, TARGET_CALLEE)]);
    for (descriptor, integers, text, long, trigger, expected_signature) in [
        (
            b"isl".as_slice(),
            vec![CALLBACK_INTEGER],
            Some(CALLBACK_TEXT),
            Some(CALLBACK_LONG),
            None,
            "isl",
        ),
        (
            [0xe9].as_slice(),
            vec![CALLBACK_INTEGER, SECOND_INTEGER],
            None,
            None,
            None,
            "ii",
        ),
        (
            b"iY".as_slice(),
            vec![CALLBACK_INTEGER, SECOND_INTEGER],
            None,
            None,
            Some(i32::MIN),
            "ii",
        ),
    ] {
        let mut code = vec![
            integer(i32::try_from(CALLEE).unwrap()),
            source_instruction("pop_int_local", wire::Operand::Int(HEAD_LOCAL)),
            source_instruction("push_int_local", wire::Operand::Int(HEAD_LOCAL)),
            source_instruction("push_int_local", wire::Operand::Int(HEAD_LOCAL)),
        ];
        code.extend(integers.iter().copied().map(integer));
        if let Some(text) = text {
            code.push(source_instruction(
                "push_constant",
                wire::Operand::ConstantString(text.as_bytes().to_vec()),
            ));
        }
        if let Some(long) = long {
            code.push(source_instruction(
                "push_constant",
                wire::Operand::ConstantLong(long),
            ));
        }
        if let Some(trigger) = trigger {
            code.push(integer(trigger));
        }
        code.push(source_instruction(
            "push_constant",
            wire::Operand::ConstantString(descriptor.to_vec()),
        ));
        code.push(integer(SOURCE_COMPONENT));
        let pc = code.len();
        code.push(source_instruction(
            "if_setonop",
            wire::Operand::Byte(u8::default()),
        ));
        code.push(source_instruction(
            "return",
            wire::Operand::Byte(u8::default()),
        ));
        let caller = wire::Script {
            name: vec![],
            locals: wire::Counts {
                int: ONE_VALUE,
                ..Default::default()
            },
            args: wire::Counts::default(),
            code,
            switches: vec![],
        };
        let counts = wire::Counts {
            int: u16::try_from(integers.len()).unwrap(),
            object: u16::from(text.is_some()),
            long: u16::from(long.is_some()),
        };
        let callee = wire::Script {
            name: vec![],
            locals: counts,
            args: counts,
            code: vec![source_instruction(
                "return",
                wire::Operand::Byte(u8::default()),
            )],
            switches: vec![],
        };
        let scripts = BTreeMap::from([
            (
                CALLER,
                semantic::normalize(&wire::encode(&caller, book).unwrap(), book).unwrap(),
            ),
            (
                CALLEE,
                semantic::normalize(&wire::encode(&callee, book).unwrap(), book).unwrap(),
            ),
        ]);
        let proof = flow::inspect(&scripts, book).unwrap();
        let callback = &proof.callbacks[&(CALLER, pc)];
        let target_use = adapter
            .target_for(&scripts[&CALLER].instructions[pc])
            .unwrap();
        let generated = generate(
            Request {
                font: None,
                database: None,
                enums: None,
                variable: None,
                target: target_use,
                operand: &Operand::Byte(u8::default()),
                callback: Some(callback),
                triggers: None,
                component: Some(&component),
                policy: Some(policy),
                links: &links,
                name: "linked_hook",
            },
            &target,
        )
        .unwrap();
        let imported = adapter
            .lower_with_uses(
                &scripts[&CALLER],
                &links,
                "installer",
                &BTreeMap::from([(pc, TARGET_BRIDGE)]),
            )
            .unwrap();
        assert_eq!(imported.script.code.len(), caller.code.len());
        assert_eq!(
            imported.script.code[0].operand,
            Operand::Int(i32::try_from(CALLEE).unwrap())
        );
        assert_eq!(
            imported.script.code[pc].operand,
            Operand::Script(TARGET_BRIDGE)
        );
        assert!(
            generated
                .script
                .code
                .iter()
                .any(|instruction| instruction.operand == Operand::Str(expected_signature.into()))
        );
        let original_bytes = wire::encode(&scripts[&CALLER].to_wire(), book).unwrap();
        assert_eq!(original_bytes, wire::encode(&caller, book).unwrap());
        let encoded = native910::script::encode_script(&generated.script, &target).unwrap();
        assert_eq!(
            native910::script::decode_script(&encoded, &target).unwrap(),
            generated.script
        );
        let target_callee = adapter
            .lower(&scripts[&CALLEE], &links, "callback_body")
            .unwrap()
            .script;
        let declaration = generated.declaration.clone().unwrap();
        let requirements = BTreeMap::from([(TARGET_BRIDGE, vec![generated.requirement])]);
        let lowered = BTreeMap::from([
            (TARGET_CALLER, imported.script),
            (TARGET_CALLEE, target_callee),
            (TARGET_BRIDGE, generated.script),
        ]);
        import910::verify_callback_fixture(
            &lowered,
            &requirements,
            &BTreeMap::from([(TARGET_BRIDGE, BTreeMap::from([declaration]))]),
            policy,
        )
        .unwrap();
        let configs = ConfigTypes::empty();
        let (summaries, _) = dataflow::infer_summaries(&lowered, &configs);
        let analysis = dataflow::analyze(&lowered[&TARGET_CALLER], &lowered, &summaries, &configs);
        assert!(analysis.failure.is_none());
        let returned = analysis.before.last().unwrap().as_ref().unwrap();
        assert_eq!(
            returned.int_from_top(0),
            Some(i32::try_from(CALLEE).unwrap())
        );
        // The old head's ordinary use survives; the installed head is remapped.
        assert_eq!(generated.callback, Some((CALLEE, TARGET_CALLEE)));
        let crate::profile::AnalysisContract::Hook {
            inactive_script_ids,
            ..
        } = book
            .opcode(opcode("if_setonop"))
            .unwrap()
            .analysis
            .as_ref()
            .unwrap()
        else {
            panic!("hook contract")
        };
        for head in [policy.clear_script_id, inactive_script_ids[0]] {
            let mut changed = caller.clone();
            changed.code[0].operand = wire::Operand::ConstantInt(head);
            let mut changed_scripts = scripts.clone();
            changed_scripts.insert(
                CALLER,
                semantic::normalize(&wire::encode(&changed, book).unwrap(), book).unwrap(),
            );
            let changed_proof = flow::inspect(&changed_scripts, book).unwrap();
            let result = generate(
                Request {
                    font: None,
                    database: None,
                    enums: None,
                    variable: None,
                    target: target_use,
                    operand: &Operand::Byte(u8::default()),
                    callback: Some(&changed_proof.callbacks[&(CALLER, pc)]),
                    triggers: None,
                    component: Some(&component),
                    policy: Some(policy),
                    links: &links,
                    name: "changed_hook",
                },
                &target,
            );
            if head == policy.clear_script_id {
                let cleared = result.unwrap();
                assert_eq!(cleared.callback, None);
                assert_eq!(
                    cleared.script.code[0].operand,
                    Operand::Int(policy.clear_script_id)
                );
            } else {
                assert!(result.is_err());
            }
        }
        if trigger.is_some() {
            let mut changed = caller.clone();
            changed
                .code
                .iter_mut()
                .find(|instruction| instruction.operand == wire::Operand::ConstantInt(i32::MIN))
                .unwrap()
                .operand = wire::Operand::ConstantInt(i32::from(true));
            let mut changed_scripts = scripts.clone();
            changed_scripts.insert(
                CALLER,
                semantic::normalize(&wire::encode(&changed, book).unwrap(), book).unwrap(),
            );
            let changed_proof = flow::inspect(&changed_scripts, book).unwrap();
            assert!(
                generate(
                    Request {
                        font: None,
                        database: None,
                        enums: None,
                        variable: None,
                        target: target_use,
                        operand: &Operand::Byte(u8::default()),
                        callback: Some(&changed_proof.callbacks[&(CALLER, pc)]),
                        triggers: None,
                        component: Some(&component),
                        policy: Some(policy),
                        links: &links,
                        name: "trapped_hook"
                    },
                    &target
                )
                .is_err()
            );
        }

        for unsafe_value in policy.int_event_tokens.iter().copied() {
            let mut unsafe_scripts = lowered.clone();
            unsafe_scripts
                .get_mut(&TARGET_CALLER)
                .unwrap()
                .code
                .iter_mut()
                .find(|instruction| instruction.operand == Operand::Int(CALLBACK_INTEGER))
                .unwrap()
                .operand = Operand::Int(unsafe_value);
            assert!(
                import910::verify_callback_fixture(
                    &unsafe_scripts,
                    &requirements,
                    &BTreeMap::from([(
                        TARGET_BRIDGE,
                        BTreeMap::from([generated.declaration.clone().unwrap()])
                    )]),
                    policy
                )
                .is_err()
            );
        }
        if text.is_some() {
            for unsafe_value in &policy.string_event_tokens {
                let mut unsafe_scripts = lowered.clone();
                unsafe_scripts
                    .get_mut(&TARGET_CALLER)
                    .unwrap()
                    .code
                    .iter_mut()
                    .find(|instruction| instruction.operand == Operand::Str(CALLBACK_TEXT.into()))
                    .unwrap()
                    .operand = Operand::Str(unsafe_value.clone());
                assert!(
                    import910::verify_callback_fixture(
                        &unsafe_scripts,
                        &requirements,
                        &BTreeMap::from([(
                            TARGET_BRIDGE,
                            BTreeMap::from([generated.declaration.clone().unwrap()])
                        )]),
                        policy
                    )
                    .is_err()
                );
            }
        }
        let mut mismatch = lowered.clone();
        mismatch.get_mut(&TARGET_CALLEE).unwrap().args.int += ONE_VALUE;
        assert!(
            import910::verify_callback_fixture(
                &mismatch,
                &requirements,
                &BTreeMap::from([(
                    TARGET_BRIDGE,
                    BTreeMap::from([generated.declaration.unwrap()])
                )]),
                policy
            )
            .is_err()
        );
    }
}
