use crate::profile::{Book, Encoding};
use anyhow::{Result, bail, ensure};
use native910::packet::{ByteWriter, Packet};
use serde::{Deserialize, Serialize};

const TRAILER_SIZE_BYTES: usize = size_of::<u16>();
const COUNTS_PER_HEADER: usize = 6;
const FIXED_HEADER_BYTES: usize = size_of::<i32>() + COUNTS_PER_HEADER * size_of::<u16>();
const MAX_CAPACITY_HINT: usize = 1024;
const INT_CONSTANT_TAG: u8 = 0;
const LONG_CONSTANT_TAG: u8 = 1;
const STRING_CONSTANT_TAG: u8 = 2;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Counts {
    pub int: u16,
    pub object: u16,
    pub long: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Operand {
    Byte(u8),
    Int(i32),
    ConstantInt(i32),
    ConstantLong(i64),
    /// Raw Windows-1252 bytes. Retains every original byte, including undefined
    /// charset slots; display decoding is separate from the wire representation.
    ConstantString(Vec<u8>),
    Variable {
        domain: u8,
        id: u16,
        secondary: u8,
    },
    Varbit {
        id: u32,
        secondary: u8,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Instruction {
    pub opcode: u16,
    pub operand: Operand,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SwitchCase {
    pub value: i32,
    /// Offset relative to the instruction following the switch.
    pub offset: i32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Script {
    pub name: Vec<u8>,
    pub locals: Counts,
    pub args: Counts,
    pub code: Vec<Instruction>,
    pub switches: Vec<Vec<SwitchCase>>,
}

fn read_string(packet: &mut Packet<'_>) -> Result<Vec<u8>> {
    let mut value = Vec::new();
    loop {
        let byte = packet.g1()?;
        if byte == 0 {
            return Ok(value);
        }
        value.push(byte);
    }
}

fn read_counts(packet: &mut Packet<'_>) -> Result<Counts> {
    Ok(Counts {
        int: packet.g2()?,
        object: packet.g2()?,
        long: packet.g2()?,
    })
}

pub fn decode(data: &[u8], book: &Book) -> Result<Script> {
    let trailer_pos = data
        .len()
        .checked_sub(TRAILER_SIZE_BYTES)
        .ok_or_else(|| anyhow::anyhow!("missing script trailer size"))?;
    let mut trailer = Packet::with_pos(data, trailer_pos)?;
    let switch_bytes = usize::from(trailer.g2()?);
    let header_pos = trailer_pos
        .checked_sub(switch_bytes)
        .and_then(|pos| pos.checked_sub(FIXED_HEADER_BYTES))
        .ok_or_else(|| anyhow::anyhow!("script shorter than declared header"))?;
    let mut header = Packet::new(&data[header_pos..trailer_pos]);
    let declared = usize::try_from(header.g4s()?)?;
    let locals = read_counts(&mut header)?;
    let args = read_counts(&mut header)?;
    ensure!(
        args.int <= locals.int && args.object <= locals.object && args.long <= locals.long,
        "arguments exceed local slots"
    );
    let mut switches = Vec::new();
    for _ in 0..header.g1()? {
        let mut cases = Vec::new();
        for _ in 0..header.g2()? {
            cases.push(SwitchCase {
                value: header.g4s()?,
                offset: header.g4s()?,
            });
        }
        switches.push(cases);
    }
    ensure!(header.is_empty(), "unconsumed script trailer bytes");
    // An instruction operand may never borrow bytes from the header.
    let mut code_packet = Packet::new(&data[..header_pos]);
    let name = read_string(&mut code_packet)?;
    let mut code = Vec::with_capacity(declared.min(MAX_CAPACITY_HINT));
    while !code_packet.is_empty() {
        let opcode = code_packet.g2()?;
        let encoding = book.opcode(opcode)?.encoding;
        let operand = match encoding {
            Encoding::Byte => Operand::Byte(code_packet.g1()?),
            Encoding::Int => Operand::Int(code_packet.g4s()?),
            Encoding::TypedConstant => match code_packet.g1()? {
                INT_CONSTANT_TAG => Operand::ConstantInt(code_packet.g4s()?),
                LONG_CONSTANT_TAG => Operand::ConstantLong(code_packet.g8s()?),
                STRING_CONSTANT_TAG => Operand::ConstantString(read_string(&mut code_packet)?),
                tag => bail!(
                    "unrecognized constant type {tag} at instruction {}",
                    code.len()
                ),
            },
            Encoding::Variable => Operand::Variable {
                domain: code_packet.g1()?,
                id: code_packet.g2()?,
                secondary: code_packet.g1()?,
            },
            Encoding::Varbit16 => Operand::Varbit {
                id: u32::from(code_packet.g2()?),
                secondary: code_packet.g1()?,
            },
            Encoding::Varbit24 => Operand::Varbit {
                id: code_packet.g3()?,
                secondary: code_packet.g1()?,
            },
        };
        code.push(Instruction { opcode, operand });
    }
    ensure!(
        code.len() == declared,
        "decoded {} instructions, header declares {declared}",
        code.len()
    );
    Ok(Script {
        name,
        locals,
        args,
        code,
        switches,
    })
}

fn write_counts(output: &mut ByteWriter, counts: Counts) {
    output.p2(counts.int);
    output.p2(counts.object);
    output.p2(counts.long);
}

fn write_string(output: &mut ByteWriter, bytes: &[u8]) -> Result<()> {
    ensure!(!bytes.contains(&0), "embedded null in script string");
    output.pdata(bytes);
    output.p1(0);
    Ok(())
}

pub fn encode(script: &Script, book: &Book) -> Result<Vec<u8>> {
    ensure!(
        script.args.int <= script.locals.int
            && script.args.object <= script.locals.object
            && script.args.long <= script.locals.long,
        "arguments exceed local slots"
    );
    let mut output = ByteWriter::default();
    write_string(&mut output, &script.name)?;
    for instruction in &script.code {
        let encoding = book.opcode(instruction.opcode)?.encoding;
        output.p2(instruction.opcode);
        match (encoding, &instruction.operand) {
            (Encoding::Byte, Operand::Byte(value)) => output.p1(*value),
            (Encoding::Int, Operand::Int(value)) => output.p4s(*value),
            (Encoding::TypedConstant, Operand::ConstantInt(value)) => {
                output.p1(INT_CONSTANT_TAG);
                output.p4s(*value);
            }
            (Encoding::TypedConstant, Operand::ConstantLong(value)) => {
                output.p1(LONG_CONSTANT_TAG);
                output.p8s(*value);
            }
            (Encoding::TypedConstant, Operand::ConstantString(value)) => {
                output.p1(STRING_CONSTANT_TAG);
                write_string(&mut output, value)?;
            }
            (
                Encoding::Variable,
                Operand::Variable {
                    domain,
                    id,
                    secondary,
                },
            ) => {
                output.p1(*domain);
                output.p2(*id);
                output.p1(*secondary);
            }
            (Encoding::Varbit16, Operand::Varbit { id, secondary }) => {
                output.p2(u16::try_from(*id)?);
                output.p1(*secondary);
            }
            (Encoding::Varbit24, Operand::Varbit { id, secondary }) => {
                output.p3(*id)?;
                output.p1(*secondary);
            }
            _ => bail!(
                "operand does not match {:?} for opcode {}",
                encoding,
                instruction.opcode
            ),
        }
    }
    output.p4s(i32::try_from(script.code.len())?);
    write_counts(&mut output, script.locals);
    write_counts(&mut output, script.args);
    let switch_start = output.len();
    output.p1(u8::try_from(script.switches.len())?);
    for cases in &script.switches {
        output.p2(u16::try_from(cases.len())?);
        for case in cases {
            output.p4s(case.value);
            output.p4s(case.offset);
        }
    }
    output.p2(u16::try_from(output.len() - switch_start)?);
    Ok(output.data)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{PROFILE_FORMAT, Profile};

    fn book() -> Book {
        Book::parse(include_bytes!("../../../revisions/950/cs2/950-1.json")).unwrap()
    }

    #[test]
    fn revision_formats_preserve_wide_ids_raw_strings_and_reject_cross_header_reads() {
        const WIDE_VARBIT: u32 = 0x12_3456;
        const RAW_SECONDARY: u8 = 0x80;
        let book = book();
        verify_reviewed_lowering(&book);
        verify_dependency_accounting(&book);
        crate::flow::verify_callbacks(&book);
        crate::flow::verify_retained_callbacks(&book);
        crate::bridge::verify(&book);
        crate::archive::verify_fetching(&book);
        crate::database::verify(&book);
        crate::enums::verify(&book);
        crate::variables::verify(&book);
        crate::frames::verify(&book);
        crate::variable_bindings::verify_trigger_bases(&book);
        let opcode = |encoding| {
            book.profile
                .opcodes
                .iter()
                .find(|row| row.encoding == encoding)
                .unwrap()
                .id
        };
        let script = Script {
            name: vec![b'n', 0x81],
            locals: Counts::default(),
            args: Counts::default(),
            code: vec![
                Instruction {
                    opcode: opcode(Encoding::Varbit24),
                    operand: Operand::Varbit {
                        id: WIDE_VARBIT,
                        secondary: RAW_SECONDARY,
                    },
                },
                Instruction {
                    opcode: opcode(Encoding::TypedConstant),
                    operand: Operand::ConstantString(vec![0x81, 0xff]),
                },
                Instruction {
                    opcode: opcode(Encoding::TypedConstant),
                    operand: Operand::ConstantLong(i64::MIN),
                },
            ],
            switches: vec![vec![SwitchCase {
                value: i32::MIN,
                offset: -1,
            }]],
        };
        let bytes = encode(&script, &book).unwrap();
        assert_eq!(decode(&bytes, &book).unwrap(), script);
        assert_eq!(
            encode(&decode(&bytes, &book).unwrap(), &book).unwrap(),
            bytes
        );
        let normalized = crate::semantic::normalize(&bytes, &book).unwrap();
        assert_eq!(encode(&normalized.to_wire(), &book).unwrap(), bytes);
        assert_eq!(
            normalized.instructions[1]
                .operation
                .as_ref()
                .unwrap()
                .argument,
            crate::semantic::Argument::Text {
                bytes: vec![0x81, 0xff],
                decoded: Some("ÿ".into())
            }
        );
        assert_eq!(
            normalized.instructions[2]
                .operation
                .as_ref()
                .unwrap()
                .argument,
            crate::semantic::Argument::Long(i64::MIN)
        );

        // Operation identity comes solely from the selected book, even when
        // the same operations are assigned entirely different wire numbers.
        const SCRAMBLE_SHIFT: u16 = 5000;
        let mut scrambled = book.profile.clone();
        for row in &mut scrambled.opcodes {
            row.id += SCRAMBLE_SHIFT;
        }
        let scrambled = Book::parse(&serde_json::to_vec(&scrambled).unwrap()).unwrap();
        let mut shifted = script.clone();
        for row in &mut shifted.code {
            row.opcode += SCRAMBLE_SHIFT;
        }
        let shifted_bytes = encode(&shifted, &scrambled).unwrap();
        assert!(decode(&shifted_bytes, &book).is_err());
        let shifted = crate::semantic::normalize(&shifted_bytes, &scrambled).unwrap();
        for (left, right) in normalized.instructions.iter().zip(&shifted.instructions) {
            assert_eq!(
                left.operation.as_ref().map(|operation| &operation.command),
                right.operation.as_ref().map(|operation| &operation.command)
            );
            assert_eq!(
                left.operation.as_ref().map(|operation| &operation.argument),
                right
                    .operation
                    .as_ref()
                    .map(|operation| &operation.argument)
            );
        }
        let id = |name: &str| {
            book.profile
                .opcodes
                .iter()
                .find(|row| row.command.as_deref() == Some(name))
                .unwrap()
                .id
        };
        let mut control = Script {
            name: vec![],
            locals: Counts {
                int: 1,
                ..Default::default()
            },
            args: Counts {
                int: 1,
                ..Default::default()
            },
            code: vec![
                Instruction {
                    opcode: id("push_int_local"),
                    operand: Operand::Int(0),
                },
                Instruction {
                    opcode: id("switch"),
                    operand: Operand::Int(0),
                },
                Instruction {
                    opcode: id("branch"),
                    operand: Operand::Int(1),
                },
                Instruction {
                    opcode: id("return"),
                    operand: Operand::Byte(0),
                },
                Instruction {
                    opcode: id("return"),
                    operand: Operand::Byte(0),
                },
            ],
            switches: vec![vec![
                SwitchCase {
                    value: 1,
                    offset: 1,
                },
                SwitchCase {
                    value: 1,
                    offset: 2,
                },
            ]],
        };
        let control_bytes = encode(&control, &book).unwrap();
        let control_ir = crate::semantic::normalize(&control_bytes, &book).unwrap();
        assert_eq!(
            control_ir.instructions[1]
                .operation
                .as_ref()
                .unwrap()
                .argument,
            crate::semantic::Argument::Switch(vec![
                crate::semantic::Case {
                    value: 1,
                    target: 3
                },
                crate::semantic::Case {
                    value: 1,
                    target: 4
                }
            ])
        );
        assert_eq!(
            control_ir.instructions[2]
                .operation
                .as_ref()
                .unwrap()
                .argument,
            crate::semantic::Argument::Branch(4)
        );
        assert_eq!(encode(&control_ir.to_wire(), &book).unwrap(), control_bytes);
        control.code[2].operand = Operand::Int(2);
        assert!(crate::semantic::normalize(&encode(&control, &book).unwrap(), &book).is_err());
        control.code[2].operand = Operand::Int(1);
        control.locals.int = 0;
        control.args.int = 0;
        assert!(crate::semantic::normalize(&encode(&control, &book).unwrap(), &book).is_err());
        let unknown = book
            .profile
            .opcodes
            .iter()
            .find(|row| row.command.is_none() && row.encoding == Encoding::Byte)
            .unwrap()
            .id;
        let mut opaque = script.clone();
        opaque.code = vec![Instruction {
            opcode: unknown,
            operand: Operand::Byte(RAW_SECONDARY),
        }];
        let opaque_bytes = encode(&opaque, &book).unwrap();
        let opaque_ir = crate::semantic::normalize(&opaque_bytes, &book).unwrap();
        assert!(opaque_ir.instructions[0].operation.is_none());
        assert_eq!(encode(&opaque_ir.to_wire(), &book).unwrap(), opaque_bytes);

        let mut old_profile: Profile =
            serde_json::from_slice(include_bytes!("../../../revisions/950/cs2/950-1.json"))
                .unwrap();
        old_profile
            .opcodes
            .iter_mut()
            .filter(|row| row.encoding == Encoding::Varbit24)
            .for_each(|row| row.encoding = Encoding::Varbit16);
        let old_book = Book::parse(&serde_json::to_vec(&old_profile).unwrap()).unwrap();
        assert!(encode(&script, &old_book).is_err());
        assert!(decode(&bytes, &old_book).is_err());
        // Removing operand bytes must not let the decoder consume the header.
        let mut truncated = bytes.clone();
        let first_operand = script.name.len() + 1 + size_of::<u16>();
        truncated.drain(first_operand..first_operand + 3);
        assert!(decode(&truncated, &book).is_err());
        let mut invalid = script.clone();
        invalid.code[1].operand = Operand::ConstantString(vec![0]);
        assert!(encode(&invalid, &book).is_err());
        old_profile.format = PROFILE_FORMAT + 1;
        assert!(Book::parse(&serde_json::to_vec(&old_profile).unwrap()).is_err());
        assert!(
            book.check_identity("949.1", &book.profile.client_md5, b"wrong index")
                .is_err()
        );
        assert!(
            book.check_identity(
                &book.profile.build.to_string(),
                &book.profile.client_md5,
                b"wrong index"
            )
            .is_err()
        );
    }
    fn verify_dependency_accounting(book: &Book) {
        use crate::import910::{DependencyGapKind, inspect_closure};
        let opcode = |command: &str| {
            book.profile
                .opcodes
                .iter()
                .find(|row| row.command.as_deref() == Some(command))
                .unwrap()
                .id
        };
        let caller_id = u32::default();
        let callee_id = caller_id + u32::from(true);
        let mut caller = Script {
            name: vec![],
            locals: Counts::default(),
            args: Counts::default(),
            code: vec![
                Instruction {
                    opcode: opcode("gosub_with_params"),
                    operand: Operand::Int(i32::try_from(callee_id).unwrap()),
                },
                Instruction {
                    opcode: opcode("return"),
                    operand: Operand::Byte(u8::default()),
                },
            ],
            switches: vec![],
        };
        let callee = Script {
            code: vec![Instruction {
                opcode: opcode("return"),
                operand: Operand::Byte(u8::default()),
            }],
            ..caller.clone()
        };
        let mut scripts = std::collections::BTreeMap::from([
            (caller_id, encode(&caller, book).unwrap()),
            (callee_id, encode(&callee, book).unwrap()),
        ]);
        let inspection = inspect_closure(&scripts, book).unwrap();
        assert!(inspection.dependency_complete);
        assert_eq!(inspection.calls.len(), 1);
        assert_eq!(inspection.calls[0].callee, callee_id);
        scripts.remove(&callee_id);
        let inspection = inspect_closure(&scripts, book).unwrap();
        assert!(!inspection.dependency_complete);
        assert!(matches!(
            inspection.unresolved_dependencies[0].reason,
            DependencyGapKind::MissingNamedCallee
        ));
        caller.code[0] = Instruction {
            opcode: opcode("if_setonop"),
            operand: Operand::Byte(u8::default()),
        };
        scripts.insert(caller_id, encode(&caller, book).unwrap());
        let inspection = inspect_closure(&scripts, book).unwrap();
        assert!(!inspection.dependency_complete);
        assert!(inspection.calls.is_empty());
        assert!(matches!(
            inspection.unresolved_dependencies[0].reason,
            DependencyGapKind::CallbackHeadNeedsStackProof
        ));
        let unknown = book
            .profile
            .opcodes
            .iter()
            .find(|row| row.command.is_none() && row.encoding == Encoding::Byte)
            .unwrap()
            .id;
        caller.code[0] = Instruction {
            opcode: unknown,
            operand: Operand::Byte(u8::default()),
        };
        scripts.insert(caller_id, encode(&caller, book).unwrap());
        let inspection = inspect_closure(&scripts, book).unwrap();
        assert!(!inspection.dependency_complete);
        assert!(matches!(
            inspection.unresolved_dependencies[0].reason,
            DependencyGapKind::UnrecoveredHandler
        ));
    }

    fn verify_reviewed_lowering(book: &Book) {
        use crate::lower910::Adapter;
        use native910::{
            opcode::OpcodeBook,
            runtime::RuntimeHost,
            vm::{Value, Vm},
        };
        let target = OpcodeBook::embedded().unwrap();
        let bytes = include_bytes!("../../../revisions/950/cs2/950-1-to-910.json");
        let adapter = Adapter::parse(bytes, book, &target).unwrap();
        let opcode = |name: &str| {
            book.profile
                .opcodes
                .iter()
                .find(|row| row.command.as_deref() == Some(name))
                .unwrap()
                .id
        };
        let cases = [
            (
                vec![Operand::ConstantInt(i32::MAX), Operand::ConstantInt(1)],
                Some("add"),
                Value::Int(i32::MIN),
            ),
            (
                vec![Operand::ConstantInt(i32::MIN), Operand::ConstantInt(1)],
                Some("subtract"),
                Value::Int(i32::MAX),
            ),
            (
                vec![Operand::ConstantLong(i64::MIN)],
                None,
                Value::Long(i64::MIN),
            ),
            (
                vec![Operand::ConstantString(vec![0x81, 0xff])],
                None,
                Value::Str("ÿ".into()),
            ),
        ];
        for (constants, operation, expected) in cases {
            let mut code: Vec<_> = constants
                .into_iter()
                .map(|operand| Instruction {
                    opcode: opcode("push_constant"),
                    operand,
                })
                .collect();
            if let Some(command) = operation {
                code.push(Instruction {
                    opcode: opcode(command),
                    operand: Operand::Byte(0),
                });
            }
            code.push(Instruction {
                opcode: opcode("return"),
                operand: Operand::Byte(0),
            });
            let source = Script {
                name: vec![],
                locals: Counts::default(),
                args: Counts::default(),
                code,
                switches: vec![],
            };
            let ir = crate::semantic::normalize(&encode(&source, book).unwrap(), book).unwrap();
            let lowered = adapter
                .lower(&ir, &Default::default(), "portable_value")
                .unwrap();
            assert!(lowered.host_requirements.is_empty());
            let encoded = native910::script::encode_script(&lowered.script, &target).unwrap();
            let decoded = native910::script::decode_script(&encoded, &target).unwrap();
            assert_eq!(decoded, lowered.script);
            let mut host = RuntimeHost::default();
            assert_eq!(
                Vm::new(&mut host, &std::collections::HashMap::new())
                    .execute(&decoded, &[])
                    .unwrap(),
                Some(expected)
            );
        }
        // Operand-counted joins retain wire bytes and their string requirement.
        const JOIN_PC: usize = 3;
        const JOIN_PARTS: i32 = 3;
        const EMPTY_JOIN: i32 = 0;
        const IDENTITY_JOIN: i32 = 1;
        for (parts, constants, expected) in [
            (EMPTY_JOIN, Vec::new(), ""),
            (IDENTITY_JOIN, vec!["identity"], "identity"),
            (
                JOIN_PARTS,
                vec!["first:", "middle", ":last"],
                "first:middle:last",
            ),
        ] {
            let mut code: Vec<_> = constants
                .into_iter()
                .map(|text| Instruction {
                    opcode: opcode("push_constant"),
                    operand: Operand::ConstantString(text.as_bytes().to_vec()),
                })
                .collect();
            let join_pc = code.len();
            code.push(Instruction {
                opcode: opcode("join_string"),
                operand: Operand::Int(parts),
            });
            code.push(Instruction {
                opcode: opcode("return"),
                operand: Operand::Byte(u8::default()),
            });
            let source = Script {
                name: vec![],
                locals: Counts::default(),
                args: Counts::default(),
                code,
                switches: vec![],
            };
            let wire = encode(&source, book).unwrap();
            let ir = crate::semantic::normalize(&wire, book).unwrap();
            assert_eq!(encode(&ir.to_wire(), book).unwrap(), wire);
            assert_eq!(
                ir.instructions[join_pc]
                    .operation
                    .as_ref()
                    .unwrap()
                    .argument,
                crate::semantic::Argument::Count(parts)
            );
            let lowered = adapter
                .lower(&ir, &Default::default(), "portable_join")
                .unwrap();
            assert_eq!(lowered.host_requirements.len(), 1);
            assert_eq!(lowered.host_requirements[0].pc, join_pc);
            assert_eq!(
                lowered.host_requirements[0].requirement,
                crate::lower910::Requirement::StringJoin
            );
            let encoded = native910::script::encode_script(&lowered.script, &target).unwrap();
            assert_eq!(
                native910::script::decode_script(&encoded, &target).unwrap(),
                lowered.script
            );
            let mut host = RuntimeHost::default();
            assert_eq!(
                Vm::new(&mut host, &std::collections::HashMap::new())
                    .execute(&lowered.script, &[])
                    .unwrap(),
                Some(Value::Str(expected.into()))
            );
            if parts == JOIN_PARTS {
                let mut malformed = ir;
                malformed.instructions[JOIN_PC]
                    .operation
                    .as_mut()
                    .unwrap()
                    .argument = crate::semantic::Argument::Count(-IDENTITY_JOIN);
                assert!(
                    adapter
                        .lower(&malformed, &Default::default(), "negative_join")
                        .is_err()
                );
                malformed.instructions[JOIN_PC]
                    .operation
                    .as_mut()
                    .unwrap()
                    .argument =
                    crate::semantic::Argument::Count(i32::from(u16::MAX) + IDENTITY_JOIN);
                assert!(
                    adapter
                        .lower(&malformed, &Default::default(), "oversized_join")
                        .is_err()
                );
            }
        }
        const ONE_INTEGER_ARGUMENT: u16 = 1;
        const INPUT_SLOT: i32 = 0;
        const SWITCH_PC: i32 = 1;
        const NEXT_INSTRUCTION: i32 = 1;
        const FIRST_TARGET: i32 = 4;
        const LAST_TARGET: i32 = 6;
        const DUPLICATE_KEY: i32 = 1;
        const DEFAULT_RESULT: i32 = -1;
        const FIRST_RESULT: i32 = 11;
        const LAST_RESULT: i32 = 22;
        let mut switched = Script {
            name: vec![],
            locals: Counts {
                int: ONE_INTEGER_ARGUMENT,
                ..Default::default()
            },
            args: Counts {
                int: ONE_INTEGER_ARGUMENT,
                ..Default::default()
            },
            code: vec![
                Instruction {
                    opcode: opcode("push_int_local"),
                    operand: Operand::Int(INPUT_SLOT),
                },
                Instruction {
                    opcode: opcode("switch"),
                    operand: Operand::Int(0),
                },
                Instruction {
                    opcode: opcode("push_constant"),
                    operand: Operand::ConstantInt(DEFAULT_RESULT),
                },
                Instruction {
                    opcode: opcode("return"),
                    operand: Operand::Byte(0),
                },
                Instruction {
                    opcode: opcode("push_constant"),
                    operand: Operand::ConstantInt(FIRST_RESULT),
                },
                Instruction {
                    opcode: opcode("return"),
                    operand: Operand::Byte(0),
                },
                Instruction {
                    opcode: opcode("push_constant"),
                    operand: Operand::ConstantInt(LAST_RESULT),
                },
                Instruction {
                    opcode: opcode("return"),
                    operand: Operand::Byte(0),
                },
            ],
            switches: vec![vec![
                SwitchCase {
                    value: DUPLICATE_KEY,
                    offset: FIRST_TARGET - SWITCH_PC - NEXT_INSTRUCTION,
                },
                SwitchCase {
                    value: DUPLICATE_KEY,
                    offset: LAST_TARGET - SWITCH_PC - NEXT_INSTRUCTION,
                },
                SwitchCase {
                    value: i32::MIN,
                    offset: FIRST_TARGET - SWITCH_PC - NEXT_INSTRUCTION,
                },
                SwitchCase {
                    value: i32::MAX,
                    offset: LAST_TARGET - SWITCH_PC - NEXT_INSTRUCTION,
                },
            ]],
        };
        let raw_switch = encode(&switched, book).unwrap();
        let ir = crate::semantic::normalize(&raw_switch, book).unwrap();
        assert_eq!(
            ir.switch_lookup,
            Some(crate::profile::SwitchLookup::LastEntryWins)
        );
        assert_eq!(encode(&ir.to_wire(), book).unwrap(), raw_switch);
        let lowered = adapter
            .lower(&ir, &Default::default(), "portable_switch")
            .unwrap();
        let encoded = native910::script::encode_script(&lowered.script, &target).unwrap();
        let decoded = native910::script::decode_script(&encoded, &target).unwrap();
        for (input, result) in [
            (DUPLICATE_KEY, LAST_RESULT),
            (INPUT_SLOT, DEFAULT_RESULT),
            (i32::MIN, FIRST_RESULT),
            (i32::MAX, LAST_RESULT),
        ] {
            let mut host = RuntimeHost::default();
            assert_eq!(
                Vm::new(&mut host, &std::collections::HashMap::new())
                    .execute(&decoded, &[Value::Int(input)])
                    .unwrap(),
                Some(Value::Int(result))
            );
        }
        // Reverse the duplicates: lookup depends on source order, not key sort.
        switched.switches[0].swap(0, 1);
        let ir = crate::semantic::normalize(&encode(&switched, book).unwrap(), book).unwrap();
        let lowered = adapter
            .lower(&ir, &Default::default(), "reversed_switch")
            .unwrap();
        let mut host = RuntimeHost::default();
        assert_eq!(
            Vm::new(&mut host, &std::collections::HashMap::new())
                .execute(&lowered.script, &[Value::Int(DUPLICATE_KEY)])
                .unwrap(),
            Some(Value::Int(FIRST_RESULT))
        );

        const EXTRA_LOCAL: u16 = 1;
        let mut locals = Script {
            name: vec![],
            locals: Counts {
                int: EXTRA_LOCAL,
                ..Default::default()
            },
            args: Counts::default(),
            code: vec![
                Instruction {
                    opcode: opcode("push_int_local"),
                    operand: Operand::Int(0),
                },
                Instruction {
                    opcode: opcode("return"),
                    operand: Operand::Byte(0),
                },
            ],
            switches: vec![],
        };
        let ir = crate::semantic::normalize(&encode(&locals, book).unwrap(), book).unwrap();
        assert!(
            adapter
                .lower(&ir, &Default::default(), "unassigned_value")
                .err()
                .expect("unassigned modern local must be rejected")
                .to_string()
                .contains("definite assignment")
        );
        locals.code.splice(
            0..0,
            [
                Instruction {
                    opcode: opcode("push_constant"),
                    operand: Operand::ConstantInt(1),
                },
                Instruction {
                    opcode: opcode("pop_int_local"),
                    operand: Operand::Int(0),
                },
            ],
        );
        let ir = crate::semantic::normalize(&encode(&locals, book).unwrap(), book).unwrap();
        assert!(
            adapter
                .lower(&ir, &Default::default(), "assigned_value")
                .is_ok()
        );
        let mut changed: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        changed["rules"][0]["source_handler"] = serde_json::json!(0);
        assert!(Adapter::parse(&serde_json::to_vec(&changed).unwrap(), book, &target).is_err());
        let mut changed = book.profile.clone();
        changed.string_encoding = Some(crate::profile::StringEncoding::Windows1252QuestionMark);
        let changed = Book::parse(&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(Adapter::parse(bytes, &changed, &target).is_err());
        let mut changed = book.profile.clone();
        changed.switch_lookup = Some(crate::profile::SwitchLookup::FirstEntryWins);
        let changed = Book::parse(&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(Adapter::parse(bytes, &changed, &target).is_err());
        let mut changed = book.profile.clone();
        changed.switch_lookup = None;
        let changed = Book::parse(&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(Adapter::parse(bytes, &changed, &target).is_err());
    }
}
