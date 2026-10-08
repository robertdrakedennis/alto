//! The Rust VM against instruction traces recorded once from the original
//! client's script interpreter (`tests/fixtures/recorded/`): a synthetic
//! script mixing calls, branches, switches, arrays, host-gated commands and
//! every value lane, and a real cache script (1258) run under two settings
//! whose recorded return depths differ.
mod common;

use native910::opcode::OpcodeBook;
use native910::runtime::RuntimeHost;
use native910::script::{CompiledScript, Counts, Instruction, Operand, SwitchCase};
use native910::vm::{Session, Vm};
use std::collections::HashMap;

fn instruction(command: &str, operand: Operand) -> Instruction {
    Instruction {
        opcode: 0,
        command: command.into(),
        operand,
    }
}

#[test]
fn recorded_and_native_match_every_instruction() {
    let callee = CompiledScript {
        name: Some("callee".into()),
        locals: Counts {
            int: 1,
            obj: 0,
            long: 0,
        },
        args: Counts {
            int: 1,
            obj: 0,
            long: 0,
        },
        code: vec![
            instruction("push_int_local", Operand::Local(0)),
            instruction("push_constant_int", Operand::Int(1)),
            instruction("add", Operand::Byte(0)),
            instruction("return", Operand::Byte(0)),
        ],
    };
    let main = CompiledScript {
        name: Some("main".into()),
        locals: Counts::default(),
        args: Counts::default(),
        code: vec![
            instruction("push_constant_int", Operand::Int(41)),
            instruction("gosub_with_params", Operand::Script(2)),
            instruction("push_constant_int", Operand::Int(0)),
            instruction("branch_if_false", Operand::Branch(5)),
            instruction("push_constant_int", Operand::Int(999)),
            instruction("push_constant_int", Operand::Int(7)),
            instruction(
                "switch",
                Operand::Switch(vec![SwitchCase {
                    value: 7,
                    target: 8,
                }]),
            ),
            instruction("push_constant_int", Operand::Int(999)),
            instruction("push_constant_int", Operand::Int(8)),
            instruction("add", Operand::Byte(0)),
            instruction("push_constant_int", Operand::Int(2)),
            instruction("define_array", Operand::Array(2 << 16)),
            instruction("push_constant_int", Operand::Int(0)),
            instruction("push_constant_int", Operand::Int(100)),
            instruction("pop_array_int", Operand::Array(2)),
            instruction("push_constant_int", Operand::Int(0)),
            instruction("push_array_int", Operand::Array(2)),
            instruction("add", Operand::Byte(0)),
            instruction("push_constant_int", Operand::Int(2)),
            instruction("detailcanset_vsync", Operand::Byte(0)),
            instruction("pop_int_discard", Operand::Byte(0)),
            instruction("pop_int_discard", Operand::Byte(0)),
            instruction("push_constant_int", Operand::Int(42)),
            instruction("pop_int_discard", Operand::Byte(0)),
            instruction("push_constant_string", Operand::Str("discard".into())),
            instruction("pop_string_discard", Operand::Byte(0)),
            instruction("push_constant_string", Operand::Long(42)),
            instruction("pop_long_discard", Operand::Byte(0)),
            instruction("return", Operand::Byte(0)),
        ],
    };
    let mut host = RuntimeHost::default();
    let provider = HashMap::from([(2, callee)]);
    let mut vm = Vm::new(&mut host, &provider);
    let mut session = Session::new(&main, &[]).unwrap();
    let mut lines = Vec::new();
    while !session.finished() {
        let s = session.snapshot();
        lines.push(format!(
            "{}\t{}\t{:?}\t{}\t{:?}\t{}\t{:?}\t{}\t{:?}",
            s.script_name.unwrap(),
            s.pc,
            s.ints,
            common::utf16_objects(s.strings.iter().map(Option::as_deref)),
            s.longs,
            s.frames.len(),
            s.int_locals,
            common::utf16_objects(s.string_locals.iter().map(Option::as_deref)),
            s.long_locals
        ));
        vm.step(&mut session).unwrap();
    }
    let actual = lines.join("\n") + "\n";
    let recorded =
        std::fs::read_to_string(common::fixture("recorded").join("cs2-instruction-mix.trace"))
            .unwrap();
    for (step, (expected, actual)) in recorded.lines().zip(actual.lines()).enumerate() {
        assert_eq!(expected, actual, "first divergence at step {step}");
    }
    assert_eq!(
        recorded.lines().count(),
        lines.len(),
        "trace lengths differ"
    );
    assert_eq!(session.snapshot().ints, vec![150]);
}

/// Real cache script 1258 under two setting keys, whose recorded traces end
/// with different return depths (`[1]` and `[2, 1]`).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn script_1258_matches_recorded_traces_for_both_return_depths() {
    use native910::{pack::PackArchive, script::decode_script, vm::Value};
    let archive = PackArchive::open(&common::require_pack_file("client.scripts.js5")).unwrap();
    let bytes = archive
        .group_files(1258)
        .unwrap()
        .unwrap()
        .remove(&0)
        .unwrap();
    let script = decode_script(&bytes, &OpcodeBook::embedded().unwrap()).unwrap();
    for (key, expected) in [(0, vec![1]), (10858, vec![2, 1])] {
        let provider = HashMap::<i32, CompiledScript>::new();
        let mut host = RuntimeHost::default();
        let mut vm = Vm::new(&mut host, &provider);
        let mut session = Session::new(&script, &[Value::Int(key), Value::Int(2)]).unwrap();
        let mut lines = Vec::new();
        while !session.finished() {
            let s = session.snapshot();
            lines.push(format!(
                "{}\t{}\t{:?}\t{}\t{:?}\t{}\t{:?}\t{}\t{:?}",
                s.script_name.as_deref().unwrap_or("null"),
                s.pc,
                s.ints,
                common::utf16_objects(s.strings.iter().map(Option::as_deref)),
                s.longs,
                s.frames.len(),
                s.int_locals,
                common::utf16_objects(s.string_locals.iter().map(Option::as_deref)),
                s.long_locals
            ));
            vm.step(&mut session).unwrap();
        }
        assert_eq!(
            lines.last().unwrap().split('\t').nth(2).unwrap(),
            format!("{expected:?}")
        );
        // The recording is stored as one line digest per instruction.
        let base = common::fixture("recorded").join(format!("script-1258-setting-{key}"));
        assert_eq!(
            common::first_trace_divergence(&base, &lines),
            None,
            "setting {key}"
        );
    }
}
