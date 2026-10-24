//! 910 CS2 script codec: binary ⇄ [`CompiledScript`].
//!
//! 910-only, so there are no build-version branches anywhere in this module: the
//! script name is always present, the long counts are always present, the switch
//! trailer is always present, and constants/var refs always use their modern
//! (post-800) typed forms. Anything else in the bytes is invalid data.
//!
//! Layout (all integers big-endian): `name\0`, then the instruction stream —
//! `opcode u16` followed by the command's operand — then the header:
//! `code_len i32`, three local counts `u16`, three argument counts `u16`, the
//! switch tables (`count u8`, then per table `cases u16` + `value i32, offset
//! i32` pairs), and a final `trailer_size u16` covering the switch-table bytes.
//!
//! Branch operands are stored relative to the instruction *after* the branch:
//! the client runs `target = branch_index + stored + 1`, so decode adds one and
//! encode subtracts one. Switch case offsets use the same convention.

use crate::error::{NativeError, Result};
use crate::opcode::OpcodeBook;
use crate::packet::{ByteWriter, Packet};
use crate::vars::VarScope;

/// A decoded 910 clientscript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledScript {
    /// Optional `[clientscript,name]` / `[proc,name]` tag.
    pub name: Option<String>,
    /// Total int/object/long local slots, including argument slots.
    pub locals: Counts,
    /// Declared int/object/long argument counts.
    pub args: Counts,
    /// Instruction stream.
    pub code: Vec<Instruction>,
}

/// An `(int, object, long)` triple: local or argument counts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counts {
    /// Int count.
    pub int: u16,
    /// Object (string) count.
    pub obj: u16,
    /// Long count.
    pub long: u16,
}

/// One decoded instruction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Instruction {
    /// Scrambled 910 opcode id (filled by decode, re-derived by encode).
    pub opcode: u16,
    /// Canonical command name.
    pub command: String,
    /// Decoded operand.
    pub operand: Operand,
}

/// A decoded instruction operand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operand {
    /// 32-bit int constant / generic large operand.
    Int(i32),
    /// 64-bit long constant.
    Long(i64),
    /// String constant.
    Str(String),
    /// Local variable slot.
    Local(i32),
    /// `domain:id` variable reference.
    VarRef(VarRef),
    /// Varbit reference.
    VarBitRef(VarBitRef),
    /// Absolute branch target (instruction index).
    Branch(i32),
    /// Switch cases with absolute targets.
    Switch(Vec<SwitchCase>),
    /// Callee script id.
    Script(i32),
    /// Array id.
    Array(i32),
    /// `join_string` part count.
    Count(i32),
    /// Single-byte generic operand. Client semantics per command family: the
    /// `cc_`/`if_` commands read it as the secondary-context flag (`== 1`),
    /// `push_var`/`pop_var`/`push_varbit`/`pop_varbit` read it as the
    /// transmog (secondary-domain) flag, and every other 1-byte command
    /// ignores it. Nonzero values are always preserved verbatim regardless.
    Byte(u8),
}

/// A variable reference with its domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VarRef {
    /// Variable domain.
    pub domain: VarScope,
    /// Variable id.
    pub id: u16,
    /// Secondary ("transmog") flag.
    pub transmog: bool,
}

/// A varbit reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VarBitRef {
    /// Varbit id.
    pub id: u16,
    /// Secondary ("transmog") flag.
    pub transmog: bool,
}

/// One switch case: match `value`, jump to absolute instruction `target`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwitchCase {
    /// Match value.
    pub value: i32,
    /// Absolute target instruction index.
    pub target: i32,
}

/// Decode one 910 script's raw bytes.
pub fn decode_script(data: &[u8], book: &OpcodeBook) -> Result<CompiledScript> {
    let mut packet = Packet::new(data);
    // Fixed header: code_len (4) + six counts (12). The switch tables ride
    // between the counts and the 2-byte trailer size.
    packet.set_pos(data.len().saturating_sub(2))?;
    let trailer_size = usize::from(packet.g2()?);
    let header_size = 16_usize
        .checked_add(2)
        .and_then(|size| size.checked_add(trailer_size))
        .ok_or_else(|| NativeError::Invalid("script header size overflow".to_string()))?;
    let header_pos = data
        .len()
        .checked_sub(header_size)
        .ok_or_else(|| NativeError::Invalid("script shorter than its header".to_string()))?;

    packet.set_pos(header_pos)?;
    let code_len = usize::try_from(packet.g4s()?)
        .map_err(|_| NativeError::Invalid("negative script code length".to_string()))?;
    let locals = Counts {
        int: packet.g2()?,
        obj: packet.g2()?,
        long: packet.g2()?,
    };
    let args = Counts {
        int: packet.g2()?,
        obj: packet.g2()?,
        long: packet.g2()?,
    };

    let switch_count = usize::from(packet.g1()?);
    let mut switch_values: Vec<Vec<i32>> = Vec::with_capacity(switch_count);
    let mut switch_offsets: Vec<Vec<i32>> = Vec::with_capacity(switch_count);
    for _ in 0..switch_count {
        let case_count = usize::from(packet.g2()?);
        let mut values = Vec::with_capacity(case_count);
        let mut offsets = Vec::with_capacity(case_count);
        for _ in 0..case_count {
            values.push(packet.g4s()?);
            offsets.push(packet.g4s()?);
        }
        switch_values.push(values);
        switch_offsets.push(offsets);
    }

    packet.set_pos(0)?;
    let name = packet.gjstrnull()?;

    // Capacity is a hint, not a promise: a corrupt header can declare billions
    // of instructions, and pre-sizing to that aborts instead of erroring below.
    let mut code = Vec::with_capacity(code_len.min(1024));
    while packet.pos() < header_pos {
        let opcode = packet.g2()?;
        let command = book.name(opcode)?.to_string();
        let index = i32::try_from(code.len())
            .map_err(|_| NativeError::Invalid("script too large".to_string()))?;
        let operand = decode_operand(
            &command,
            book.has_large_operand(opcode),
            &mut packet,
            index,
            code_len,
            &switch_values,
            &switch_offsets,
        )?;
        code.push(Instruction {
            opcode,
            command,
            operand,
        });
    }

    if code.len() != code_len {
        return Err(NativeError::Invalid(format!(
            "script code length mismatch: decoded {} instructions, header declares {code_len}",
            code.len()
        )));
    }

    Ok(CompiledScript {
        name,
        locals,
        args,
        code,
    })
}

/// Commands whose operand is an absolute branch target. Shared with the source
/// language, where these commands take a label instead of an index.
pub fn is_branch_command(command: &str) -> bool {
    matches!(
        command,
        "branch"
            | "branch_not"
            | "branch_equals"
            | "branch_less_than"
            | "branch_greater_than"
            | "branch_less_than_or_equals"
            | "branch_greater_than_or_equals"
            | "long_branch_not"
            | "long_branch_equals"
            | "long_branch_less_than"
            | "long_branch_greater_than"
            | "long_branch_less_than_or_equals"
            | "long_branch_greater_than_or_equals"
            | "branch_if_true"
            | "branch_if_false"
    )
}

/// A decoded target must land on an instruction — or exactly past the last
/// one (falling off the end exits the script). Anything else would crash the
/// client, so it is invalid data, not a model we carry.
fn check_target(target: i32, code_len: usize, context: &str) -> Result<()> {
    let in_range = target >= 0 && usize::try_from(target).is_ok_and(|index| index <= code_len);
    if in_range {
        Ok(())
    } else {
        Err(NativeError::Invalid(format!(
            "{context} target {target} out of range for {code_len} instructions"
        )))
    }
}

/// Decode the trailing 1-byte var/varbit secondary flag. The client treats it
/// as a boolean; any other value is invalid data, rejected rather than silently
/// collapsed (collapsing would re-encode as `0` — a silent byte change).
fn decode_transmog_flag(packet: &mut Packet<'_>) -> Result<bool> {
    match packet.g1()? {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(NativeError::Invalid(format!(
            "unexpected var secondary flag {other} (expected 0 or 1)"
        ))),
    }
}

fn decode_operand(
    command: &str,
    is_large_operand: bool,
    packet: &mut Packet<'_>,
    index: i32,
    code_len: usize,
    switch_values: &[Vec<i32>],
    switch_offsets: &[Vec<i32>],
) -> Result<Operand> {
    match command {
        "push_constant_int" => Ok(Operand::Int(packet.g4s()?)),
        "push_constant_string" => match packet.g1()? {
            0 => Ok(Operand::Int(packet.g4s()?)),
            1 => Ok(Operand::Long(packet.g8s()?)),
            2 => Ok(Operand::Str(packet.gjstr()?)),
            tag => Err(NativeError::Invalid(format!(
                "unsupported typed constant tag: {tag}"
            ))),
        },
        "push_int_local" | "pop_int_local" | "push_string_local" | "pop_string_local"
        | "push_long_local" | "pop_long_local" => Ok(Operand::Local(packet.g4s()?)),
        "push_var" | "pop_var" => {
            let domain = VarScope::from_id(packet.g1()?)?;
            let id = packet.g2()?;
            let transmog = decode_transmog_flag(packet)?;
            Ok(Operand::VarRef(VarRef {
                domain,
                id,
                transmog,
            }))
        }
        "push_varbit" | "pop_varbit" => {
            let id = packet.g2()?;
            let transmog = decode_transmog_flag(packet)?;
            Ok(Operand::VarBitRef(VarBitRef { id, transmog }))
        }
        command if is_branch_command(command) => {
            let relative = packet.g4s()?;
            let target = index
                .checked_add(relative)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| {
                    NativeError::Invalid(format!("branch target overflow at index {index}"))
                })?;
            check_target(target, code_len, "branch")?;
            Ok(Operand::Branch(target))
        }
        "switch" => {
            let table = usize::try_from(packet.g4s()?)
                .map_err(|_| NativeError::Invalid("negative switch table index".to_string()))?;
            let values = switch_values.get(table).ok_or_else(|| {
                NativeError::Invalid(format!("switch table out of range: {table}"))
            })?;
            let offsets = switch_offsets.get(table).ok_or_else(|| {
                NativeError::Invalid(format!("switch offset table out of range: {table}"))
            })?;
            if values.len() != offsets.len() {
                return Err(NativeError::Invalid(
                    "switch value/offset length mismatch".to_string(),
                ));
            }
            let mut cases = Vec::with_capacity(values.len());
            for (value, offset) in values.iter().zip(offsets) {
                let target = index
                    .checked_add(*offset)
                    .and_then(|at| at.checked_add(1))
                    .ok_or_else(|| {
                        NativeError::Invalid(format!("switch target overflow at index {index}"))
                    })?;
                check_target(target, code_len, "switch case")?;
                cases.push(SwitchCase {
                    value: *value,
                    target,
                });
            }
            Ok(Operand::Switch(cases))
        }
        "join_string" => Ok(Operand::Count(packet.g4s()?)),
        "gosub_with_params" => Ok(Operand::Script(packet.g4s()?)),
        "define_array"
        | "push_array_int"
        | "pop_array_int"
        | "push_array_int_leave_index_on_stack"
        | "push_array_int_and_index"
        | "pop_array_int_leave_value_on_stack" => Ok(Operand::Array(packet.g4s()?)),
        // Raw `push_long_constant` (761) lands here too: the client reads its
        // operand like any large-operand command (`g4s`, 4 bytes); an 8-byte
        // long only exists behind `push_constant_string`'s long tag, which
        // the client rewrites to `PUSH_LONG_CONSTANT` at decode.
        _ if is_large_operand => Ok(Operand::Int(packet.g4s()?)),
        _ => Ok(Operand::Byte(packet.g1()?)),
    }
}

/// Encode a [`CompiledScript`] back to 910 binary. Branch and switch targets
/// are absolute instruction indices in the model; the stored relative offsets
/// are recomputed here, so inserting or removing instructions never requires
/// hand-fixing jump targets.
pub fn encode_script(script: &CompiledScript, book: &OpcodeBook) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::with_capacity(script.code.len() * 8 + 128);
    writer.pjstrnull(script.name.as_deref())?;

    struct BranchPatch {
        pos: usize,
        target: i32,
        instr_index: i32,
    }
    struct SwitchPatch {
        pos: usize,
        values: Vec<i32>,
        targets: Vec<i32>,
        instr_index: i32,
    }
    let mut branches: Vec<BranchPatch> = Vec::new();
    let mut switches: Vec<SwitchPatch> = Vec::new();

    for (position, instr) in script.code.iter().enumerate() {
        let instr_index = i32::try_from(position)
            .map_err(|_| NativeError::Invalid("instruction index overflow".to_string()))?;
        let opcode = book.opcode_for(&instr.command)?;
        writer.p2(opcode);
        match &instr.operand {
            Operand::Branch(target) => {
                // A branch operand on a non-branch command would write 4 bytes
                // where decode reads 1, desyncing the stream. Reject the model
                // instead of emitting bytes no decoder agrees with.
                if !is_branch_command(&instr.command) {
                    return Err(NativeError::Invalid(format!(
                        "command `{}` cannot carry a branch target",
                        instr.command
                    )));
                }
                check_target(*target, script.code.len(), "branch")?;
                branches.push(BranchPatch {
                    pos: writer.len(),
                    target: *target,
                    instr_index,
                });
                writer.p4s(0);
            }
            Operand::Switch(cases) => {
                if instr.command != "switch" {
                    return Err(NativeError::Invalid(format!(
                        "command `{}` cannot carry switch cases",
                        instr.command
                    )));
                }
                for case in cases {
                    check_target(case.target, script.code.len(), "switch case")?;
                }
                switches.push(SwitchPatch {
                    pos: writer.len(),
                    values: cases.iter().map(|case| case.value).collect(),
                    targets: cases.iter().map(|case| case.target).collect(),
                    instr_index,
                });
                writer.p4s(0);
            }
            operand => encode_operand(
                operand,
                &instr.command,
                book.has_large_operand(opcode),
                &mut writer,
            )?,
        }
    }

    for patch in &branches {
        let relative = patch
            .target
            .checked_sub(patch.instr_index)
            .and_then(|value| value.checked_sub(1))
            .ok_or_else(|| {
                NativeError::Invalid(format!(
                    "branch target underflow at index {}",
                    patch.instr_index
                ))
            })?;
        writer.patch_i32_at(patch.pos, relative)?;
    }
    for (table, patch) in switches.iter().enumerate() {
        let index = i32::try_from(table)
            .map_err(|_| NativeError::Invalid("switch table index overflow".to_string()))?;
        writer.patch_i32_at(patch.pos, index)?;
    }

    let code_len = i32::try_from(script.code.len())
        .map_err(|_| NativeError::Invalid("script too large to encode".to_string()))?;
    writer.p4s(code_len);
    writer.p2(script.locals.int);
    writer.p2(script.locals.obj);
    writer.p2(script.locals.long);
    writer.p2(script.args.int);
    writer.p2(script.args.obj);
    writer.p2(script.args.long);

    let table_start = writer.len();
    let switch_count = u8::try_from(switches.len())
        .map_err(|_| NativeError::Invalid("too many switch tables".to_string()))?;
    writer.p1(switch_count);
    for patch in &switches {
        let case_count = u16::try_from(patch.values.len())
            .map_err(|_| NativeError::Invalid("switch case count overflow".to_string()))?;
        writer.p2(case_count);
        for (value, target) in patch.values.iter().zip(patch.targets.iter()) {
            let relative = target
                .checked_sub(patch.instr_index)
                .and_then(|at| at.checked_sub(1))
                .ok_or_else(|| {
                    NativeError::Invalid(format!(
                        "switch target underflow at index {}",
                        patch.instr_index
                    ))
                })?;
            writer.p4s(*value);
            writer.p4s(relative);
        }
    }
    let trailer_size = u16::try_from(writer.len() - table_start)
        .map_err(|_| NativeError::Invalid("switch trailer too large".to_string()))?;
    writer.p2(trailer_size);

    Ok(writer.data)
}

/// Narrow an `i32` into a single-byte generic-operand slot without silent
/// truncation: the byte slot round-trips through `g1`/`p1` (`u8`), so anything
/// outside `-128..=255` would corrupt on repack and is rejected instead.
fn encode_byte_operand(value: i32) -> Result<u8> {
    if (-128..=255).contains(&value) {
        Ok(value as u8)
    } else {
        Err(NativeError::Invalid(format!(
            "operand {value} does not fit in a 1-byte slot (expected -128..=255)"
        )))
    }
}

fn encode_operand(
    operand: &Operand,
    command: &str,
    is_large_operand: bool,
    writer: &mut ByteWriter,
) -> Result<()> {
    match command {
        "push_constant_int" => match operand {
            Operand::Int(value) => writer.p4s(*value),
            _ => return invalid_operand(command, "Int"),
        },
        "push_constant_string" => match operand {
            Operand::Int(value) => {
                writer.p1(0);
                writer.p4s(*value);
            }
            Operand::Long(value) => {
                writer.p1(1);
                writer.p8s(*value);
            }
            Operand::Str(text) => {
                writer.p1(2);
                writer.pjstr(text)?;
            }
            _ => return invalid_operand(command, "Int/Long/Str"),
        },
        "push_int_local" | "pop_int_local" | "push_string_local" | "pop_string_local"
        | "push_long_local" | "pop_long_local" => match operand {
            Operand::Local(value) | Operand::Int(value) => writer.p4s(*value),
            _ => return invalid_operand(command, "Local/Int"),
        },
        "push_var" | "pop_var" => match operand {
            Operand::VarRef(var) => {
                writer.p1(u8::from(var.domain));
                writer.p2(var.id);
                writer.p1(u8::from(var.transmog));
            }
            _ => return invalid_operand(command, "VarRef"),
        },
        "push_varbit" | "pop_varbit" => match operand {
            Operand::VarBitRef(varbit) => {
                writer.p2(varbit.id);
                writer.p1(u8::from(varbit.transmog));
            }
            _ => return invalid_operand(command, "VarBitRef"),
        },
        "join_string" => match operand {
            Operand::Count(value) | Operand::Int(value) => writer.p4s(*value),
            _ => return invalid_operand(command, "Count/Int"),
        },
        "gosub_with_params" => match operand {
            Operand::Script(id) | Operand::Int(id) => writer.p4s(*id),
            _ => return invalid_operand(command, "Script/Int"),
        },
        "define_array"
        | "push_array_int"
        | "pop_array_int"
        | "push_array_int_leave_index_on_stack"
        | "push_array_int_and_index"
        | "pop_array_int_leave_value_on_stack" => match operand {
            Operand::Array(id) | Operand::Int(id) => writer.p4s(*id),
            _ => return invalid_operand(command, "Array/Int"),
        },
        _ => match operand {
            Operand::Byte(value) if is_large_operand => writer.p4s(i32::from(*value)),
            Operand::Byte(value) => writer.p1(*value),
            Operand::Int(value) if is_large_operand => writer.p4s(*value),
            Operand::Int(value) => writer.p1(encode_byte_operand(*value)?),
            // Refuse a silent zero placeholder for an operand variant the
            // default encoding cannot represent — that would desync every later
            // instruction. A correctly decoded default-group command only ever
            // carries Byte/Int here.
            _ => {
                return Err(NativeError::Invalid(format!(
                    "command `{command}` has an unencodable operand for default encoding"
                )));
            }
        },
    }
    Ok(())
}

fn invalid_operand<T>(command: &str, expected: &str) -> Result<T> {
    Err(NativeError::Invalid(format!(
        "{command} expects a {expected} operand"
    )))
}
