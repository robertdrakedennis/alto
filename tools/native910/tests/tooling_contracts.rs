use native910::config::ConfigTypes;
use native910::inames::InterfaceRegistry;
use native910::opcode::OpcodeBook;
use native910::parse::parse_source_mapped;
use native910::runtime::RuntimeHost;
use native910::script::{CompiledScript, Counts, Instruction, Operand};
use native910::semantics::{Effect, StateEffect, contract, effect, state_effect};
use native910::source::lower_mapped;
use native910::symbols::SymbolRegistry;
use native910::vm::{Host, InstructionContext, Session, Value, Vm, VmError, VmResult};

fn script(code: Vec<Instruction>) -> CompiledScript {
    CompiledScript {
        name: Some("fixture".into()),
        locals: Counts::default(),
        args: Counts::default(),
        code,
    }
}
fn op(command: &str, operand: Operand) -> Instruction {
    Instruction {
        opcode: 0,
        command: command.into(),
        operand,
    }
}

/// Every retail opcode's public contract against the committed stack-contract
/// table (`native910::cs2_stack_contracts`): where the table holds a fixed
/// shape the contract must state exactly that shape and a `Fixed` type rule;
/// where it does not, the contract must still carry an explicit (non-Unknown)
/// rule backed by a hand-checked handler.
#[test]
fn retail_contracts_match_the_stack_contract_table() {
    let book = OpcodeBook::embedded().unwrap();
    let mut compared = 0;
    let mut mismatches = Vec::new();
    for (opcode, name) in book.entries().filter(|(id, _)| *id <= 1431) {
        let operand = if name == "push_constant_string" {
            Operand::Str(String::new())
        } else {
            Operand::Byte(0)
        };
        let row = native910::semantics::contract_for_opcode(&book, opcode, &operand).unwrap();
        assert_ne!(
            row.type_rule,
            native910::semantics::TypeRule::Unknown,
            "{name}"
        );
        let Some(table) = native910::cs2_stack_contracts::effect(name) else {
            continue;
        };
        compared += 1;
        if row.effect != table {
            mismatches.push(format!(
                "{name}: contract {:?}, table {table:?}",
                row.effect
            ));
        }
        if !matches!(
            row.type_rule,
            native910::semantics::TypeRule::Fixed
                | native910::semantics::TypeRule::Operand
                | native910::semantics::TypeRule::ParamDefinition
                | native910::semantics::TypeRule::HookDescriptor
                | native910::semantics::TypeRule::RuntimeState
        ) {
            mismatches.push(format!(
                "{name}: fixed shape in the table but type rule {:?}",
                row.type_rule
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    assert!(compared >= 1200, "only {compared} rows compared");
    for name in ["worldlist_pingworlds", "video_advert_play"] {
        assert!(matches!(
            effect(name, &Operand::Byte(0)),
            Effect::Conditional { .. }
        ));
    }
    assert_eq!(effect("retail_trap_79", &Operand::Byte(0)), Effect::Trap);
}

#[test]
fn stack_shape_is_not_purity_and_synthetic_is_unverified() {
    assert!(matches!(
        effect("random", &Operand::Byte(0)),
        Effect::Fixed { .. }
    ));
    assert_eq!(
        state_effect("random", &Operand::Byte(0)),
        StateEffect::Nondeterministic
    );
    assert_eq!(
        state_effect("if_settext", &Operand::Byte(0)),
        StateEffect::Host
    );
    let book = OpcodeBook::embedded().unwrap();
    let row = contract(&book, "if_slider_setup", &Operand::Byte(0)).unwrap();
    assert!(!row.retail_dispatch);
    assert_eq!(row.effect, Effect::Unknown);
}

#[test]
fn expanded_expressions_and_switch_map_to_authored_lines() {
    let text = "[clientscript,test]()\nint $x;\n// comment\n$x = 2 + 3;\npush_int_local($x);\nswitch {\ncase 5 -> done;\n}\ndone:\nreturn(0);\n";
    let symbols = SymbolRegistry::empty();
    let configs = ConfigTypes::empty();
    let (source, lines) =
        parse_source_mapped(text, &symbols, &configs, &InterfaceRegistry::empty()).unwrap();
    let (compiled, map) = lower_mapped(
        &source,
        &OpcodeBook::embedded().unwrap(),
        &symbols,
        &configs,
    )
    .unwrap();
    let mapped: Vec<_> = map.iter().map(|i| lines[*i]).collect();
    assert_eq!(compiled.code.len(), mapped.len());
    assert_eq!(mapped, vec![4, 4, 4, 4, 5, 6, 10]);
}

#[test]
fn paused_execution_retains_stacks_and_failure_cannot_resume() {
    let prog = script(vec![
        op("push_constant_int", Operand::Int(41)),
        op("push_constant_int", Operand::Int(1)),
        op("add", Operand::Byte(0)),
        op("return", Operand::Byte(0)),
    ]);
    let mut host = RuntimeHost::default();
    let mut vm = Vm::new(&mut host, &());
    let mut session = Session::new(&prog, &[]).unwrap();
    assert!(!vm.step(&mut session).unwrap());
    assert_eq!(session.snapshot().ints, vec![41]);
    assert!(!vm.step(&mut session).unwrap());
    assert!(!vm.step(&mut session).unwrap());
    assert_eq!(session.snapshot().ints, vec![42]);
    assert!(vm.step(&mut session).unwrap());
    let bad = script(vec![op("add", Operand::Byte(0))]);
    let mut session = Session::new(&bad, &[]).unwrap();
    assert!(vm.step(&mut session).is_err());
    assert!(session.snapshot().failed);
    assert!(vm.step(&mut session).is_err());
}

#[test]
fn unsupported_host_operations_are_not_padded() {
    // A missing component remains an explicit error rather than a placeholder value.
    let mut host = RuntimeHost::default();
    let operand = Operand::Byte(1);
    let context = InstructionContext {
        script_id: None,
        event: None,
        script_name: Some("fixture"),
        pc: 3,
        command: "if_gettext",
        operand: &operand,
        secondary: true,
        int_locals: &[],
    };
    let error = host
        .trap_context(&context, &mut vec![123], &mut vec![], &mut vec![])
        .unwrap_err();
    assert!(matches!(error, VmError::UnknownCommand { .. }));
}

#[test]
fn variable_types_do_not_depend_on_current_values() -> VmResult<()> {
    use native910::vars::VarScope;
    use native910::vm::VarLane;
    let mut host = RuntimeHost::default();
    host.definitions
        .insert((VarScope::Client, 42), VarLane::String);
    assert_eq!(
        host.var_get(VarScope::Client, 42, false)?,
        Value::Str(String::new())
    );
    assert!(host.var_type(VarScope::Client, 43).is_err());
    Ok(())
}

#[test]
fn dynamic_find_minus_one_preserves_scope() {
    let mut host = RuntimeHost::default();
    host.component_text.insert((65536, 0), "old".into());
    host.active_components[0] = Some((65536, 0));
    let operand = Operand::Byte(0);
    let context = InstructionContext {
        script_id: None,
        event: None,
        script_name: Some("test"),
        pc: 0,
        command: "cc_find",
        operand: &operand,
        secondary: false,
        int_locals: &[],
    };
    assert_eq!(
        host.trap_context(&context, &mut vec![123, -1], &mut vec![], &mut vec![])
            .unwrap(),
        Some(Value::Int(0))
    );
    assert_eq!(host.active_components[0], Some((65536, 0)));
}

#[test]
fn analyzer_reports_consumed_condition_underflow() {
    use native910::returns::{FailureKind, analyze};
    let code = script(vec![
        op("branch_if_false", Operand::Branch(1)),
        op("return", Operand::Byte(0)),
    ]);
    let analysis = analyze(
        &code,
        &std::collections::BTreeMap::new(),
        &std::collections::BTreeMap::new(),
    );
    assert_eq!(analysis.failure, Some((0, FailureKind::StackUnderflow)));
}

#[test]
fn falling_off_script_is_not_a_return() {
    let code = script(vec![op("push_constant_int", Operand::Int(1))]);
    let mut session = Session::new(&code, &[]).unwrap();
    let mut host = RuntimeHost::default();
    let mut vm = Vm::new(&mut host, &());
    assert!(!vm.step(&mut session).unwrap());
    assert!(vm.step(&mut session).is_err());
}

#[test]
fn source_breakpoints_resume_and_step_expanded_expressions() {
    use native910::debug::{Debugger, Stop};
    let text = "[clientscript,test]()\nint $x;\n$x = 2 + 3;\npush_int_local($x);\nreturn(0);\n";
    let symbols = SymbolRegistry::empty();
    let configs = ConfigTypes::empty();
    let (source, lines) =
        parse_source_mapped(text, &symbols, &configs, &InterfaceRegistry::empty()).unwrap();
    let (compiled, map) = lower_mapped(
        &source,
        &OpcodeBook::embedded().unwrap(),
        &symbols,
        &configs,
    )
    .unwrap();
    let mut debugger =
        Debugger::new(Session::for_script(1, &compiled, &[], Some("test event".into())).unwrap());
    debugger
        .source_maps
        .insert(1, map.iter().map(|i| lines[*i]).collect());
    assert!(debugger.break_on_line(1, 3));
    let mut host = RuntimeHost::default();
    let mut vm = Vm::new(&mut host, &());
    assert_eq!(debugger.resume(&mut vm).unwrap(), Stop::Breakpoint);
    assert_eq!(debugger.snapshot().pc, 0);
    assert_eq!(debugger.step_source(&mut vm).unwrap(), Stop::Step);
    assert_eq!(debugger.source_line(), Some(4));
    assert_eq!(debugger.snapshot().int_locals, vec![5]);
    assert_eq!(debugger.resume(&mut vm).unwrap(), Stop::Finished);
    assert_eq!(debugger.snapshot().ints, vec![5]);
}

#[test]
fn watchpoint_stops_after_variable_write() {
    use native910::debug::{Debugger, Stop, Watchpoint};
    use native910::script::VarRef;
    use native910::vars::VarScope;
    use native910::vm::VarLane;
    let code = script(vec![
        op("push_constant_int", Operand::Int(7)),
        op(
            "pop_var",
            Operand::VarRef(VarRef {
                domain: VarScope::Client,
                id: 42,
                transmog: false,
            }),
        ),
        op("return", Operand::Byte(0)),
    ]);
    let mut host = RuntimeHost::default();
    host.definitions
        .insert((VarScope::Client, 42), VarLane::Int);
    let mut debugger = Debugger::new(Session::for_script(5, &code, &[], None).unwrap());
    debugger
        .watchpoints
        .insert(Watchpoint::Variable(VarScope::Client, 42, false));
    let mut vm = Vm::new(&mut host, &());
    assert_eq!(debugger.resume(&mut vm).unwrap(), Stop::Watchpoint);
    assert_eq!(debugger.snapshot().pc, 2);
    assert_eq!(debugger.snapshot().effects.len(), 1);
    assert_eq!(debugger.snapshot().last_instruction, Some((Some(5), 1)));
    assert_eq!(debugger.resume(&mut vm).unwrap(), Stop::Finished);
}
