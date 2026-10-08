//! Forward dataflow over typed stacks and locals. Constants survive a join only
//! when every predecessor agrees. Definitions are finite instruction sets, so
//! loops converge without guessing a value or unrolling a path. Calls consume
//! declared arguments and use verified return summaries; host results are opaque.
use crate::config::{ConfigTypes, VarValueKind};
use crate::returns::{FailureKind, ReturnArity};
use crate::script::{CompiledScript, Instruction, Operand};
use crate::semantics::{Effect, effect};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Initial values and hook traffic are properties of the analyzed VM.
/// Defaults retain the ordinary runtime analysis contract.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LocalDefaults {
    #[default]
    Initialized,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DescriptorUnits {
    #[default]
    Utf16,
    Utf8Bytes,
}

impl DescriptorUnits {
    #[must_use]
    pub fn units(self, descriptor: &str) -> Vec<u16> {
        match self {
            Self::Utf16 => crate::jstr::units(descriptor),
            Self::Utf8Bytes => descriptor.bytes().map(u16::from).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PositiveTriggers {
    #[default]
    TransmitList,
    Trap,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HookCodec {
    pub descriptor_units: DescriptorUnits,
    pub positive_triggers: PositiveTriggers,
    /// A nonempty retained list removes the descriptor's last unit even when
    /// a replacement has no suffix. Unknown retained state means unknown traffic.
    pub retained_triggers: bool,
}

impl HookCodec {
    /// Resolve descriptor traffic only with the retained state it depends on.
    /// `None` for retained state is deliberately different from an empty list.
    #[must_use]
    pub fn signature_units(
        self,
        descriptor: &str,
        count: Option<i32>,
        retained_nonempty: Option<bool>,
    ) -> Option<Vec<u16>> {
        let positive = if descriptor.ends_with('Y') {
            count? > i32::default()
        } else {
            false
        };
        if positive && self.positive_triggers == PositiveTriggers::Trap {
            return None;
        }
        let strip = if self.retained_triggers {
            positive || retained_nonempty?
        } else {
            positive
        };
        let mut units = self.descriptor_units.units(descriptor);
        if strip {
            units.pop();
        }
        Some(units)
    }
}

#[derive(Clone, Debug, Default)]
pub struct AnalysisOptions {
    pub local_defaults: LocalDefaults,
    pub hooks: BTreeMap<u16, HookCodec>,
    /// Reviewed source traffic overrides used only by private projections.
    pub traffic: BTreeMap<u16, Effect>,
    /// Exact imported metadata signature to instruction traffic.
    pub bound_traffic: BTreeMap<String, BTreeMap<usize, TypedTraffic>>,
    /// Source schema traffic indexed by opcode and the reaching packed field.
    pub database_fields: BTreeMap<u16, BTreeMap<i32, TypedTraffic>>,
    /// Source enum traffic uses serial type IDs, independent of target types.
    pub enum_string_types: BTreeMap<u16, u16>,
}

const ENUM_ARGUMENT_COUNT: u16 = 4;
const ENUM_OUTPUT_TYPE_DEPTH: usize = 2;
const INTEGER_OUTPUT_LANE: usize = 0;
const OBJECT_OUTPUT_LANE: usize = 1;
const ONE_OUTPUT: u16 = 1;

impl AnalysisOptions {
    fn enum_traffic(&self, instruction: &Instruction, state: &State) -> Option<TypedTraffic> {
        let string_type = self.enum_string_types.get(&instruction.opcode)?;
        let output_type = state.int_from_top(ENUM_OUTPUT_TYPE_DEPTH)?;
        let string = output_type == i32::from(*string_type);
        let mut pushes = [u16::default(); 3];
        pushes[if string {
            OBJECT_OUTPUT_LANE
        } else {
            INTEGER_OUTPUT_LANE
        }] = ONE_OUTPUT;
        Some(TypedTraffic {
            effect: Effect::Fixed {
                pops: [ENUM_ARGUMENT_COUNT, 0, 0],
                pushes,
            },
            nonnull_strings: if string { vec![true] } else { Vec::new() },
        })
    }
}

#[derive(Clone, Debug)]
pub struct TypedTraffic {
    pub effect: Effect,
    /// Object output order, distinct from the tuple's interleaved lane order.
    pub nonnull_strings: Vec<bool>,
}

/// Select every rostered call dependency, including recursive cycles. Missing
/// callees remain in instruction operands for the analyzer to diagnose.
pub(crate) fn dependency_closure(
    scripts: &BTreeMap<i32, CompiledScript>,
    roots: impl Iterator<Item = i32>,
) -> BTreeMap<i32, CompiledScript> {
    let mut selected = BTreeMap::new();
    let mut queue: Vec<_> = roots.collect();
    while let Some(id) = queue.pop() {
        if selected.contains_key(&id) {
            continue;
        }
        let Some(script) = scripts.get(&id) else {
            continue;
        };
        for instruction in &script.code {
            if instruction.command == "gosub_with_params"
                && let Operand::Script(target) | Operand::Int(target) = instruction.operand
            {
                queue.push(target);
            }
        }
        selected.insert(id, script.clone());
    }
    selected
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Constant {
    Int(i32),
    String(String),
    Long(i64),
    Null,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Value {
    pub constant: Option<Constant>,
    /// True only when every reaching value is a non-null string object.
    pub nonnull_string: bool,
    /// Entry argument identity, retained through copies and agreeing joins.
    pub argument: Option<(usize, u16)>,
    /// PCs that may have produced this value; empty for entry locals.
    pub definitions: BTreeSet<usize>,
}
impl Value {
    fn entry(constant: Option<Constant>) -> Self {
        Self {
            nonnull_string: matches!(constant, Some(Constant::String(_))),
            constant,
            argument: None,
            definitions: BTreeSet::new(),
        }
    }
    fn produced(pc: usize, constant: Option<Constant>) -> Self {
        Self {
            nonnull_string: matches!(constant, Some(Constant::String(_))),
            constant,
            argument: None,
            definitions: BTreeSet::from([pc]),
        }
    }
    fn merge(&mut self, other: &Self) {
        self.nonnull_string &= other.nonnull_string;
        if self.constant != other.constant {
            self.constant = None;
        }
        if self.argument != other.argument {
            self.argument = None;
        }
        self.definitions.extend(&other.definitions);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    pub stacks: [Vec<Value>; 3],
    pub locals: [Vec<Value>; 3],
}
impl State {
    fn merge(&mut self, other: &Self) -> Result<bool, FailureKind> {
        if self
            .stacks
            .iter()
            .zip(&other.stacks)
            .any(|(a, b)| a.len() != b.len())
        {
            return Err(FailureKind::IncompatibleMerge);
        }
        let before = self.clone();
        for (left, right) in self
            .stacks
            .iter_mut()
            .chain(&mut self.locals)
            .zip(other.stacks.iter().chain(&other.locals))
        {
            for (a, b) in left.iter_mut().zip(right) {
                a.merge(b);
            }
        }
        Ok(*self != before)
    }
    pub fn int_from_top(&self, offset: usize) -> Option<i32> {
        let index = self.stacks[0].len().checked_sub(offset + 1)?;
        match self.stacks[0][index].constant {
            Some(Constant::Int(n)) => Some(n),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Analysis {
    /// Entry state for each reachable instruction, including an obstructing PC.
    pub before: Vec<Option<State>>,
    pub arity: ReturnArity,
    pub failure: Option<(usize, FailureKind)>,
    pub returned: Option<[Vec<Value>; 3]>,
    /// Explicit retail traps and calls proven not to return normally.
    pub nonreturning: BTreeSet<usize>,
}

impl Analysis {
    /// Reaching edges use the same constant predicates as stack analysis.
    pub fn successors(&self, script: &CompiledScript, pc: usize) -> Vec<usize> {
        if self.nonreturning.contains(&pc) {
            return Vec::new();
        }
        self.before
            .get(pc)
            .and_then(Option::as_ref)
            .map_or_else(Vec::new, |state| {
                reaching_flow(&script.code[pc], pc, script.code.len(), state).next
            })
    }
}

/// Resolve traffic whose shape depends on a reaching value or config definition.
/// This makes no claim about successful host execution or returned values.
pub fn resolved_effect(instruction: &Instruction, state: &State, configs: &ConfigTypes) -> Effect {
    resolved_effect_with_options(instruction, state, configs, &AnalysisOptions::default())
}

pub fn resolved_effect_with_options(
    instruction: &Instruction,
    state: &State,
    configs: &ConfigTypes,
    options: &AnalysisOptions,
) -> Effect {
    if options.enum_string_types.contains_key(&instruction.opcode) {
        return options
            .enum_traffic(instruction, state)
            .map_or(Effect::Unknown, |traffic| traffic.effect);
    }
    if let Some(fields) = options.database_fields.get(&instruction.opcode) {
        return state
            .int_from_top(1)
            .and_then(|field| fields.get(&field))
            .map_or(Effect::Unknown, |traffic| traffic.effect);
    }
    if let Some(effect) = options.traffic.get(&instruction.opcode) {
        return *effect;
    }
    let command = instruction.command.as_str();
    let fixed = |pops, pushes| Effect::Fixed { pops, pushes };
    if matches!(command, "push_var" | "pop_var") {
        let Operand::VarRef(var) = &instruction.operand else {
            return Effect::Unknown;
        };
        let lane = match configs.var_kind(var.domain, var.id) {
            Some(VarValueKind::Int) => 0,
            Some(VarValueKind::String) => 1,
            Some(VarValueKind::Long) => 2,
            _ => return Effect::Unknown,
        };
        let mut counts = [0; 3];
        counts[lane] = 1;
        return if command == "push_var" {
            fixed([0; 3], counts)
        } else {
            fixed(counts, [0; 3])
        };
    }
    if matches!(
        command,
        "struct_param"
            | "oc_param"
            | "nc_param"
            | "lc_param"
            | "seq_param"
            | "mec_param"
            | "quest_param"
            | "cc_param"
    ) {
        let Some(string) = state
            .int_from_top(0)
            .and_then(|id| configs.param_is_string(id).ok())
        else {
            return Effect::Unknown;
        };
        return fixed(
            [if command == "cc_param" { 1 } else { 2 }, 0, 0],
            if string { [0, 1, 0] } else { [1, 0, 0] },
        );
    }
    if command == "player_group_member_get_same_world_var" {
        let lane = match state.int_from_top(1) {
            Some(flag) if flag != 1 => Some(0),
            Some(_) => state
                .int_from_top(0)
                .and_then(|id| u16::try_from(id).ok())
                .and_then(|id| configs.var_kind(crate::vars::VarScope::Player, id))
                .and_then(|kind| match kind {
                    VarValueKind::Int => Some(0),
                    VarValueKind::String => Some(1),
                    VarValueKind::Long => Some(2),
                    VarValueKind::Other => None,
                }),
            None => None,
        };
        let Some(lane) = lane else {
            return Effect::Unknown;
        };
        let mut pushes = [0; 3];
        pushes[lane] = 1;
        return fixed([3, 0, 0], pushes);
    }
    if command == "db_listall" {
        return configs
            .db_listall_pushes_count(state.int_from_top(0))
            .map_or(Effect::Unknown, |count| {
                fixed([1, 0, 0], [u16::from(count), 0, 0])
            });
    }
    if command == "db_getfield" {
        let Some(types) = state
            .int_from_top(1)
            .and_then(|field| configs.db_field_types(field))
        else {
            return Effect::Unknown;
        };
        let mut pushes = [0_u16; 3];
        for kind in types {
            // The retail handler uses string for serial 36 and int otherwise,
            // including its default-value path. It never pushes the long lane.
            let lane = usize::from(*kind == 36);
            let Some(count) = pushes[lane].checked_add(1) else {
                return Effect::Unknown;
            };
            pushes[lane] = count;
        }
        return fixed([3, 0, 0], pushes);
    }
    if command == "_enum" {
        // `_enum` selects the lane using the declared output serial
        // (second of four int arguments); the definition must agree at runtime.
        let Some(output) = state.int_from_top(2) else {
            return Effect::Unknown;
        };
        return fixed([4, 0, 0], if output == 36 { [0, 1, 0] } else { [1, 0, 0] });
    }
    if crate::semantics::is_hook_setter(command) {
        let Some(Constant::String(descriptor)) =
            state.stacks[1].last().and_then(|v| v.constant.as_ref())
        else {
            return Effect::Unknown;
        };
        let codec = options
            .hooks
            .get(&instruction.opcode)
            .copied()
            .unwrap_or_default();
        let explicit = u16::from(command.starts_with("if_"));
        let mut pops = [1 + explicit, 1, 0]; // script ID, optional component, descriptor
        let mut trigger_count = None;
        if descriptor.ends_with('Y') {
            let Some(count) = state.int_from_top(usize::from(explicit)) else {
                return Effect::Unknown;
            };
            trigger_count = Some(count);
            if count > 0 && codec.positive_triggers == PositiveTriggers::Trap {
                return Effect::Trap;
            }
            let Ok(extra) = u16::try_from(count.max(0)) else {
                return Effect::Unknown;
            };
            let Some(total) = pops[0].checked_add(extra).and_then(|n| n.checked_add(1)) else {
                return Effect::Unknown;
            };
            pops[0] = total;
        }
        let Some(units) = codec.signature_units(descriptor, trigger_count, None) else {
            return Effect::Unknown;
        };
        for unit in units {
            let lane = match unit {
                115 => 1,
                108 => 2,
                _ => 0,
            };
            let Some(count) = pops[lane].checked_add(1) else {
                return Effect::Unknown;
            };
            pops[lane] = count;
        }
        return fixed(pops, [0; 3]);
    }
    effect(command, &instruction.operand)
}

/// Analyze all CFG paths, retaining reaching values and local definitions.
pub fn analyze(
    script: &CompiledScript,
    scripts: &BTreeMap<i32, CompiledScript>,
    summaries: &BTreeMap<i32, ReturnArity>,
    configs: &ConfigTypes,
) -> Analysis {
    analyze_with_values(script, scripts, summaries, &BTreeMap::new(), configs)
}

pub type ReturnValues = BTreeMap<i32, [Vec<Value>; 3]>;
/// A corpus analysis reused by symbol, graph and diagnostic consumers.
pub type Summaries = (BTreeMap<i32, ReturnArity>, ReturnValues);

/// Call summaries substitute proven constants and argument aliases only.
pub fn analyze_with_values(
    script: &CompiledScript,
    scripts: &BTreeMap<i32, CompiledScript>,
    summaries: &BTreeMap<i32, ReturnArity>,
    values: &ReturnValues,
    configs: &ConfigTypes,
) -> Analysis {
    analyze_paths(
        script,
        scripts,
        summaries,
        values,
        configs,
        PathMode {
            cut_unknown_calls: false,
            retain_instructions: true,
        },
        &AnalysisOptions::default(),
    )
}

/// Proven entry constants, in declaration order on each typed lane. Omitted
/// entries remain unknown; this is never a guess that an argument is zero.
pub type EntryArguments = [Vec<Option<Constant>>; 3];

/// Entry facts separate a known string representation from its unknown content.
/// A string fact cannot accompany a null or a value on another typed lane.
#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct EntryContext {
    pub arguments: EntryArguments,
    pub nonnull_strings: Vec<bool>,
}

/// Analyze one entry or call context without publishing its signature as a
/// universal return count for the script.
pub fn analyze_in_context(
    script: &CompiledScript,
    scripts: &BTreeMap<i32, CompiledScript>,
    summaries: &BTreeMap<i32, ReturnArity>,
    values: &ReturnValues,
    configs: &ConfigTypes,
    arguments: &EntryArguments,
) -> Analysis {
    analyze_with_entry_context(
        script,
        scripts,
        summaries,
        values,
        configs,
        &EntryContext {
            arguments: arguments.clone(),
            ..EntryContext::default()
        },
    )
}

/// Analyze supplied representation facts without treating them as a universal
/// signature or claiming that an absent object argument contains a string.
pub fn analyze_with_entry_context(
    script: &CompiledScript,
    scripts: &BTreeMap<i32, CompiledScript>,
    summaries: &BTreeMap<i32, ReturnArity>,
    values: &ReturnValues,
    configs: &ConfigTypes,
    context: &EntryContext,
) -> Analysis {
    analyze_with_options(
        script,
        scripts,
        summaries,
        values,
        configs,
        context,
        &AnalysisOptions::default(),
    )
}

/// Analyze source-specific value rules without changing runtime execution.
/// Supplied summaries must have been inferred under the same options.
pub fn analyze_with_options(
    script: &CompiledScript,
    scripts: &BTreeMap<i32, CompiledScript>,
    summaries: &BTreeMap<i32, ReturnArity>,
    values: &ReturnValues,
    configs: &ConfigTypes,
    context: &EntryContext,
    options: &AnalysisOptions,
) -> Analysis {
    Analyzer::new(scripts, summaries, values, configs, options).run(
        script,
        context,
        Mode {
            specialize_calls: true,
            cut_unknown_calls: false,
            retain_instructions: true,
        },
    )
}

#[derive(Clone, Copy)]
struct Mode {
    specialize_calls: bool,
    cut_unknown_calls: bool,
    retain_instructions: bool,
}

#[derive(Clone)]
struct CallSummary {
    arity: ReturnArity,
    returned: Option<[Vec<Value>; 3]>,
}

// A finite exploration budget; exhausting it leaves the call unresolved.
const MAX_CONTEXT_DEPTH: usize = 64;
const MAX_CALL_CONTEXTS: usize = 1024;
type CallContext = (i32, EntryContext);
struct Analyzer<'a> {
    scripts: &'a BTreeMap<i32, CompiledScript>,
    summaries: &'a BTreeMap<i32, ReturnArity>,
    values: &'a ReturnValues,
    configs: &'a ConfigTypes,
    contexts: BTreeMap<CallContext, CallSummary>,
    active: BTreeSet<i32>,
    options: AnalysisOptions,
}
impl<'a> Analyzer<'a> {
    fn new(
        scripts: &'a BTreeMap<i32, CompiledScript>,
        summaries: &'a BTreeMap<i32, ReturnArity>,
        values: &'a ReturnValues,
        configs: &'a ConfigTypes,
        options: &AnalysisOptions,
    ) -> Self {
        Self {
            scripts,
            summaries,
            values,
            configs,
            contexts: BTreeMap::new(),
            active: BTreeSet::new(),
            options: options.clone(),
        }
    }

    fn call(
        &mut self,
        id: i32,
        state: &State,
        specialize: bool,
    ) -> Result<CallSummary, FailureKind> {
        let callee = self.scripts.get(&id).ok_or(FailureKind::MissingCallee)?;
        let refine_strings = specialize
            && self.values.get(&id).is_some_and(|returned| {
                returned[1].iter().any(|value| {
                    !value.nonnull_string && value.constant.is_none() && value.argument.is_none()
                })
            })
            && state.stacks[1]
                .iter()
                .rev()
                .take(usize::from(callee.args.obj))
                .any(|value| value.nonnull_string);
        if !refine_strings
            && let Some(arity @ (ReturnArity::Known { .. } | ReturnArity::Never)) =
                self.summaries.get(&id)
        {
            return Ok(CallSummary {
                arity: *arity,
                returned: self.values.get(&id).cloned(),
            });
        }
        if !specialize {
            return Err(FailureKind::UnknownCalleeReturn);
        }
        let counts = [callee.args.int, callee.args.obj, callee.args.long];
        let mut arguments: EntryArguments = Default::default();
        for (lane, count) in counts.iter().enumerate() {
            let start = state.stacks[lane]
                .len()
                .checked_sub(usize::from(*count))
                .ok_or(FailureKind::StackUnderflow)?;
            arguments[lane] = state.stacks[lane][start..]
                .iter()
                .map(|v| v.constant.clone())
                .collect();
        }
        let object_start = state.stacks[1].len() - usize::from(callee.args.obj);
        let nonnull_strings: Vec<_> = state.stacks[1][object_start..]
            .iter()
            .map(|value| value.nonnull_string)
            .collect();
        if (arguments.iter().flatten().all(Option::is_none) && !nonnull_strings.contains(&true))
            || self.active.len() >= MAX_CONTEXT_DEPTH
            || self.contexts.len() >= MAX_CALL_CONTEXTS
            || self.active.contains(&id)
        {
            return Err(FailureKind::UnknownCalleeReturn);
        }
        let key = (
            id,
            EntryContext {
                arguments,
                nonnull_strings,
            },
        );
        if let Some(summary) = self.contexts.get(&key) {
            return Ok(summary.clone());
        }
        self.active.insert(id);
        let analysis = self.run(
            callee,
            &key.1,
            Mode {
                specialize_calls: true,
                cut_unknown_calls: false,
                retain_instructions: false,
            },
        );
        self.active.remove(&id);
        let summary = CallSummary {
            arity: analysis.arity,
            returned: analysis.returned,
        };
        self.contexts.insert(key, summary.clone());
        Ok(summary)
    }

    fn run(&mut self, script: &CompiledScript, context: &EntryContext, mode: Mode) -> Analysis {
        let arguments = &context.arguments;
        let mut result = Analysis {
            before: vec![None; script.code.len()],
            arity: ReturnArity::Unknown,
            failure: None,
            returned: None,
            nonreturning: BTreeSet::new(),
        };
        let counts = [script.locals.int, script.locals.obj, script.locals.long];
        let args = [script.args.int, script.args.obj, script.args.long];
        if script.code.is_empty()
            || args.iter().zip(counts).any(|(a, l)| *a > l)
            || context.nonnull_strings.len() > usize::from(script.args.obj)
            || context
                .nonnull_strings
                .iter()
                .enumerate()
                .any(|(slot, known)| {
                    *known
                        && arguments[1]
                            .get(slot)
                            .and_then(Option::as_ref)
                            .is_some_and(|value| !matches!(value, Constant::String(_)))
                })
            || arguments
                .iter()
                .zip(args)
                .any(|(v, count)| v.len() > usize::from(count))
            || arguments.iter().enumerate().any(|(lane, entries)| {
                entries.iter().flatten().any(|value| {
                    !matches!(
                        (lane, value),
                        (0, Constant::Int(_))
                            | (1, Constant::String(_) | Constant::Null)
                            | (2, Constant::Long(_))
                    )
                })
            })
        {
            result.failure = Some((0, FailureKind::InvalidOperand));
            return result;
        }
        let locals = std::array::from_fn(|lane| {
            (0..counts[lane])
                .map(|slot| {
                    let mut value = Value::entry(if slot < args[lane] {
                        arguments[lane].get(usize::from(slot)).cloned().flatten()
                    } else if self.options.local_defaults == LocalDefaults::Unknown {
                        None
                    } else {
                        Some(match lane {
                            0 => Constant::Int(0),
                            1 => Constant::Null,
                            _ => Constant::Long(0),
                        })
                    });
                    if slot < args[lane] {
                        value.argument = Some((lane, slot));
                        if lane == 1
                            && context.nonnull_strings.get(usize::from(slot)) == Some(&true)
                        {
                            value.nonnull_string = true;
                        }
                    }
                    value
                })
                .collect()
        });
        result.before[0] = Some(State {
            stacks: std::array::from_fn(|_| Vec::new()),
            locals,
        });
        let mut queue = VecDeque::from([0]);
        let mut queued = vec![false; script.code.len()];
        queued[0] = true;
        let mut ends = None;
        let mut incoming = vec![0_usize; script.code.len()];
        if !mode.retain_instructions {
            for (pc, instruction) in script.code.iter().enumerate() {
                for target in crate::returns::flow(instruction, pc, script.code.len()).next {
                    incoming[target] += 1;
                }
            }
        }
        while let Some(mut pc) = queue.pop_front() {
            queued[pc] = false;
            let Some(mut state) = result.before[pc].clone() else {
                continue;
            };
            loop {
                let instr = &script.code[pc];
                if mode.cut_unknown_calls
                    && instr.command == "gosub_with_params"
                    && let Operand::Script(target) | Operand::Int(target) = instr.operand
                    && self.summaries.get(&target) == Some(&ReturnArity::Unknown)
                {
                    break;
                }
                let edges = reaching_flow(instr, pc, script.code.len(), &state);
                let call = if instr.command == "gosub_with_params" {
                    match instr.operand {
                        Operand::Script(id) | Operand::Int(id) => {
                            self.call(id, &state, mode.specialize_calls).map(Some)
                        }
                        _ => Err(FailureKind::InvalidOperand),
                    }
                } else {
                    Ok(None)
                };
                let typed = crate::execution::import_signature(script)
                    .and_then(|signature| self.options.bound_traffic.get(signature))
                    .and_then(|traffic| traffic.get(&pc))
                    .or_else(|| {
                        self.options
                            .database_fields
                            .get(&instr.opcode)
                            .and_then(|fields| {
                                state.int_from_top(1).and_then(|field| fields.get(&field))
                            })
                    })
                    .cloned()
                    .or_else(|| self.options.enum_traffic(instr, &state));
                let resolved = typed.as_ref().map_or_else(
                    || resolved_effect_with_options(instr, &state, self.configs, &self.options),
                    |traffic| traffic.effect,
                );
                let nonreturning = resolved == Effect::Trap
                    || call
                        .as_ref()
                        .ok()
                        .and_then(Option::as_ref)
                        .is_some_and(|summary| summary.arity == ReturnArity::Never);
                let transfer = call.and_then(|call| {
                    transfer(pc, instr, &mut state, self.scripts, call.as_ref(), resolved)
                });
                if transfer.is_ok()
                    && let Some(typed) = &typed
                {
                    let Effect::Fixed { pushes, .. } = typed.effect else {
                        result.failure = Some((pc, FailureKind::InvalidOperand));
                        return result;
                    };
                    if typed.nonnull_strings.len() != usize::from(pushes[1]) {
                        result.failure = Some((pc, FailureKind::InvalidOperand));
                        return result;
                    }
                    let start = state.stacks[1].len() - typed.nonnull_strings.len();
                    for (value, nonnull) in state.stacks[1][start..]
                        .iter_mut()
                        .zip(&typed.nonnull_strings)
                    {
                        value.nonnull_string = *nonnull;
                    }
                }
                if nonreturning && transfer.is_ok() {
                    result.nonreturning.insert(pc);
                    break;
                }
                let failure = if let Err(reason) = transfer {
                    Some(reason)
                } else if edges.lost {
                    Some(FailureKind::InvalidControlFlow)
                } else {
                    None
                };
                if let Some(reason) = failure {
                    result.failure = Some((pc, reason));
                    return result;
                }
                if edges.exit {
                    let depths: Vec<usize> = state.stacks.iter().map(Vec::len).collect();
                    if ends.as_ref().is_some_and(|prior| *prior != depths) {
                        result.failure = Some((pc, FailureKind::DivergentExit));
                        return result;
                    }
                    ends = Some(depths);
                    if let Some(prior) = &mut result.returned {
                        for (left, right) in prior.iter_mut().zip(&state.stacks) {
                            for (a, b) in left.iter_mut().zip(right) {
                                a.merge(b);
                            }
                        }
                    } else {
                        result.returned = Some(state.stacks.clone());
                    }
                }
                if !mode.retain_instructions
                    && edges.next.as_slice() == [pc + 1]
                    && incoming[pc + 1] == 1
                {
                    pc += 1;
                    continue;
                }
                for next in edges.next {
                    let changed = if let Some(prior) = &mut result.before[next] {
                        match prior.merge(&state) {
                            Ok(changed) => changed,
                            Err(reason) => {
                                result.failure = Some((next, reason));
                                return result;
                            }
                        }
                    } else {
                        result.before[next] = Some(state.clone());
                        true
                    };
                    if changed && !queued[next] {
                        queue.push_back(next);
                        queued[next] = true;
                    }
                }
                break;
            }
        }
        if let Some(depths) = ends {
            if let (Ok(int), Ok(obj), Ok(long)) = (
                u16::try_from(depths[0]),
                u16::try_from(depths[1]),
                u16::try_from(depths[2]),
            ) {
                result.arity = ReturnArity::Known { int, obj, long };
            } else {
                result.failure = Some((0, FailureKind::DepthOverflow));
            }
        } else if mode.cut_unknown_calls {
            result.failure = Some((0, FailureKind::NoExit));
        } else {
            result.arity = ReturnArity::Never;
        }
        result
    }
}

#[derive(Clone, Copy)]
struct PathMode {
    cut_unknown_calls: bool,
    retain_instructions: bool,
}

fn analyze_paths(
    script: &CompiledScript,
    scripts: &BTreeMap<i32, CompiledScript>,
    summaries: &BTreeMap<i32, ReturnArity>,
    values: &ReturnValues,
    configs: &ConfigTypes,
    mode: PathMode,
    options: &AnalysisOptions,
) -> Analysis {
    Analyzer::new(scripts, summaries, values, configs, options).run(
        script,
        &EntryContext::default(),
        Mode {
            specialize_calls: mode.retain_instructions,
            cut_unknown_calls: mode.cut_unknown_calls,
            retain_instructions: mode.retain_instructions,
        },
    )
}

/// A constant predicate selects exactly one edge. Other predicates retain every
/// possible edge; revisiting a widened loop state can therefore add successors.
fn reaching_flow(
    instruction: &Instruction,
    pc: usize,
    len: usize,
    state: &State,
) -> crate::returns::Edges {
    let command = instruction.command.as_str();
    let selected = if command == "switch" {
        match (&instruction.operand, state.int_from_top(0)) {
            (Operand::Switch(cases), Some(key)) => Some(
                cases
                    .iter()
                    .find(|case| case.value == key)
                    .map(|case| case.target),
            ),
            _ => None,
        }
    } else if let Operand::Branch(target) = instruction.operand {
        let taken = if command == "branch_if_true" {
            state.int_from_top(0).map(|value| value == 1)
        } else if command == "branch_if_false" {
            state.int_from_top(0).map(|value| value == 0)
        } else {
            let (lane, comparison) = if let Some(comparison) = command.strip_prefix("long_branch_")
            {
                (2, comparison)
            } else if let Some(comparison) = command.strip_prefix("branch_") {
                (0, comparison)
            } else {
                return crate::returns::flow(instruction, pc, len);
            };
            let stack = &state.stacks[lane];
            let pair = stack.len().checked_sub(2).and_then(|start| {
                match (&stack[start].constant, &stack[start + 1].constant) {
                    (Some(Constant::Int(a)), Some(Constant::Int(b))) if lane == 0 => Some(a.cmp(b)),
                    (Some(Constant::Long(a)), Some(Constant::Long(b))) if lane == 2 => {
                        Some(a.cmp(b))
                    }
                    _ => None,
                }
            });
            pair.and_then(|order| match comparison {
                "equals" => Some(order.is_eq()),
                "not" => Some(!order.is_eq()),
                "less_than" => Some(order.is_lt()),
                "greater_than" => Some(order.is_gt()),
                "less_than_or_equals" => Some(!order.is_gt()),
                "greater_than_or_equals" => Some(!order.is_lt()),
                _ => None,
            })
        };
        taken.map(|taken| taken.then_some(target))
    } else {
        None
    };
    match selected {
        None => crate::returns::flow(instruction, pc, len),
        Some(target) => {
            let next = match target {
                Some(target) => usize::try_from(target).ok(),
                None => pc.checked_add(1),
            };
            match next.filter(|next| *next < len) {
                Some(next) => crate::returns::Edges {
                    next: vec![next],
                    exit: false,
                    lost: false,
                },
                None => crate::returns::Edges {
                    next: vec![],
                    exit: false,
                    lost: true,
                },
            }
        }
    }
}

fn transfer(
    pc: usize,
    instruction: &Instruction,
    state: &mut State,
    scripts: &BTreeMap<i32, CompiledScript>,
    call: Option<&CallSummary>,
    resolved: Effect,
) -> Result<(), FailureKind> {
    let (pops, pushes) = match resolved {
        Effect::Fixed { pops, pushes } => (pops, pushes),
        Effect::ControlFlow { pops } => (pops, [0; 3]),
        Effect::Trap => ([0; 3], [0; 3]),
        Effect::Conditional { .. } => return Err(FailureKind::RuntimeDependent),
        Effect::Unknown => {
            return Err(
                if crate::semantics::type_rule(&instruction.command)
                    == crate::semantics::TypeRule::Unknown
                {
                    FailureKind::UnknownCommand
                } else {
                    FailureKind::UnresolvedType
                },
            );
        }
        Effect::Call => {
            let (Operand::Script(id) | Operand::Int(id)) = instruction.operand else {
                return Err(FailureKind::InvalidOperand);
            };
            let callee = scripts.get(&id).ok_or(FailureKind::MissingCallee)?;
            let pushes = match call.map(|summary| summary.arity) {
                Some(ReturnArity::Known { int, obj, long }) => [int, obj, long],
                Some(ReturnArity::Never) => [0; 3],
                _ => return Err(FailureKind::UnknownCalleeReturn),
            };
            ([callee.args.int, callee.args.obj, callee.args.long], pushes)
        }
    };
    let mut taken: [Vec<Value>; 3] = std::array::from_fn(|_| Vec::new());
    for lane in 0..3 {
        let start = state.stacks[lane]
            .len()
            .checked_sub(usize::from(pops[lane]))
            .ok_or(FailureKind::StackUnderflow)?;
        taken[lane] = state.stacks[lane].split_off(start);
    }
    let name = instruction.command.as_str();
    if name == "gosub_with_params"
        && let Some(results) = call.and_then(|summary| summary.returned.as_ref())
    {
        for (lane, entries) in results.iter().enumerate() {
            if entries.len() != usize::from(pushes[lane]) {
                return Err(FailureKind::InvalidOperand);
            }
            for entry in entries {
                let value = if let Some((source, slot)) = entry.argument {
                    taken
                        .get(source)
                        .and_then(|stack| stack.get(usize::from(slot)))
                        .cloned()
                        .ok_or(FailureKind::InvalidOperand)?
                } else {
                    let mut value = Value::produced(pc, entry.constant.clone());
                    value.nonnull_string = entry.nonnull_string;
                    value
                };
                state.stacks[lane].push(value);
            }
        }
        return Ok(());
    }
    let local_lane = match name {
        "push_int_local" | "pop_int_local" => Some(0),
        "push_string_local" | "pop_string_local" => Some(1),
        "push_long_local" | "pop_long_local" => Some(2),
        _ => None,
    };
    if let Some(lane) = local_lane {
        let (Operand::Local(slot) | Operand::Int(slot)) = instruction.operand else {
            return Err(FailureKind::InvalidOperand);
        };
        let local = usize::try_from(slot)
            .ok()
            .and_then(|i| state.locals[lane].get_mut(i))
            .ok_or(FailureKind::InvalidOperand)?;
        if name.starts_with("pop_") {
            *local = taken[lane][0].clone();
        } else {
            state.stacks[lane].push(local.clone());
        }
        return Ok(());
    }
    match name {
        "push_array_int_leave_index_on_stack" => {
            state.stacks[0].push(taken[0][0].clone());
            state.stacks[0].push(Value::produced(pc, None));
            return Ok(());
        }
        "push_array_int_and_index" => {
            state.stacks[0].push(Value::produced(pc, None));
            state.stacks[0].push(taken[0][0].clone());
            return Ok(());
        }
        "pop_array_int_leave_value_on_stack" => {
            state.stacks[0].push(taken[0][1].clone());
            return Ok(());
        }
        _ => {}
    }
    let constant = match (name, &instruction.operand) {
        ("push_constant_int" | "push_constant_string", Operand::Int(n)) => Some(Constant::Int(*n)),
        ("push_long_constant" | "push_constant_string", Operand::Long(n)) => {
            Some(Constant::Long(*n))
        }
        ("push_constant_string", Operand::Str(s)) => Some(Constant::String(s.clone())),
        _ => evaluate(name, &taken),
    };
    for (lane, count) in pushes.iter().enumerate() {
        if state.stacks[lane].len() + usize::from(*count) > usize::from(u16::MAX) {
            return Err(FailureKind::DepthOverflow);
        }
        for _ in 0..*count {
            let mut value = Value::produced(pc, constant.clone());
            if lane == 1 && matches!(name, "join_string" | "append" | "tostring") {
                value.nonnull_string = true;
            }
            state.stacks[lane].push(value);
        }
    }
    Ok(())
}

/// Evaluate only deterministic, bounded operations with 32-bit wrapping arithmetic.
fn evaluate(name: &str, taken: &[Vec<Value>; 3]) -> Option<Constant> {
    if let Some(value) = crate::cs2_stack_contracts::constant_int_result(name) {
        return Some(Constant::Int(value));
    }
    let ints: Option<Vec<i32>> = taken[0]
        .iter()
        .map(|v| match v.constant {
            Some(Constant::Int(n)) => Some(n),
            _ => None,
        })
        .collect();
    if let Some(ints) = ints {
        let value = match (name, ints.as_slice()) {
            ("add", [a, b]) => Some(a.wrapping_add(*b)),
            ("quickchat_dynamic_command_add", [a, b]) => Some(a.wrapping_sub(*b)),
            ("multiply", [a, b]) => Some(a.wrapping_mul(*b)),
            ("divide", [a, b]) if *b != 0 => Some(a.wrapping_div(*b)),
            ("modulo", [a, b]) if *b != 0 => Some(a.wrapping_rem(*b)),
            ("and", [a, b]) => Some(a & b),
            ("or", [a, b]) => Some(a | b),
            ("min", [a, b]) => Some(*a.min(b)),
            ("max", [a, b]) => Some(*a.max(b)),
            ("abs", [a]) => Some(a.wrapping_abs()),
            _ => None,
        };
        if let Some(n) = value {
            return Some(Constant::Int(n));
        }
        if let ("tostring", [n]) = (name, ints.as_slice()) {
            return Some(Constant::String(n.to_string()));
        }
    }
    if matches!(name, "append" | "join_string") {
        let strings: Option<Vec<&str>> = taken[1]
            .iter()
            .map(|v| match &v.constant {
                Some(Constant::String(s)) => Some(s.as_str()),
                Some(Constant::Null) => Some("null"),
                _ => None,
            })
            .collect();
        let strings = strings?;
        if strings.iter().map(|s| s.len()).sum::<usize>() <= 65536 {
            return Some(Constant::String(crate::jstr::normalize(strings.concat())));
        }
    }
    None
}

/// Monotone interprocedural signature discovery. Recursive cycles remain explicit
/// unknowns until an independently verified signature can ground them.
pub fn infer_returns(
    scripts: &BTreeMap<i32, CompiledScript>,
    configs: &ConfigTypes,
) -> (BTreeMap<i32, ReturnArity>, bool) {
    let (summaries, _) = infer_summaries(scripts, configs);
    (summaries, true)
}

pub fn infer_summaries(
    scripts: &BTreeMap<i32, CompiledScript>,
    configs: &ConfigTypes,
) -> (BTreeMap<i32, ReturnArity>, ReturnValues) {
    infer_summaries_with_options(scripts, configs, &AnalysisOptions::default())
}

pub fn infer_summaries_with_options(
    scripts: &BTreeMap<i32, CompiledScript>,
    configs: &ConfigTypes,
    options: &AnalysisOptions,
) -> (BTreeMap<i32, ReturnArity>, ReturnValues) {
    let mut values = ReturnValues::new();
    let mut unresolved_calls = BTreeMap::new();
    let mut summaries: BTreeMap<_, _> = scripts
        .keys()
        .map(|id| (*id, ReturnArity::Unknown))
        .collect();
    let mut callers: BTreeMap<i32, BTreeSet<i32>> = BTreeMap::new();
    for (id, script) in scripts {
        for instruction in &script.code {
            if instruction.command == "gosub_with_params"
                && let Operand::Script(target) | Operand::Int(target) = instruction.operand
            {
                callers.entry(target).or_default().insert(*id);
            }
        }
    }
    let mut queue: VecDeque<i32> = scripts.keys().copied().collect();
    let mut queued: BTreeSet<i32> = scripts.keys().copied().collect();
    let trace = std::env::var_os("ALTO_TRACE_SUMMARIES").is_some();
    let mut visits = 0_usize;
    let mut specialize_calls = false;
    loop {
        while let Some(id) = queue.pop_front() {
            visits += 1;
            if trace && visits.is_multiple_of(10000) {
                eprintln!("summary visits={visits} script={id} queued={}", queue.len());
            }
            queued.remove(&id);
            let analysis = Analyzer::new(scripts, &summaries, &values, configs, options).run(
                &scripts[&id],
                &EntryContext::default(),
                Mode {
                    specialize_calls,
                    cut_unknown_calls: false,
                    retain_instructions: false,
                },
            );
            if let Some((pc, FailureKind::UnknownCalleeReturn)) = analysis.failure {
                if let Operand::Script(target) | Operand::Int(target) =
                    scripts[&id].code[pc].operand
                {
                    unresolved_calls.insert(id, target);
                }
            } else {
                unresolved_calls.remove(&id);
            }
            let arity = analysis.arity;
            if arity == ReturnArity::Unknown {
                continue;
            }
            let returned = analysis.returned.unwrap_or_default();
            if summaries[&id] == arity && values.get(&id) == Some(&returned) {
                continue;
            }
            summaries.insert(id, arity);
            values.insert(id, returned);
            if let Some(dependents) = callers.get(&id) {
                for dependent in dependents {
                    if queued.insert(*dependent) {
                        queue.push_back(*dependent);
                    }
                }
            }
        }
        let seeded = grounded_recursive_arities(
            scripts,
            &summaries,
            &values,
            configs,
            &unresolved_calls,
            options,
        );
        if seeded.is_empty() {
            if specialize_calls {
                break;
            }
            // Establish cheap universal summaries first. Only the remaining
            // blockers need recursive call-site proofs, avoiding repeated
            // descent through the whole still-unclassified corpus.
            specialize_calls = true;
            for (id, arity) in &summaries {
                if *arity == ReturnArity::Unknown && queued.insert(*id) {
                    queue.push_back(*id);
                }
            }
            continue;
        }
        for (id, arity) in seeded {
            summaries.insert(id, arity);
            if queued.insert(id) {
                queue.push_back(id);
            }
            if let Some(dependents) = callers.get(&id) {
                for dependent in dependents {
                    if queued.insert(*dependent) {
                        queue.push_back(*dependent);
                    }
                }
            }
        }
    }
    (summaries, values)
}

/// Infer hypotheses only from paths reaching a return without unknown calls.
/// Then validate every path against the hypotheses, repeatedly removing failed
/// hypotheses and their dependents. Surviving cycles have finite base cases and
/// an inductive stack-shape proof; no guessed zero-return signatures are seeded.
fn grounded_recursive_arities(
    scripts: &BTreeMap<i32, CompiledScript>,
    known: &BTreeMap<i32, ReturnArity>,
    values: &ReturnValues,
    configs: &ConfigTypes,
    unresolved_calls: &BTreeMap<i32, i32>,
    options: &AnalysisOptions,
) -> BTreeMap<i32, ReturnArity> {
    let dependent = cycle_dependents(unresolved_calls);
    let mut candidates: BTreeMap<_, _> = scripts
        .iter()
        .filter(|(id, _)| known[id] == ReturnArity::Unknown && dependent.contains(id))
        .filter_map(|(id, script)| {
            let base = analyze_paths(
                script,
                scripts,
                known,
                values,
                configs,
                PathMode {
                    cut_unknown_calls: true,
                    retain_instructions: false,
                },
                options,
            )
            .arity;
            (base != ReturnArity::Unknown).then_some((*id, base))
        })
        .collect();
    // Propagate grounded hypotheses through mutually recursive members that
    // have no direct base-return path of their own. Every hypothesis is still
    // checked against all paths below before any signature is published.
    loop {
        let mut assumed = known.clone();
        assumed.extend(candidates.iter().map(|(id, arity)| (*id, *arity)));
        let fresh: Vec<_> = dependent
            .iter()
            .filter(|id| known[id] == ReturnArity::Unknown && !candidates.contains_key(id))
            .filter_map(|id| {
                let script = scripts.get(id)?;
                let base = analyze_paths(
                    script,
                    scripts,
                    &assumed,
                    values,
                    configs,
                    PathMode {
                        cut_unknown_calls: true,
                        retain_instructions: false,
                    },
                    options,
                )
                .arity;
                matches!(base, ReturnArity::Known { .. }).then_some((*id, base))
            })
            .collect();
        if fresh.is_empty() {
            break;
        }
        candidates.extend(fresh);
    }
    loop {
        let mut assumed = known.clone();
        assumed.extend(candidates.iter().map(|(id, a)| (*id, *a)));
        let rejected: Vec<_> = candidates
            .iter()
            .filter_map(|(id, arity)| {
                let checked = analyze_paths(
                    &scripts[id],
                    scripts,
                    &assumed,
                    values,
                    configs,
                    PathMode {
                        cut_unknown_calls: false,
                        retain_instructions: false,
                    },
                    options,
                )
                .arity;
                (checked != *arity).then_some(*id)
            })
            .collect();
        if rejected.is_empty() {
            return candidates;
        }
        for id in rejected {
            candidates.remove(&id);
        }
    }
}

/// Nodes whose first unresolved-call chain reaches a cycle. Terminal missing
/// command/type roots cannot be fixed by recursive hypotheses and are excluded.
fn cycle_dependents(edges: &BTreeMap<i32, i32>) -> BTreeSet<i32> {
    let mut classified = BTreeMap::new();
    for start in edges.keys() {
        let mut path = BTreeSet::new();
        let mut cursor = *start;
        let cyclic = loop {
            if let Some(cyclic) = classified.get(&cursor) {
                break *cyclic;
            }
            if !path.insert(cursor) {
                break true;
            }
            let Some(next) = edges.get(&cursor) else {
                break false;
            };
            cursor = *next;
        };
        for node in path {
            classified.insert(node, cyclic);
        }
    }
    classified
        .into_iter()
        .filter_map(|(id, cyclic)| cyclic.then_some(id))
        .collect()
}

#[cfg(test)]
mod compact_tests {
    use super::*;
    #[test]
    fn recursive_hypotheses_never_republish_known_members() {
        let script = CompiledScript {
            name: None,
            args: crate::script::Counts::default(),
            locals: crate::script::Counts::default(),
            code: vec![Instruction {
                opcode: 0,
                command: "return".into(),
                operand: Operand::Byte(0),
            }],
        };
        let arity = ReturnArity::Known {
            int: 0,
            obj: 0,
            long: 0,
        };
        let scripts = BTreeMap::from([(1, script.clone()), (2, script)]);
        let known = BTreeMap::from([(1, arity), (2, ReturnArity::Unknown)]);
        let seeded = grounded_recursive_arities(
            &scripts,
            &known,
            &ReturnValues::new(),
            &ConfigTypes::empty(),
            &BTreeMap::from([(1, 2), (2, 1)]),
            &AnalysisOptions::default(),
        );
        assert_eq!(seeded, BTreeMap::from([(2, arity)]));
    }
    #[test]
    fn compact_summaries_match_full_states_across_a_loop() {
        let op = |command: &str, operand| Instruction {
            opcode: 0,
            command: command.into(),
            operand,
        };
        let script = CompiledScript {
            name: None,
            args: crate::script::Counts {
                int: 1,
                obj: 0,
                long: 0,
            },
            locals: crate::script::Counts {
                int: 2,
                obj: 0,
                long: 0,
            },
            code: vec![
                op("push_constant_int", Operand::Int(1)),
                op("pop_int_local", Operand::Local(1)),
                op("push_int_local", Operand::Local(0)),
                op("branch_if_false", Operand::Branch(9)),
                op("push_int_local", Operand::Local(1)),
                op("push_constant_int", Operand::Int(1)),
                op("add", Operand::Byte(0)),
                op("pop_int_local", Operand::Local(1)),
                op("branch", Operand::Branch(2)),
                op("push_int_local", Operand::Local(1)),
                op("return", Operand::Byte(0)),
            ],
        };
        let configs = ConfigTypes::empty();
        let scripts = BTreeMap::new();
        let summaries = BTreeMap::new();
        let values = ReturnValues::new();
        let full = analyze_paths(
            &script,
            &scripts,
            &summaries,
            &values,
            &configs,
            PathMode {
                cut_unknown_calls: false,
                retain_instructions: true,
            },
            &AnalysisOptions::default(),
        );
        let compact = analyze_paths(
            &script,
            &scripts,
            &summaries,
            &values,
            &configs,
            PathMode {
                cut_unknown_calls: false,
                retain_instructions: false,
            },
            &AnalysisOptions::default(),
        );
        assert_eq!(full.arity, compact.arity);
        assert_eq!(full.failure, compact.failure);
        assert_eq!(full.returned, compact.returned);
        assert!(full.before[1].is_some());
        assert!(compact.before[1].is_none());
    }
}
