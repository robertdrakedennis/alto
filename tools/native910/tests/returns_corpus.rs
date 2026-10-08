//! Return-arity gates: hand-verified unit models of the legacy
//! `returns::infer_returns` fixed point.

use native910::returns::{ReturnArity, infer_returns};
use native910::script::{CompiledScript, Counts, Instruction, Operand, SwitchCase};
use std::collections::BTreeMap;

fn instr(command: &str, operand: Operand) -> Instruction {
    Instruction {
        opcode: 0,
        command: command.to_string(),
        operand,
    }
}

fn plain(code: Vec<Instruction>) -> CompiledScript {
    CompiledScript {
        name: None,
        locals: Counts {
            int: 0,
            obj: 0,
            long: 0,
        },
        args: Counts {
            int: 0,
            obj: 0,
            long: 0,
        },
        code,
    }
}

fn with_args(code: Vec<Instruction>, args: Counts) -> CompiledScript {
    CompiledScript {
        name: None,
        locals: Counts {
            int: 0,
            obj: 0,
            long: 0,
        },
        args,
        code,
    }
}

fn push_int(value: i32) -> Instruction {
    instr("push_constant_int", Operand::Int(value))
}

fn ret() -> Instruction {
    instr("return", Operand::Byte(0))
}

fn tally(entries: Vec<(i32, CompiledScript)>) -> (BTreeMap<i32, ReturnArity>, bool) {
    let table: BTreeMap<i32, CompiledScript> = entries.into_iter().collect();
    infer_returns(&table)
}

fn lookup(outcome: &(BTreeMap<i32, ReturnArity>, bool), id: i32) -> ReturnArity {
    outcome.0.get(&id).copied().unwrap_or(ReturnArity::Unknown)
}

const VOID: ReturnArity = ReturnArity::Known {
    int: 0,
    obj: 0,
    long: 0,
};

#[test]
fn empty_script_has_no_valid_return() {
    let outcome = tally(vec![(7, plain(vec![]))]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 7), ReturnArity::Unknown);
}

#[test]
fn lone_return_returns_void() {
    let outcome = tally(vec![(7, plain(vec![ret()]))]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 7), VOID);
}

#[test]
fn push_then_return_leaves_one_int() {
    let outcome = tally(vec![(7, plain(vec![push_int(1), ret()]))]);
    assert!(outcome.1);
    assert_eq!(
        lookup(&outcome, 7),
        ReturnArity::Known {
            int: 1,
            obj: 0,
            long: 0
        }
    );
}

#[test]
fn agreeing_branches_stay_known() {
    // Both fork paths record (2, 0, 0) so the script is Known. Note the
    // model: branch conditions consume their boolean on both paths,
    // leaving only the selected result.
    let outcome = tally(vec![(
        7,
        plain(vec![
            push_int(1),
            instr("branch_if_false", Operand::Branch(4)),
            push_int(2),
            ret(),
            push_int(3),
            ret(),
        ]),
    )]);
    assert!(outcome.1);
    assert_eq!(
        lookup(&outcome, 7),
        ReturnArity::Known {
            int: 1,
            obj: 0,
            long: 0
        }
    );
}

#[test]
fn disagreeing_branches_are_unknown() {
    // Fallthrough records (2, 0, 0); the target pops to (0, 0, 0). One
    // divergent return poisons the whole script: soundness over coverage.
    let outcome = tally(vec![(
        7,
        plain(vec![
            push_int(1),
            instr("branch_if_false", Operand::Branch(4)),
            push_int(2),
            ret(),
            instr("pop_int_local", Operand::Local(0)),
            ret(),
        ]),
    )]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 7), ReturnArity::Unknown);
}

#[test]
fn gosub_chain_resolves_through_fixed_point() {
    // Pass 1 grounds only script 3 (no calls); pass 2 grounds 2 against 3;
    // pass 3 grounds 1 against 2. A single pass could never resolve the
    // chain, so all-Known proves the fixed-point iterates.
    let outcome = tally(vec![
        (
            1,
            plain(vec![instr("gosub_with_params", Operand::Script(2)), ret()]),
        ),
        (
            2,
            plain(vec![instr("gosub_with_params", Operand::Script(3)), ret()]),
        ),
        (3, plain(vec![push_int(9), ret()])),
    ]);
    assert!(outcome.1);
    let one = ReturnArity::Known {
        int: 1,
        obj: 0,
        long: 0,
    };
    assert_eq!(lookup(&outcome, 3), one);
    assert_eq!(lookup(&outcome, 2), one);
    assert_eq!(lookup(&outcome, 1), one);
}

#[test]
fn direct_recursion_stays_unknown() {
    // The call pushes the callee's CURRENT map state, which starts Unknown
    // and is never grounded by a cycle — so the path is Lost on every pass
    // and the monotone rule keeps the script Unknown. Terminates via the
    // first no-change pass (converged), not via the pass cap.
    let outcome = tally(vec![(
        1,
        plain(vec![instr("gosub_with_params", Operand::Script(1)), ret()]),
    )]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 1), ReturnArity::Unknown);
}

#[test]
fn mutual_recursion_stays_unknown() {
    // Same grounding argument as direct recursion, across two scripts: 1
    // waits on 2 while 2 waits on 1, so neither ever upgrades.
    let outcome = tally(vec![
        (
            1,
            plain(vec![instr("gosub_with_params", Operand::Script(2)), ret()]),
        ),
        (
            2,
            plain(vec![instr("gosub_with_params", Operand::Script(1)), ret()]),
        ),
    ]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 1), ReturnArity::Unknown);
    assert_eq!(lookup(&outcome, 2), ReturnArity::Unknown);
}

#[test]
fn unknown_command_is_unknown() {
    // A bare `join_string` with nothing pushed: its contract row is
    // `Pure { [0, 2, 0], [0, 1, 0] }`, so the two object pops underflow the
    // empty lane — the path, and hence the script, is Unknown.
    let outcome = tally(vec![(
        7,
        plain(vec![instr("join_string", Operand::Count(2)), ret()]),
    )]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 7), ReturnArity::Unknown);
}

#[test]
fn stack_underflow_is_unknown() {
    // add pops two ints from an empty lane: underflow marks the path
    // Unknown, never an error.
    let outcome = tally(vec![(
        7,
        plain(vec![instr("add", Operand::Byte(0)), ret()]),
    )]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 7), ReturnArity::Unknown);
}

#[test]
fn unreachable_code_never_poisons() {
    // branch is unconditional (target only), so index 2 — an add that would
    // underflow — is unreachable and ignored.
    let outcome = tally(vec![(
        7,
        plain(vec![
            push_int(1),
            instr("branch", Operand::Branch(3)),
            instr("add", Operand::Byte(0)),
            ret(),
        ]),
    )]);
    assert!(outcome.1);
    assert_eq!(
        lookup(&outcome, 7),
        ReturnArity::Known {
            int: 1,
            obj: 0,
            long: 0
        }
    );
}

#[test]
fn gosub_to_missing_callee_is_unknown() {
    // Callee id 4242 is absent from the map: nothing to pop or push
    // against, so the calling path is Unknown.
    let outcome = tally(vec![(
        7,
        plain(vec![
            instr("gosub_with_params", Operand::Script(4242)),
            ret(),
        ]),
    )]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 7), ReturnArity::Unknown);
}

#[test]
fn gosub_pops_declared_args() {
    // Callee 2 declares one int arg but never touches the stacks (args
    // arrive in slots): Known void. Caller 1 pushes the arg, the call pops
    // it and pushes nothing: Known void. Caller 3 pushes nothing, so the
    // declared-arg pop underflows: Unknown.
    let callee = with_args(
        vec![ret()],
        Counts {
            int: 1,
            obj: 0,
            long: 0,
        },
    );
    let outcome = tally(vec![
        (
            1,
            plain(vec![
                push_int(5),
                instr("gosub_with_params", Operand::Script(2)),
                ret(),
            ]),
        ),
        (
            3,
            plain(vec![instr("gosub_with_params", Operand::Script(2)), ret()]),
        ),
        (2, callee),
    ]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 2), VOID);
    assert_eq!(lookup(&outcome, 1), VOID);
    assert_eq!(lookup(&outcome, 3), ReturnArity::Unknown);
}

#[test]
fn callee_popping_its_own_args_is_unknown() {
    // Entry depths are (0, 0, 0) — argument values arrive in slots, not on
    // stacks — so popping the declared arg underflows: Unknown.
    let outcome = tally(vec![(
        2,
        with_args(
            vec![instr("pop_int_local", Operand::Local(0)), ret()],
            Counts {
                int: 1,
                obj: 0,
                long: 0,
            },
        ),
    )]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 2), ReturnArity::Unknown);
}

#[test]
fn typed_stacks_track_each_lane() {
    // One push per lane records (1, 1, 1); popping the object lane back off
    // records (1, 0, 0). Exercises the obj/long lanes and the u16 answer.
    let outcome = tally(vec![
        (
            7,
            plain(vec![
                push_int(1),
                instr("push_constant_string", Operand::Str("x".to_string())),
                instr("push_long_constant", Operand::Long(9)),
                ret(),
            ]),
        ),
        (
            8,
            plain(vec![
                push_int(1),
                instr("push_constant_string", Operand::Str("x".to_string())),
                instr("pop_string_local", Operand::Local(0)),
                ret(),
            ]),
        ),
    ]);
    assert!(outcome.1);
    assert_eq!(
        lookup(&outcome, 7),
        ReturnArity::Known {
            int: 1,
            obj: 1,
            long: 1
        }
    );
    assert_eq!(
        lookup(&outcome, 8),
        ReturnArity::Known {
            int: 1,
            obj: 0,
            long: 0
        }
    );
}

/// `switch` jumps by the case
/// offset on a match and otherwise just continues with `pc + 1`: both the
/// case target AND the fall-through are live successors. Distinct targets
/// with different return arities make the fall-through edge observable.
#[test]
fn switch_falls_through_and_joins() {
    let case = |target| {
        instr(
            "switch",
            Operand::Switch(vec![SwitchCase { value: 0, target }]),
        )
    };
    // Fall-through (index 2) returns two ints, the case (index 5) one int:
    // the paths disagree, so the arity is Unknown. Dropping the fall-through
    // edge would leave only the case path and claim Known { int: 1 }.
    let disagreeing = tally(vec![(
        7,
        plain(vec![
            push_int(7),
            case(5),
            push_int(8),
            push_int(9),
            ret(),
            push_int(10),
            ret(),
        ]),
    )]);
    assert!(disagreeing.1);
    assert_eq!(lookup(&disagreeing, 7), ReturnArity::Unknown);
    // Dropping the case edge instead would claim the fall-through's shape.
    let case_only_differs = tally(vec![(
        8,
        plain(vec![push_int(7), case(3), ret(), push_int(10), ret()]),
    )]);
    assert_eq!(lookup(&case_only_differs, 8), ReturnArity::Unknown);
    // Agreeing distinct paths join to the common shape.
    let agreeing = tally(vec![(
        9,
        plain(vec![
            push_int(7),
            case(4),
            push_int(8),
            ret(),
            push_int(10),
            ret(),
        ]),
    )]);
    assert_eq!(
        lookup(&agreeing, 9),
        ReturnArity::Known {
            int: 1,
            obj: 0,
            long: 0
        }
    );
}

#[test]
fn branch_to_end_is_invalid_execution() {
    let outcome = tally(vec![(
        7,
        plain(vec![push_int(4), instr("branch", Operand::Branch(2))]),
    )]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 7), ReturnArity::Unknown);
}

#[test]
fn loop_without_exit_is_unknown() {
    // The only reachable path loops forever with no recorded end. Zero
    // recorded ends is Unknown — vacuous agreement would overclaim.
    let outcome = tally(vec![(7, plain(vec![instr("branch", Operand::Branch(0))]))]);
    assert!(outcome.1);
    assert_eq!(lookup(&outcome, 7), ReturnArity::Unknown);
}
