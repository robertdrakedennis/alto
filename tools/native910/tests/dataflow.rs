mod common;
use native910::{
    config::ConfigTypes,
    dataflow::{self, Constant},
    returns::{FailureKind, ReturnArity},
    script::{CompiledScript, Counts, Instruction, Operand},
    semantics::Effect,
};
use std::collections::{BTreeMap, BTreeSet};
fn op(name: &str, operand: Operand) -> Instruction {
    Instruction {
        opcode: 0,
        command: name.into(),
        operand,
    }
}
fn script(code: Vec<Instruction>) -> CompiledScript {
    CompiledScript {
        name: None,
        args: Counts {
            int: 1,
            ..Counts::default()
        },
        locals: Counts {
            int: 2,
            ..Counts::default()
        },
        code,
    }
}
fn analyze(s: &CompiledScript) -> dataflow::Analysis {
    dataflow::analyze(s, &BTreeMap::new(), &BTreeMap::new(), &ConfigTypes::empty())
}
#[test]
fn stub_literal_preserves_unconsumed_input() {
    let s = script(vec![
        op("push_constant_int", Operand::Int(2)),
        op("detailcanset_vsync", Operand::Byte(0)),
        op("return", Operand::Byte(0)),
    ]);
    let result = analyze(&s);
    assert_eq!(result.failure, None);
    let returned = result.returned.unwrap();
    assert_eq!(
        returned[0]
            .iter()
            .map(|v| v.constant.clone())
            .collect::<Vec<_>>(),
        vec![Some(Constant::Int(2)), Some(Constant::Int(0))]
    );
}
fn diamond(right: i32) -> CompiledScript {
    script(vec![
        op("push_int_local", Operand::Local(0)),
        op("branch_if_true", Operand::Branch(5)),
        op("push_constant_string", Operand::Int(36)),
        op("pop_int_local", Operand::Local(1)),
        op("branch", Operand::Branch(7)),
        op("push_constant_string", Operand::Int(right)),
        op("pop_int_local", Operand::Local(1)),
        op("push_int_local", Operand::Local(1)),
        op("return", Operand::Byte(0)),
    ])
}
#[test]
fn joins_preserve_agreement_and_union_definitions() {
    let a = analyze(&diamond(36));
    assert_eq!(a.failure, None);
    let value = &a.before[8].as_ref().unwrap().stacks[0][0];
    assert_eq!(value.constant, Some(Constant::Int(36)));
    assert_eq!(
        value.definitions.iter().copied().collect::<Vec<_>>(),
        vec![2, 5]
    );
    let a = analyze(&diamond(0));
    assert_eq!(a.before[8].as_ref().unwrap().stacks[0][0].constant, None);
    verify_string_representation_joins();
}
fn verify_string_representation_joins() {
    const CALLER_ID: i32 = 1;
    const CALLEE_ID: i32 = 2;
    const SELECTOR_SLOT: i32 = 0;
    const LEFT_SLOT: i32 = 0;
    const RIGHT_SLOT: i32 = 1;
    const RESULT_SLOT: i32 = 2;
    const RIGHT_PATH: i32 = 5;
    const JOIN_PATH: i32 = 7;
    const RESULT_PC: usize = 8;
    const ONE_ARGUMENT: u16 = 1;
    const STRING_ARGUMENTS: u16 = 2;
    const STRING_LOCALS: u16 = 3;
    let chooser = CompiledScript {
        name: None,
        args: Counts {
            int: ONE_ARGUMENT,
            obj: STRING_ARGUMENTS,
            ..Default::default()
        },
        locals: Counts {
            int: ONE_ARGUMENT,
            obj: STRING_LOCALS,
            ..Default::default()
        },
        code: vec![
            op("push_int_local", Operand::Local(SELECTOR_SLOT)),
            op("branch_if_true", Operand::Branch(RIGHT_PATH)),
            op("push_string_local", Operand::Local(LEFT_SLOT)),
            op("pop_string_local", Operand::Local(RESULT_SLOT)),
            op("branch", Operand::Branch(JOIN_PATH)),
            op("push_string_local", Operand::Local(RIGHT_SLOT)),
            op("pop_string_local", Operand::Local(RESULT_SLOT)),
            op("push_string_local", Operand::Local(RESULT_SLOT)),
            op("return", Operand::Byte(u8::default())),
        ],
    };
    let configs = ConfigTypes::empty();
    let context = dataflow::EntryContext {
        arguments: Default::default(),
        nonnull_strings: vec![true, true],
    };
    let result = dataflow::analyze_with_entry_context(
        &chooser,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &configs,
        &context,
    );
    let value = &result.before[RESULT_PC].as_ref().unwrap().stacks[1][0];
    assert_eq!(value.constant, None);
    assert!(
        value.nonnull_string,
        "different string contents retain their common representation"
    );
    assert!(
        !analyze(&chooser).before[RESULT_PC].as_ref().unwrap().stacks[1][0].nonnull_string,
        "an unspecified object argument is not a string proof"
    );
    let mut nullable = context.clone();
    nullable.nonnull_strings[1] = false;
    assert!(
        !dataflow::analyze_with_entry_context(
            &chooser,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &configs,
            &nullable
        )
        .before[RESULT_PC]
            .as_ref()
            .unwrap()
            .stacks[1][0]
            .nonnull_string
    );
    let mut contradictory = context;
    contradictory.arguments[1] = vec![Some(Constant::Null)];
    assert_eq!(
        dataflow::analyze_with_entry_context(
            &chooser,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &configs,
            &contradictory
        )
        .failure,
        Some((0, FailureKind::InvalidOperand))
    );
    let caller = CompiledScript {
        name: None,
        args: Counts {
            int: ONE_ARGUMENT,
            ..Default::default()
        },
        locals: Counts {
            int: ONE_ARGUMENT,
            ..Default::default()
        },
        code: vec![
            op("push_int_local", Operand::Local(SELECTOR_SLOT)),
            op("push_constant_string", Operand::Str("Left".into())),
            op("push_constant_string", Operand::Str("Right".into())),
            op("gosub_with_params", Operand::Script(CALLEE_ID)),
            op("return", Operand::Byte(u8::default())),
        ],
    };
    let scripts = BTreeMap::from([(CALLER_ID, caller.clone()), (CALLEE_ID, chooser)]);
    let (summaries, values) = dataflow::infer_summaries(&scripts, &configs);
    let result = dataflow::analyze_in_context(
        &caller,
        &scripts,
        &summaries,
        &values,
        &configs,
        &Default::default(),
    );
    let returned = &result.returned.as_ref().unwrap()[1][0];
    assert_eq!(returned.constant, None);
    assert!(
        returned.nonnull_string,
        "callee joins preserve caller representation facts"
    );
}

#[test]
fn loop_widens_local_constant_and_terminates() {
    let s = script(vec![
        op("push_constant_string", Operand::Int(1)),
        op("pop_int_local", Operand::Local(1)),
        op("push_int_local", Operand::Local(0)),
        op("branch_if_false", Operand::Branch(8)),
        op("push_constant_string", Operand::Int(2)),
        op("pop_int_local", Operand::Local(1)),
        op("branch", Operand::Branch(2)),
        op("unknown", Operand::Byte(0)),
        op("push_int_local", Operand::Local(1)),
        op("return", Operand::Byte(0)),
    ]);
    let a = analyze(&s);
    assert_eq!(a.failure, None);
    assert_eq!(a.before[8].as_ref().unwrap().locals[0][1].constant, None);
    assert!(a.before[7].is_none());
}
#[test]
fn enum_lane_uses_reaching_output_type() {
    let s = script(vec![
        op("push_constant_string", Operand::Int(36)),
        op("pop_int_local", Operand::Local(1)),
        op("push_constant_string", Operand::Int(0)),
        op("push_int_local", Operand::Local(1)),
        op("push_constant_string", Operand::Int(9)),
        op("push_constant_string", Operand::Int(1)),
        op("_enum", Operand::Byte(0)),
        op("return", Operand::Byte(0)),
    ]);
    assert_eq!(
        analyze(&s).arity,
        ReturnArity::Known {
            int: 0,
            obj: 1,
            long: 0
        }
    );
}
#[test]
fn unknown_and_inconsistent_paths_fail_explicitly() {
    let mut s = diamond(36);
    s.code[5] = op("unknown", Operand::Byte(0));
    assert_eq!(analyze(&s).failure, Some((5, FailureKind::UnknownCommand)));
    let mut s = diamond(36);
    s.code[6] = op("noopDisplayCommand", Operand::Byte(0));
    assert_eq!(
        analyze(&s).failure,
        Some((7, FailureKind::IncompatibleMerge))
    );
}
#[test]
fn hook_zero_transmit_count_keeps_y_argument() {
    // popIntArray returns null for zero; the caller therefore retains Y as an int argument.
    let lone = format!("{}Y", native910::jstr::from_unit(0xd800));
    let pair = format!("{}Y", native910::jstr::from_text("\u{10f800}"));
    for (descriptor, extra) in [("iY", 0), (lone.as_str(), 0), (pair.as_str(), 1)] {
        for (count, expected) in [(0, 4), (2, 5)] {
            let state = dataflow::State {
                stacks: [
                    vec![dataflow::Value {
                        argument: None,
                        constant: Some(Constant::Int(count)),
                        nonnull_string: false,
                        definitions: BTreeSet::default(),
                    }],
                    vec![dataflow::Value {
                        argument: None,
                        constant: Some(Constant::String(descriptor.into())),
                        nonnull_string: true,
                        definitions: BTreeSet::default(),
                    }],
                    vec![],
                ],
                locals: Default::default(),
            };
            assert_eq!(
                dataflow::resolved_effect(
                    &op("cc_setonclick", Operand::Byte(0)),
                    &state,
                    &ConfigTypes::empty()
                ),
                Effect::Fixed {
                    pops: [expected + extra, 1, 0],
                    pushes: [0; 3]
                }
            );
        }
    }
}
#[test]
fn callee_summary_propagates_without_inventing_return_values() {
    let mut target = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("return", Operand::Byte(0)),
    ]);
    target.locals.int = 1;
    let mut caller = script(vec![
        op("push_constant_string", Operand::Int(7)),
        op("gosub_with_params", Operand::Script(2)),
        op("return", Operand::Byte(0)),
    ]);
    caller.args.int = 0;
    let scripts = BTreeMap::from([(1, caller.clone()), (2, target)]);
    let (summaries, done) = dataflow::infer_returns(&scripts, &ConfigTypes::empty());
    assert!(done);
    assert_eq!(
        summaries[&1],
        ReturnArity::Known {
            int: 1,
            obj: 0,
            long: 0
        }
    );
    assert_eq!(
        dataflow::analyze(&caller, &scripts, &summaries, &ConfigTypes::empty()).before[2]
            .as_ref()
            .unwrap()
            .stacks[0][0]
            .constant,
        None
    );
}

#[test]
fn computed_param_id_selects_the_declared_lane() {
    let configs = ConfigTypes::from_parts(
        BTreeMap::from([(
            42,
            native910::config::ParamConfig {
                kind: Some(36),
                ..native910::config::ParamConfig::default()
            },
        )]),
        BTreeMap::new(),
    );
    let s = script(vec![
        op("push_constant_string", Operand::Int(9)),
        op("push_constant_string", Operand::Int(40)),
        op("push_constant_string", Operand::Int(2)),
        op("add", Operand::Byte(0)),
        op("struct_param", Operand::Byte(0)),
        op("return", Operand::Byte(0)),
    ]);
    let result = dataflow::analyze(&s, &BTreeMap::new(), &BTreeMap::new(), &configs);
    assert_eq!(
        result.arity,
        ReturnArity::Known {
            int: 0,
            obj: 1,
            long: 0
        }
    );
    assert_eq!(result.before[4].as_ref().unwrap().int_from_top(0), Some(42));
    assert_eq!(analyze(&s).failure, Some((4, FailureKind::UnresolvedType)));
}

#[test]
fn retained_array_index_remains_a_reaching_constant() {
    let s = script(vec![
        op("push_constant_string", Operand::Int(3)),
        op("push_array_int_and_index", Operand::Array(0)),
        op("return", Operand::Byte(0)),
    ]);
    let result = analyze(&s);
    assert_eq!(result.before[2].as_ref().unwrap().int_from_top(0), Some(3));
    assert_eq!(result.before[2].as_ref().unwrap().int_from_top(1), None);
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn config_loader_resolves_retail_variable_definition() {
    let root = common::pack_root();
    let configs = ConfigTypes::load(&root).unwrap();
    assert_eq!(
        configs.var_kind(native910::vars::VarScope::Client, 995),
        Some(native910::config::VarValueKind::Int)
    );
}

#[test]
fn xref_uses_agreeing_cfg_values_but_rejects_conflicting_values() {
    for (right, expected) in [(36, 1), (37, 0)] {
        let mut body = diamond(right);
        body.code[8] = op("if_getwidth", Operand::Byte(0));
        body.code.push(op("return", Operand::Byte(0)));
        let scripts = BTreeMap::from([(1, body)]);
        let xref = native910::xref::build_xref_with_configs(
            &scripts,
            &BTreeMap::new(),
            &ConfigTypes::empty(),
        );
        assert_eq!(xref.stats.dangling, expected);
    }
}

#[test]
fn call_argument_alias_unlocks_param_typing_in_caller() {
    let mut identity = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("return", Operand::Byte(0)),
    ]);
    identity.locals.int = 1;
    let mut main = script(vec![
        op("push_constant_string", Operand::Int(9)),
        op("push_constant_string", Operand::Int(42)),
        op("gosub_with_params", Operand::Script(2)),
        op("struct_param", Operand::Byte(0)),
        op("return", Operand::Byte(0)),
    ]);
    main.args.int = 0;
    let configs = ConfigTypes::from_parts(
        BTreeMap::from([(
            42,
            native910::config::ParamConfig {
                kind: Some(36),
                ..native910::config::ParamConfig::default()
            },
        )]),
        BTreeMap::new(),
    );
    let scripts = BTreeMap::from([(1, main), (2, identity)]);
    let (arities, values) = dataflow::infer_summaries(&scripts, &configs);
    assert_eq!(
        arities[&1],
        ReturnArity::Known {
            int: 0,
            obj: 1,
            long: 0
        }
    );
    assert_eq!(values[&2][0][0].argument, Some((0, 0)));
}

#[test]
fn conflicting_return_arguments_do_not_become_an_alias() {
    let mut body = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("branch_if_false", Operand::Branch(4)),
        op("push_int_local", Operand::Local(0)),
        op("return", Operand::Byte(0)),
        op("push_int_local", Operand::Local(1)),
        op("return", Operand::Byte(0)),
    ]);
    body.args.int = 2;
    let scripts = BTreeMap::from([(1, body)]);
    let (_, values) = dataflow::infer_summaries(&scripts, &ConfigTypes::empty());
    assert_eq!(values[&1][0][0].argument, None);
    assert_eq!(values[&1][0][0].constant, None);
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn retail_database_field_uses_whole_tuple_and_unknown_schema_stays_unknown() {
    let root = common::pack_root();
    let configs = ConfigTypes::load(&root).unwrap();
    let field = (0..75 * 256)
        .find(|field| {
            configs
                .db_field_types(*field)
                .is_some_and(|types| types.contains(&36) && types.iter().any(|t| *t != 36))
        })
        .unwrap();
    let types = configs.db_field_types(field).unwrap();
    let body = script(vec![
        op("push_constant_string", Operand::Int(0)),
        op("push_constant_string", Operand::Int(field)),
        op("push_constant_string", Operand::Int(0)),
        op("db_getfield", Operand::Byte(0)),
        op("return", Operand::Byte(0)),
    ]);
    let analysis = dataflow::analyze(&body, &BTreeMap::new(), &BTreeMap::new(), &configs);
    assert_eq!(
        analysis.arity,
        ReturnArity::Known {
            int: types.iter().filter(|t| **t != 36).count() as u16,
            obj: types.iter().filter(|t| **t == 36).count() as u16,
            long: 0
        }
    );
    assert_eq!(
        analyze(&body).failure,
        Some((3, FailureKind::UnresolvedType))
    );
    assert_eq!(
        native910::semantics::effect("db_listall", &Operand::Byte(0)),
        Effect::Unknown
    );
}

#[test]
fn recursive_calls_propagate_array_writes_and_opaque_host_effects() {
    let first = script(vec![
        op("push_array_int", Operand::Array(2)),
        op("gosub_with_params", Operand::Script(2)),
        op("return", Operand::Byte(0)),
    ]);
    let second = script(vec![
        op("pop_array_int", Operand::Array(2)),
        op("cc_settext", Operand::Byte(1)),
        op("gosub_with_params", Operand::Script(1)),
        op("return", Operand::Byte(0)),
    ]);
    let scripts = BTreeMap::from([(1, first), (2, second)]);
    let resources = native910::resources::infer(&scripts);
    for id in [1, 2] {
        assert!(resources[&id].array_reads.contains(&2));
        assert!(resources[&id].array_writes.contains(&2));
        assert!(resources[&id].component_contexts.contains(&true));
        assert!(resources[&id].host_commands.contains("cc_settext"));
    }
}

#[test]
fn recursive_signature_requires_base_return_and_inductive_agreement() {
    let body = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("branch_if_false", Operand::Branch(5)),
        op("push_int_local", Operand::Local(0)),
        op("gosub_with_params", Operand::Script(1)),
        op("return", Operand::Byte(0)),
        op("push_constant_string", Operand::Int(7)),
        op("return", Operand::Byte(0)),
    ]);
    let scripts = BTreeMap::from([(1, body.clone())]);
    let (arities, _) = dataflow::infer_summaries(&scripts, &ConfigTypes::empty());
    assert_eq!(
        arities[&1],
        ReturnArity::Known {
            int: 1,
            obj: 0,
            long: 0
        }
    );
    let mut inconsistent = body;
    inconsistent.code[5] = op("noopDisplayCommand", Operand::Byte(0));
    let (arities, _) =
        dataflow::infer_summaries(&BTreeMap::from([(1, inconsistent)]), &ConfigTypes::empty());
    // Recursive result agrees with void base: this is a valid void recursion.
    assert_eq!(
        arities[&1],
        ReturnArity::Known {
            int: 0,
            obj: 0,
            long: 0
        }
    );
    let ungrounded = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("gosub_with_params", Operand::Script(1)),
        op("return", Operand::Byte(0)),
    ]);
    let (arities, _) =
        dataflow::infer_summaries(&BTreeMap::from([(1, ungrounded)]), &ConfigTypes::empty());
    assert_eq!(arities[&1], ReturnArity::Unknown);
}

#[test]
fn recursive_stack_growth_is_not_a_verified_signature() {
    let body = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("branch_if_false", Operand::Branch(6)),
        op("push_int_local", Operand::Local(0)),
        op("gosub_with_params", Operand::Script(1)),
        op("push_constant_string", Operand::Int(1)),
        op("return", Operand::Byte(0)),
        op("push_constant_string", Operand::Int(7)),
        op("return", Operand::Byte(0)),
    ]);
    let (arities, _) =
        dataflow::infer_summaries(&BTreeMap::from([(1, body)]), &ConfigTypes::empty());
    assert_eq!(arities[&1], ReturnArity::Unknown);
}

#[test]
fn explicit_traps_and_nonreturning_calls_have_no_fabricated_return_count() {
    let mut abort = script(vec![op("retail_trap_79", Operand::Byte(0))]);
    abort.args.int = 0;
    let mut main = script(vec![
        op("gosub_with_params", Operand::Script(2)),
        op("unknown", Operand::Byte(0)),
    ]);
    main.args.int = 0;
    let scripts = BTreeMap::from([(1, main.clone()), (2, abort)]);
    let (arities, values) = dataflow::infer_summaries(&scripts, &ConfigTypes::empty());
    assert_eq!(arities[&1], ReturnArity::Never);
    assert_eq!(arities[&2], ReturnArity::Never);
    let result =
        dataflow::analyze_with_values(&main, &scripts, &arities, &values, &ConfigTypes::empty());
    assert!(result.before[1].is_none());
    assert!(result.nonreturning.contains(&0));
}

#[test]
fn terminal_branch_does_not_poison_a_normal_return_signature() {
    let body = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("branch_if_false", Operand::Branch(3)),
        op("retail_trap_103", Operand::Byte(0)),
        op("push_constant_int", Operand::Int(7)),
        op("return", Operand::Byte(0)),
    ]);
    let result = analyze(&body);
    assert_eq!(
        result.arity,
        ReturnArity::Known {
            int: 1,
            obj: 0,
            long: 0
        }
    );
    assert!(result.nonreturning.contains(&2));
}

#[test]
fn mutual_recursion_can_share_a_grounded_base_case() {
    let wrapper = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("gosub_with_params", Operand::Script(2)),
        op("return", Operand::Byte(0)),
    ]);
    let base = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("branch_if_false", Operand::Branch(5)),
        op("push_int_local", Operand::Local(0)),
        op("gosub_with_params", Operand::Script(1)),
        op("return", Operand::Byte(0)),
        op("push_constant_int", Operand::Int(7)),
        op("return", Operand::Byte(0)),
    ]);
    let (arities, _) = dataflow::infer_summaries(
        &BTreeMap::from([(1, wrapper), (2, base)]),
        &ConfigTypes::empty(),
    );
    assert_eq!(
        arities[&1],
        ReturnArity::Known {
            int: 1,
            obj: 0,
            long: 0
        }
    );
    assert_eq!(arities[&2], arities[&1]);
}

/// Call-site proofs must agree with actual VM stacks, without publishing the
/// selector-specific shape as a universal helper signature.
#[test]
fn contextual_calls_follow_exact_boolean_switch_and_signed_long_branches() {
    use native910::{
        runtime::RuntimeHost,
        script::SwitchCase,
        vm::{Session, Vm},
    };
    const MAIN: i32 = 900_020;
    const WRAPPER: i32 = MAIN + 1;
    const HELPER: i32 = MAIN + 2;
    const PAYLOAD: i32 = 77;
    let mut helper = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("branch_if_true", Operand::Branch(5)),
        op("push_int_local", Operand::Local(1)),
        op("return", Operand::Byte(0)),
        op("unknown", Operand::Byte(0)),
        op("push_int_local", Operand::Local(1)),
        op("push_constant_int", Operand::Int(0)),
        op("return", Operand::Byte(0)),
    ]);
    helper.args.int = 2;
    let mut wrapper = script(vec![
        op("push_int_local", Operand::Local(0)),
        op("push_int_local", Operand::Local(1)),
        op("gosub_with_params", Operand::Script(HELPER)),
        op("return", Operand::Byte(0)),
    ]);
    wrapper.args.int = 2;
    let configs = ConfigTypes::empty();
    for route in ["boolean", "switch", "long"] {
        let mut helper = helper.clone();
        if route == "switch" {
            helper.code[1] = op(
                "switch",
                Operand::Switch(vec![
                    SwitchCase {
                        value: 1,
                        target: 5,
                    },
                    SwitchCase {
                        value: 1,
                        target: 4,
                    }, // first matching key wins
                ]),
            );
        } else if route == "long" {
            helper.code.splice(
                0..2,
                [
                    op("push_long_local", Operand::Local(0)),
                    op("push_long_constant", Operand::Long(i64::MIN)),
                    op("long_branch_greater_than", Operand::Branch(6)),
                ],
            );
            helper.args.long = 1;
            helper.locals.long = 1;
        }
        let mut wrapper = wrapper.clone();
        if route == "long" {
            wrapper.args.long = 1;
            wrapper.locals.long = 1;
            wrapper
                .code
                .insert(2, op("push_long_local", Operand::Local(0)));
        }
        for selector in [0, 1, 2] {
            let long_value = if selector == 1 { i64::MAX } else { i64::MIN };
            let mut main = script(vec![
                op("push_constant_int", Operand::Int(selector)),
                op("push_constant_int", Operand::Int(PAYLOAD)),
                op("gosub_with_params", Operand::Script(WRAPPER)),
                op("return", Operand::Byte(0)),
            ]);
            main.args.int = 0;
            if route == "long" {
                main.code
                    .insert(2, op("push_long_constant", Operand::Long(long_value)));
            }
            let scripts = BTreeMap::from([
                (MAIN, main.clone()),
                (WRAPPER, wrapper.clone()),
                (HELPER, helper.clone()),
            ]);
            let (arities, values) = dataflow::infer_summaries(&scripts, &configs);
            assert_eq!(arities[&HELPER], ReturnArity::Unknown);
            assert_eq!(arities[&WRAPPER], ReturnArity::Unknown);
            let expected = if selector == 1 {
                vec![PAYLOAD, 0]
            } else {
                vec![PAYLOAD]
            };
            let analysis =
                dataflow::analyze_with_values(&main, &scripts, &arities, &values, &configs);
            assert_eq!(analysis.failure, None, "{route}/{selector}");
            assert_eq!(
                arities[&MAIN],
                ReturnArity::Known {
                    int: expected.len() as u16,
                    obj: 0,
                    long: 0
                }
            );
            if route != "long" {
                let symbols = native910::symbols::SymbolRegistry::build(
                    vec![(WRAPPER, "wrapped".into())],
                    &scripts
                        .iter()
                        .map(|(id, script)| (*id, script.args))
                        .collect(),
                )
                .unwrap()
                .with_call_context(
                    &scripts,
                    &configs,
                    &(arities.clone(), values.clone()),
                );
                assert_eq!(symbols.returns_of(WRAPPER), None);
                let source = format!(
                    "[clientscript,authored]()\nint $result;\n$result = ~wrapped({selector}, {PAYLOAD});\npush_int_local($result);\nreturn(0);\n"
                );
                let book = native910::opcode::OpcodeBook::embedded().unwrap();
                let authored = native910::source::assemble_source(
                    &source,
                    &book,
                    &symbols,
                    &configs,
                    &native910::inames::InterfaceRegistry::empty(),
                );
                if selector == 1 {
                    assert!(
                        authored
                            .unwrap_err()
                            .to_string()
                            .contains("exactly one value required")
                    );
                } else {
                    let authored =
                        native910::script::decode_script(&authored.unwrap(), &book).unwrap();
                    let provider: std::collections::HashMap<_, _> = scripts
                        .iter()
                        .map(|(id, script)| (*id, script.clone()))
                        .collect();
                    assert_eq!(
                        Vm::new(&mut RuntimeHost::default(), &provider)
                            .execute(&authored, &[])
                            .unwrap(),
                        Some(native910::vm::Value::Int(PAYLOAD))
                    );
                }
                assert_eq!(
                    symbols.returns_of(WRAPPER),
                    None,
                    "entry proof must not leak into generic signatures"
                );
            }
            let mut host = RuntimeHost::default();
            let provider: std::collections::HashMap<_, _> = scripts
                .iter()
                .map(|(id, script)| (*id, script.clone()))
                .collect();
            let mut vm = Vm::new(&mut host, &provider);
            let mut session = Session::new(&main, &[]).unwrap();
            while !session.finished() {
                vm.step(&mut session).unwrap();
            }
            assert_eq!(session.snapshot().ints, expected, "{route}/{selector}");
            assert_eq!(
                analysis.returned.unwrap()[0]
                    .iter()
                    .map(|v| v.constant.clone())
                    .collect::<Vec<_>>(),
                expected
                    .into_iter()
                    .map(|value| Some(Constant::Int(value)))
                    .collect::<Vec<_>>()
            );
            let entry = [
                vec![Some(Constant::Int(selector)), Some(Constant::Int(PAYLOAD))],
                vec![],
                if route == "long" {
                    vec![Some(Constant::Long(long_value))]
                } else {
                    vec![]
                },
            ];
            assert_eq!(
                dataflow::analyze_in_context(
                    &helper, &scripts, &arities, &values, &configs, &entry
                )
                .arity,
                arities[&MAIN]
            );
            let bad_entry = [vec![Some(Constant::Null)], vec![], vec![]];
            assert_eq!(
                dataflow::analyze_in_context(
                    &helper, &scripts, &arities, &values, &configs, &bad_entry
                )
                .failure,
                Some((0, FailureKind::InvalidOperand))
            );
        }
    }
}
