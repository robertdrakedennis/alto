//! Static return-arity inference for 910 scripts.
//!
//! 910 scripts declare argument counts but NOT return counts: a call leaves
//! whatever the callee pushed, so any future `x = ~f()` or call-result
//! folding needs this map. [`infer_returns`] recovers it for well-structured
//! scripts and answers [`ReturnArity::Unknown`] for everything else.
//!
//! Method: per-script forward simulation of typed `(int, obj, long)` stack
//! depths over the reachable control-flow graph (branch/switch targets are
//! absolute instruction indices; targets past the final instruction are invalid), lifted to the whole corpus by a monotone fixed-point over
//! the `gosub_with_params` call graph. The fixed-point starts every script at
//! Unknown, re-infers each script against the current map, upgrades
//! Unknown-to-Known only, and caps passes at `scripts.len() + 1`, so
//! termination is guaranteed and (direct or indirect) recursion settles at
//! Unknown. The pass reports whether it converged.
//!
//! `Unknown` is a first-class, documented answer — never an error, never a
//! panic. In particular, inference does NOT cover:
//!
//! * Operand-dependent or unlisted commands: anything the `effect()` table
//!   reports as `Unknown` (`push_var`/`pop_var`, interface commands, ...)
//!   makes a reachable path — and hence its script — Unknown. (`join_string`
//!   states a per-count `Fixed` row, so only a genuinely starved lane —
//!   underflow — reports Unknown there.)
//! * Divergent exits: every recorded `return` depth must
//!   agree EXACTLY. One divergent `return` poisons the whole script, as does
//!   one join where two paths arrive with different depths (stack traffic is
//!   depth-affine, so a mid-path difference can only surface as divergent
//!   exits or as underflow — both Unknown).
//! * Stack underflow: a pop with nothing tracked on that lane ends that path
//!   as Unknown. Entry depths are always `(0, 0, 0)` because argument values
//!   arrive in slots, not on stacks — so a callee that pops its own declared
//!   arguments is Unknown, while callers still pop the callee's declared
//!   argument counts before pushing its inferred returns.
//! * Calls whose callee id is absent from the input map, or whose callee is
//!   itself Unknown. A cycle never grounds (the map starts all-Unknown), so
//!   every recursive script stays Unknown.
//! * Conditional branches and switches consume their operands on every edge.
//! * Scripts with no analyzable exit (for example a provable infinite loop):
//!   zero recorded ends is Unknown, since vacuous agreement would overclaim.
//!
//! This lightweight depth-only API remains available for callers without configs.
//! Project builds and source registries use `dataflow::infer_returns`, which also
//! resolves reaching values and config-dependent traffic.

use crate::effects::{Effect, effect};
use crate::script::{CompiledScript, Instruction, Operand, is_branch_command};
use std::collections::{BTreeMap, VecDeque};

/// A script's statically known return arity: the `(int, obj, long)` stack
/// depths it leaves behind on every exit, or [`ReturnArity::Unknown`] when
/// static analysis cannot pin them down exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnArity {
    /// No normal return is possible on any fully analyzed path.
    Never,
    /// Every reachable exit path leaves exactly these stack depths.
    Known {
        /// Int values left on the int stack.
        int: u16,
        /// Object (string) values left on the object stack.
        obj: u16,
        /// Long values left on the long stack.
        long: u16,
    },
    /// Unknowable statically: divergent exits, operand-dependent commands,
    /// underflow, unresolved calls, recursion, or no analyzable exit. A
    /// first-class answer, never an error.
    Unknown,
}

/// Infer every script's return arity by fixed-point over the call graph.
///
/// Keys are callee (group) ids, matching the input map. Starts all Unknown;
/// each pass re-infers every still-Unknown script against the current map and
/// applies Unknown-to-Known upgrades only, so results are monotone. Passes
/// are capped at `scripts.len() + 1`, which always suffices (each
/// non-converged pass grounds at least one script, and there are only
/// `scripts.len()` of them) while guaranteeing termination regardless. The
/// flag reports whether a pass completed with no changes.
#[must_use]
pub fn infer_returns(
    scripts: &BTreeMap<i32, CompiledScript>,
) -> (BTreeMap<i32, ReturnArity>, bool) {
    let mut current: BTreeMap<i32, ReturnArity> = scripts
        .keys()
        .map(|callee| (*callee, ReturnArity::Unknown))
        .collect();
    let cap = scripts.len().saturating_add(1);
    for _ in 0..cap {
        let mut fresh = Vec::new();
        for (callee, script) in scripts {
            if !matches!(current.get(callee), Some(ReturnArity::Unknown)) {
                continue;
            }
            if let ReturnArity::Known { int, obj, long } = infer_one(script, scripts, &current) {
                fresh.push((*callee, ReturnArity::Known { int, obj, long }));
            }
        }
        if fresh.is_empty() {
            return (current, true);
        }
        for (callee, arity) in fresh {
            current.insert(callee, arity);
        }
    }
    (current, false)
}

/// Why a reachable script path could not be verified.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum FailureKind {
    UnknownCommand,
    UnresolvedType,
    RuntimeDependent,
    InvalidControlFlow,
    StackUnderflow,
    DepthOverflow,
    MissingCallee,
    UnknownCalleeReturn,
    InvalidOperand,
    IncompatibleMerge,
    DivergentExit,
    NoExit,
}

/// Analysis diagnostics retain the first obstructing instruction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Analysis {
    pub arity: ReturnArity,
    pub failure: Option<(usize, FailureKind)>,
}
impl Analysis {
    fn unknown(pc: usize, reason: FailureKind) -> Self {
        Self {
            arity: ReturnArity::Unknown,
            failure: Some((pc, reason)),
        }
    }
}

fn infer_one(
    script: &CompiledScript,
    scripts: &BTreeMap<i32, CompiledScript>,
    current: &BTreeMap<i32, ReturnArity>,
) -> ReturnArity {
    analyze(script, scripts, current).arity
}

/// Verify reachable typed stack depths against the current call summaries.
pub fn analyze(
    script: &CompiledScript,
    scripts: &BTreeMap<i32, CompiledScript>,
    current: &BTreeMap<i32, ReturnArity>,
) -> Analysis {
    let code = script.code.as_slice();
    if code.is_empty() {
        // No instruction fetch or valid return is possible.
        return Analysis::unknown(0, FailureKind::NoExit);
    }
    let live = reachable(code);
    let mut seen: Vec<Option<[u32; 3]>> = vec![None; code.len()];
    let mut ends: Option<[u32; 3]> = None;
    let mut queue = VecDeque::from([0_usize]);
    seen[0] = Some([0, 0, 0]);
    while let Some(index) = queue.pop_front() {
        if !live[index] {
            continue;
        }
        let Some(mut depths) = seen[index] else {
            continue;
        };
        match advance(index, &mut depths, code, scripts, current) {
            Advance::Lost(reason) => return Analysis::unknown(index, reason),
            Advance::Flow { next, exits } => {
                if exits {
                    match ends {
                        None => ends = Some(depths),
                        Some(prior) if prior == depths => {}
                        Some(_) => return Analysis::unknown(index, FailureKind::DivergentExit),
                    }
                }
                for place in next {
                    match seen[place] {
                        None => {
                            seen[place] = Some(depths);
                            queue.push_back(place);
                        }
                        Some(prior) if prior == depths => {}
                        Some(_) => return Analysis::unknown(place, FailureKind::IncompatibleMerge),
                    }
                }
            }
        }
    }
    let Some(depths) = ends else {
        // No analyzable exit (for example a loop that never returns).
        return Analysis::unknown(0, FailureKind::NoExit);
    };
    let [ints, objs, longs] = depths;
    let (Ok(int), Ok(obj), Ok(long)) = (
        u16::try_from(ints),
        u16::try_from(objs),
        u16::try_from(longs),
    ) else {
        return Analysis::unknown(0, FailureKind::DepthOverflow);
    };
    Analysis {
        arity: ReturnArity::Known { int, obj, long },
        failure: None,
    }
}

/// Outcome of executing one instruction abstractly.
enum Advance {
    /// The path continues at these indices, and additionally exits here when
    /// `exits` (a `return`, or a route to `code.len()`).
    Flow {
        /// Instruction indices the path continues at.
        next: Vec<usize>,
        /// Whether the path also records its depths as an exit here.
        exits: bool,
    },
    /// The path cannot be tracked (underflow, unknown command, unresolved
    /// call, unresolvable control flow). Poisons the whole script to Unknown.
    Lost(FailureKind),
}

impl Advance {
    /// Route an [`Edges`] set into an outcome: lost routes poison the path.
    fn from_edges(edges: Edges) -> Self {
        if edges.lost {
            Self::Lost(FailureKind::InvalidControlFlow)
        } else {
            Self::Flow {
                next: edges.next,
                exits: edges.exit,
            }
        }
    }
}

/// How one instruction routes control, before stack-depth bookkeeping.
pub(crate) struct Edges {
    /// Instruction indices execution may continue at.
    pub(crate) next: Vec<usize>,
    /// Whether some route leaves the script here (record exit depths).
    pub(crate) exit: bool,
    /// Whether some route cannot be resolved (poisons the path).
    pub(crate) lost: bool,
}

impl Edges {
    /// A single in-code successor.
    fn step(place: usize) -> Self {
        Self {
            next: vec![place],
            exit: false,
            lost: false,
        }
    }

    /// A route that leaves the script (fall off the end).
    fn exit() -> Self {
        Self {
            next: Vec::new(),
            exit: true,
            lost: false,
        }
    }

    /// A route that cannot be resolved.
    fn lost() -> Self {
        Self {
            next: Vec::new(),
            exit: false,
            lost: true,
        }
    }

    /// Union another route set in; a lost input poisons the whole set.
    fn absorb(&mut self, other: Self) {
        if self.lost || other.lost {
            *self = Self::lost();
        } else {
            self.next.extend(other.next);
            self.exit = self.exit || other.exit;
        }
    }
}

/// One absolute jump target, classified. Targets outside `0..code.len()`
/// are untrackable (decode rejects
/// such targets, so only hand-built models can carry them).
enum Edge {
    /// Continue at this instruction.
    Code(usize),
    /// Out of range: unresolvable.
    Broken,
}

/// Classify one absolute jump target.
fn edge(target: i32, len: usize) -> Edge {
    let Ok(place) = usize::try_from(target) else {
        return Edge::Broken;
    };
    match place.cmp(&len) {
        std::cmp::Ordering::Less => Edge::Code(place),
        std::cmp::Ordering::Equal => Edge::Broken,
        std::cmp::Ordering::Greater => Edge::Broken,
    }
}

/// The fallthrough route past `index`: the next instruction, or an invalid edge
/// when `index` is the last one.
fn fallthrough(index: usize, len: usize) -> Edges {
    match index.checked_add(1) {
        Some(next) if next < len => Edges::step(next),
        _ => Edges::lost(),
    }
}

/// Route one absolute target into an [`Edges`] set.
fn edge_routes(target: i32, len: usize) -> Edges {
    match edge(target, len) {
        Edge::Code(place) => Edges::step(place),
        Edge::Broken => Edges::lost(),
    }
}

/// Control-flow routing for `return` / branches / `switch`. Anything else
/// carrying a `ControlFlow` effect is unresolvable and lost.
fn control_flow(command: &str, operand: &Operand, index: usize, len: usize) -> Edges {
    if command == "return" {
        return Edges::exit();
    }
    if command == "branch" {
        // Unconditional: the target is the only route, never fallthrough.
        let Operand::Branch(target) = operand else {
            return Edges::lost();
        };
        return edge_routes(*target, len);
    }
    if command == "switch" {
        let Operand::Switch(cases) = operand else {
            return Edges::lost();
        };
        let mut routes = Edges {
            next: Vec::with_capacity(cases.len().saturating_add(1)),
            exit: false,
            lost: false,
        };
        for case in cases {
            routes.absorb(edge_routes(case.target, len));
        }
        // A switch falls through past itself too.
        routes.absorb(fallthrough(index, len));
        return routes;
    }
    if is_branch_command(command) {
        // Conditional branch: target edge plus fallthrough.
        let Operand::Branch(target) = operand else {
            return Edges::lost();
        };
        let mut routes = edge_routes(*target, len);
        routes.absorb(fallthrough(index, len));
        return routes;
    }
    Edges::lost()
}

/// Full routing for any instruction: straight-line effects fall through,
/// control flow routes per [`control_flow`].
pub(crate) fn flow(instr: &Instruction, index: usize, len: usize) -> Edges {
    match effect(&instr.command, &instr.operand) {
        Effect::Trap => Edges {
            next: Vec::new(),
            exit: false,
            lost: false,
        },
        Effect::Fixed { .. } | Effect::Call | Effect::Unknown | Effect::Conditional { .. } => {
            fallthrough(index, len)
        }
        Effect::ControlFlow { .. } => control_flow(&instr.command, &instr.operand, index, len),
    }
}

/// Instructions reachable from 0 via fallthrough, branch targets, and switch
/// case targets (plus fallthrough past switch). Unreachable code is ignored
/// by inference and never poisons.
fn reachable(code: &[Instruction]) -> Vec<bool> {
    let mut live = vec![false; code.len()];
    if code.is_empty() {
        return live;
    }
    let mut queue = VecDeque::from([0_usize]);
    live[0] = true;
    while let Some(index) = queue.pop_front() {
        let Some(instr) = code.get(index) else {
            continue;
        };
        for place in flow(instr, index, code.len()).next {
            if !live[place] {
                live[place] = true;
                queue.push_back(place);
            }
        }
    }
    live
}

/// Execute one reachable instruction abstractly: update `depths` per its
/// effect and report where the path goes. Any untrackable step is
/// [`Advance::Lost`] — inference-wide Unknown, never an error, never a panic.
fn advance(
    index: usize,
    depths: &mut [u32; 3],
    code: &[Instruction],
    scripts: &BTreeMap<i32, CompiledScript>,
    current: &BTreeMap<i32, ReturnArity>,
) -> Advance {
    let Some(instr) = code.get(index) else {
        return Advance::Lost(FailureKind::InvalidControlFlow);
    };
    match effect(&instr.command, &instr.operand) {
        Effect::Fixed { pops, pushes } => {
            for (lane, delta) in depths.iter_mut().zip(pops.iter().zip(pushes.iter())) {
                let (take, give) = delta;
                let Some(after_take) = lane.checked_sub(u32::from(*take)) else {
                    // Underflow: this path needs more than is tracked.
                    return Advance::Lost(FailureKind::StackUnderflow);
                };
                let Some(after_give) = after_take.checked_add(u32::from(*give)) else {
                    return Advance::Lost(FailureKind::DepthOverflow);
                };
                *lane = after_give;
            }
            Advance::from_edges(fallthrough(index, code.len()))
        }
        Effect::Call => {
            let (Operand::Script(id) | Operand::Int(id)) = &instr.operand else {
                return Advance::Lost(FailureKind::InvalidOperand);
            };
            let target = *id;
            let Some(callee) = scripts.get(&target) else {
                // Unknown callee id: nothing to pop or push against.
                return Advance::Lost(FailureKind::MissingCallee);
            };
            let wanted = [callee.args.int, callee.args.obj, callee.args.long];
            for (lane, take) in depths.iter_mut().zip(wanted.iter()) {
                let Some(left) = lane.checked_sub(u32::from(*take)) else {
                    return Advance::Lost(FailureKind::StackUnderflow);
                };
                *lane = left;
            }
            let Some(known) = current.get(&target) else {
                return Advance::Lost(FailureKind::UnknownCalleeReturn);
            };
            match *known {
                ReturnArity::Known { int, obj, long } => {
                    let back = [int, obj, long];
                    for (lane, give) in depths.iter_mut().zip(back.iter()) {
                        let Some(after) = lane.checked_add(u32::from(*give)) else {
                            return Advance::Lost(FailureKind::DepthOverflow);
                        };
                        *lane = after;
                    }
                    Advance::from_edges(fallthrough(index, code.len()))
                }
                // The callee's arity is not grounded yet: this path waits for
                // a later fixed-point pass (or forever, for recursion).
                ReturnArity::Never => Advance::Flow {
                    next: Vec::new(),
                    exits: false,
                },
                ReturnArity::Unknown => Advance::Lost(FailureKind::UnknownCalleeReturn),
            }
        }
        Effect::ControlFlow { pops } => {
            for (lane, take) in depths.iter_mut().zip(pops) {
                let Some(left) = lane.checked_sub(u32::from(take)) else {
                    return Advance::Lost(FailureKind::StackUnderflow);
                };
                *lane = left;
            }
            Advance::from_edges(control_flow(
                &instr.command,
                &instr.operand,
                index,
                code.len(),
            ))
        }
        Effect::Trap => Advance::Lost(FailureKind::NoExit),
        Effect::Conditional { .. } => Advance::Lost(FailureKind::RuntimeDependent),
        Effect::Unknown => Advance::Lost(FailureKind::UnknownCommand),
    }
}
