//! Named operations with absolute control-flow targets and retained wire facts.
//! Recovery of an operation's identity never asserts equivalence to a target VM.

use crate::{
    profile::{Book, Build, StringEncoding, SwitchLookup, digest},
    wire,
};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::collections::BTreeMap;

const SEMANTIC_FORMAT: u32 = 1;
const INSTRUCTION_ADVANCE: i64 = 1;
const UNASSIGNED_CP1252: [u8; 5] = [0x81, 0x8d, 0x8f, 0x90, 0x9d];

#[derive(Clone, Debug, Serialize)]
pub struct Script {
    pub format: u32,
    pub build: Build,
    pub client_md5: String,
    pub profile_sha256: String,
    pub source_sha256: String,
    pub name: Vec<u8>,
    pub locals: wire::Counts,
    pub args: wire::Counts,
    pub instructions: Vec<Instruction>,
    pub wire_switches: Vec<Vec<wire::SwitchCase>>,
    pub switch_lookup: Option<SwitchLookup>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Instruction {
    pub wire: wire::Instruction,
    /// Absent when identity is unresolved. The original operand is retained.
    pub operation: Option<Operation>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Operation {
    pub command: String,
    pub argument: Argument,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Int,
    Object,
    Long,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Argument {
    Int(i32),
    Long(i64),
    Text {
        bytes: Vec<u8>,
        decoded: Option<String>,
    },
    Local {
        lane: Lane,
        slot: u16,
    },
    Branch(usize),
    Switch(Vec<Case>),
    Script(u32),
    /// Number of object-stack values consumed by a string join.
    Count(i32),
    Variable {
        domain: u8,
        id: u16,
        secondary: u8,
    },
    Varbit {
        id: u32,
        secondary: u8,
    },
    Byte(u8),
    /// A large operand whose operation-specific role has not been modeled.
    Large(i32),
    /// Internal decoder substitutions reference a runtime constant pool.
    ConstantPool(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Case {
    pub value: i32,
    pub target: usize,
}

impl Script {
    pub fn to_wire(&self) -> wire::Script {
        wire::Script {
            name: self.name.clone(),
            locals: self.locals,
            args: self.args,
            code: self
                .instructions
                .iter()
                .map(|instruction| instruction.wire.clone())
                .collect(),
            switches: self.wire_switches.clone(),
        }
    }
}

pub fn normalize(bytes: &[u8], book: &Book) -> Result<Script> {
    let source = wire::decode(bytes, book)?;
    ensure!(
        wire::encode(&source, book)? == bytes,
        "donor failed byte round trip"
    );
    let instructions = source
        .code
        .iter()
        .enumerate()
        .map(|(pc, instruction)| {
            let operation = book
                .opcode(instruction.opcode)?
                .command
                .as_deref()
                .map(|command| normalize_operation(command, instruction, &source, pc, book))
                .transpose()?;
            Ok(Instruction {
                wire: instruction.clone(),
                operation,
            })
        })
        .collect::<Result<_>>()?;
    Ok(Script {
        format: SEMANTIC_FORMAT,
        build: book.profile.build,
        client_md5: book.profile.client_md5.clone(),
        profile_sha256: book.sha256.clone(),
        source_sha256: digest(bytes),
        name: source.name,
        locals: source.locals,
        args: source.args,
        instructions,
        wire_switches: source.switches,
        switch_lookup: book.profile.switch_lookup,
    })
}

fn target(pc: usize, offset: i32, count: usize) -> Result<usize> {
    let target = i64::try_from(pc)? + i64::from(offset) + INSTRUCTION_ADVANCE;
    let target = usize::try_from(target)?;
    ensure!(
        target < count,
        "branch at {pc} targets missing instruction {target}"
    );
    Ok(target)
}

fn normalize_operation(
    command: &str,
    instruction: &wire::Instruction,
    source: &wire::Script,
    pc: usize,
    book: &Book,
) -> Result<Operation> {
    use wire::Operand;
    let mut command = command.to_owned();
    let argument = match (&*command, &instruction.operand) {
        ("push_constant", Operand::ConstantInt(value)) => {
            command = "push_constant_int".into();
            Argument::Int(*value)
        }
        ("push_constant", Operand::ConstantLong(value)) => {
            command = "push_long_constant".into();
            Argument::Long(*value)
        }
        ("push_constant", Operand::ConstantString(bytes)) => {
            command = "push_constant_string".into();
            Argument::Text {
                bytes: bytes.clone(),
                decoded: decode_text(bytes, book.profile.string_encoding)?,
            }
        }
        ("push_constant_int", Operand::Int(value)) => Argument::Int(*value),
        ("join_string", Operand::Int(count)) => Argument::Count(*count),
        ("push_long_constant", Operand::Int(index)) => Argument::ConstantPool(*index),
        ("gosub_with_params", Operand::Int(group)) => Argument::Script(u32::try_from(*group)?),
        ("switch", Operand::Int(index)) => {
            let cases = source
                .switches
                .get(usize::try_from(*index)?)
                .ok_or_else(|| anyhow::anyhow!("switch at {pc} has missing table {index}"))?;
            Argument::Switch(
                cases
                    .iter()
                    .map(|case| {
                        Ok(Case {
                            value: case.value,
                            target: target(pc, case.offset, source.code.len())?,
                        })
                    })
                    .collect::<Result<_>>()?,
            )
        }
        (name, Operand::Int(offset)) if native910::script::is_branch_command(name) => {
            Argument::Branch(target(pc, *offset, source.code.len())?)
        }
        (name, Operand::Int(slot)) if name.ends_with("_local") => {
            let lane = match name {
                "push_int_local" | "pop_int_local" => Lane::Int,
                "push_string_local" | "pop_string_local" => Lane::Object,
                "push_long_local" | "pop_long_local" => Lane::Long,
                _ => anyhow::bail!("unmodeled local operation {name}"),
            };
            let slot = u16::try_from(*slot)?;
            let count = match lane {
                Lane::Int => source.locals.int,
                Lane::Object => source.locals.object,
                Lane::Long => source.locals.long,
            };
            ensure!(
                slot < count,
                "local at {pc} uses missing {lane:?} slot {slot}"
            );
            Argument::Local { lane, slot }
        }
        (
            _,
            Operand::Variable {
                domain,
                id,
                secondary,
            },
        ) => Argument::Variable {
            domain: *domain,
            id: *id,
            secondary: *secondary,
        },
        (_, Operand::Varbit { id, secondary }) => Argument::Varbit {
            id: *id,
            secondary: *secondary,
        },
        (_, Operand::Byte(value)) => Argument::Byte(*value),
        (_, Operand::Int(value)) => Argument::Large(*value),
        _ => anyhow::bail!("unmodeled operand for {command} at {pc}"),
    };
    Ok(Operation { command, argument })
}

fn decode_text(bytes: &[u8], encoding: Option<StringEncoding>) -> Result<Option<String>> {
    let Some(encoding) = encoding else {
        return Ok(None);
    };
    let mut terminated: Vec<_> = bytes
        .iter()
        .copied()
        .filter(|byte| {
            encoding != StringEncoding::Windows1252DropUnassigned
                || !UNASSIGNED_CP1252.contains(byte)
        })
        .collect();
    terminated.push(u8::default());
    Ok(Some(native910::packet::Packet::new(&terminated).gjstr()?))
}

// Resolve duplicate keys before emitting the target's first-match table.
// Wire tables stay untouched in the donor representation for exact recovery.
pub fn selected_cases(cases: &[Case], lookup: Option<SwitchLookup>) -> Result<Vec<Case>> {
    let lookup = lookup.ok_or_else(|| anyhow::anyhow!("unrecovered switch lookup policy"))?;
    let mut selected = BTreeMap::new();
    for case in cases {
        match lookup {
            SwitchLookup::FirstEntryWins => {
                selected.entry(case.value).or_insert(*case);
            }
            SwitchLookup::LastEntryWins => {
                selected.insert(case.value, *case);
            }
        }
    }
    Ok(selected.into_values().collect())
}
