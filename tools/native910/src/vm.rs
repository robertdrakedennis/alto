//! CS2 VM runtime for rev 910.
//!
//! Execution model of the retail CS2 machine:
//! - script decode: header counts, typed `push_constant_string` tags and
//!   var/varbit references;
//! - machine state: three typed stacks (int, object, long), three typed local
//!   tables and a call-frame table;
//! - call frames: a saved caller script, program counter and locals;
//! - hook entry (argument copy into a fresh state) and the fetch loop, which
//!   completes when a root frame returns;
//! - one handler per command that the VM executes for real.
//!
//! The VM never touches rendering or networking. All engine opcodes (interface,
//! world, audio, config lookups, …) go through [`Host::trap`], whose default is
//! an explicit [`VmError::UnknownCommand`] — structurally full coverage (every
//! [`CompiledScript`](crate::script::CompiledScript) command either executes
//! for real or traps), behaviorally incremental.
//!
//! Operand model: [`CompiledScript`](crate::script::CompiledScript) already
//! carries absolute [`Branch`](crate::script::Operand::Branch) /
//! [`Switch`](crate::script::Operand::Switch) targets (decode resolved the
//! relative `target = index + stored + 1` form), so the loop jumps directly to
//! absolute indices. Falling off the instruction stream fails; only a root
//! return completes execution. The codec can still preserve such branch bytes.

use crate::error::NativeError;
use crate::execution::{Accounting, HostOperation, Role};
use crate::script::{CompiledScript, Operand};
use crate::vars::VarScope;
use std::collections::HashMap;
use thiserror::Error;

thread_local! {
    /// The step counter: reset when an execution begins and incremented before
    /// each instruction fetch. Nested executions share it, as in the retail
    /// client, so it lives beside the sessions rather than inside one.
    static OPCOUNT: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// Current step counter value, read by the `opcount` command.
pub fn opcount() -> i32 {
    OPCOUNT.with(std::cell::Cell::get)
}

/// A script value on the typed stacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// 32-bit int (int stack).
    Int(i32),
    /// String (object stack).
    Str(String),
    /// A null reference on the object stack. Distinct from an empty string.
    Null,
    /// 64-bit long (long stack).
    Long(i64),
}

/// Every failure the VM can produce.
#[derive(Debug, Error)]
pub enum VmError {
    /// An explicit unconditional exception in the retail dispatcher.
    #[error("retail command {command} unconditionally throws")]
    RetailTrap { command: String },
    /// An engine opcode with no host implementation yet.
    #[error("unknown engine command '{command}'")]
    UnknownCommand {
        /// Command name that trapped.
        command: String,
    },
    /// A `gosub` callee the provider cannot resolve.
    #[error("unknown script {id}")]
    UnknownScript {
        /// Requested script id.
        id: i32,
    },
    /// A pop from an empty stack.
    #[error("{stack} stack underflow")]
    StackUnderflow {
        /// Which stack (`"int"`, `"object"`, `"long"`).
        stack: &'static str,
    },
    /// A push past the 1000-slot stack size.
    #[error("{stack} stack overflow")]
    StackFull {
        /// Which stack.
        stack: &'static str,
    },
    /// A local slot outside the allocated header counts.
    #[error("bad {kind} local slot {slot}")]
    BadLocal {
        /// Slot index requested.
        slot: i32,
        /// Which file (`"int"`, `"object"`, `"long"`).
        kind: &'static str,
    },
    /// An array misuse (missing id, bad length, out of bounds).
    #[error("bad array {id}: {reason}")]
    BadArray {
        /// Array id operand.
        id: i32,
        /// What was wrong.
        reason: String,
    },
    /// A branch/switch target outside `0..=code.len()`.
    #[error("bad branch target {target}")]
    BadBranch {
        /// Requested absolute target.
        target: i32,
    },
    /// A command carrying an operand kind outside its class.
    #[error("bad operand for '{command}': expected {expected}")]
    BadOperand {
        /// Command name.
        command: String,
        /// Expected operand kinds.
        expected: &'static str,
    },
    /// Entry args whose per-type counts differ from the script header.
    #[error("arity mismatch: expected {expected}, got {got}")]
    ArityMismatch {
        /// Header counts.
        expected: String,
        /// Supplied counts.
        got: String,
    },
    /// Integer divide/modulo by zero.
    #[error("divide by zero in '{command}'")]
    DivideByZero {
        /// Command name.
        command: String,
    },
    /// The 500k-step guard tripped.
    #[error("step limit exceeded")]
    StepLimit,
    /// A `gosub` past the 50-frame table.
    #[error("call frame overflow")]
    FrameOverflow,
    /// A host trap that started but failed.
    #[error("trap '{command}' failed: {reason}")]
    TrapFailed {
        /// Command name.
        command: String,
        /// Host-supplied reason.
        reason: String,
    },
    /// A string consumer dereferenced a null object.
    #[error("null object in '{command}'")]
    NullObject { command: String },
    /// `append_char` with `-1`.
    #[error("invalid char value {value}")]
    InvalidChar {
        /// Requested code unit.
        value: i32,
    },
    /// A string op with out-of-range indices.
    #[error("string bounds: {reason}")]
    StringBounds {
        /// What was wrong.
        reason: String,
    },
}

/// VM result alias.
pub type VmResult<T> = Result<T, VmError>;

impl From<VmError> for NativeError {
    fn from(error: VmError) -> Self {
        Self::Invalid(error.to_string())
    }
}

/// Legacy preview table; not used by faithful execution.
pub use crate::preview::{Arity, arity};

/// Instruction context supplied without discarding arguments.
#[derive(Clone, Copy, Debug)]
pub struct InstructionContext<'a> {
    pub script_name: Option<&'a str>,
    pub script_id: Option<i32>,
    pub event: Option<&'a str>,
    pub pc: usize,
    pub command: &'a str,
    pub operand: &'a Operand,
    pub secondary: bool,
    /// Read-only arguments of a generated host bridge, validated by its metadata.
    pub int_locals: &'a [i32],
}

/// Variable lane from a definition, independent of its current value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VarLane {
    Int,
    String,
    Long,
}

/// Observable writes reported by hosts at instruction boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEffect {
    VariableWrite {
        domain: VarScope,
        id: u16,
        secondary: bool,
        value: Value,
    },
    ComponentText {
        packed: i32,
        child: i32,
        text: String,
    },
}

/// Engine state behind the VM: vars, varbits, arrays, and the engine-opcode trap.
///
/// `transmog` flags select the host's secondary variable domain. Variable types
/// come from definitions through `var_type`, never from current stored values.
pub trait Host {
    /// Drain writes since the last executed instruction. No synthetic events.
    fn take_effects(&mut self) -> Vec<HostEffect> {
        Vec::new()
    }

    /// Definition lookup. Hosts must explicitly supply types for variable operations.
    fn var_type(&mut self, domain: VarScope, id: u16) -> VmResult<VarLane> {
        Err(VmError::UnknownCommand {
            command: format!("variable type {domain:?}:{id}"),
        })
    }

    /// Context-aware engine dispatch. The host owns argument consumption and results.
    fn trap_context(
        &mut self,
        context: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        strs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        dispatch_checked(self, context, ints, strs, longs)
    }

    /// Nullable object dispatch. Hosts can handle null-aware commands directly;
    /// string-only handlers receive their non-null suffix while lower null slots
    /// remain on the VM stack.
    fn trap_objects_context(
        &mut self,
        context: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        with_string_stack(context.command, objects, |strings| {
            self.trap_context(context, ints, strings, longs)
        })
    }

    /// Explicit imported host semantics. Unimplemented operations fail; they
    /// never fall through to a wire command with different behavior.
    fn trap_operation_context(
        &mut self,
        operation: HostOperation,
        _context: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objects: &mut Vec<Option<String>>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Err(VmError::UnknownCommand {
            command: operation.spelling(),
        })
    }

    /// Imported resource dispatch. The bytes are pinned by the script identity;
    /// resource owners may cache their decoded image for the session.
    fn trap_resource_context(
        &mut self,
        operation: HostOperation,
        context: &InstructionContext<'_>,
        _resource: Option<&crate::execution::Resource>,
        ints: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        self.trap_operation_context(operation, context, ints, objects, longs)
    }

    /// Definition-owned integer default, independent of the currently stored
    /// value. Imported reads use this to retain their bound default contract.
    fn var_integer_default(&mut self, domain: VarScope, id: u16) -> VmResult<i32> {
        Err(VmError::UnknownCommand {
            command: format!("variable default {domain:?}:{id}"),
        })
    }
    /// Read a var (int/long/string by stored type).
    fn var_get(&mut self, domain: VarScope, id: u16, transmog: bool) -> VmResult<Value>;
    /// Write a var.
    fn var_set(&mut self, domain: VarScope, id: u16, transmog: bool, value: Value) -> VmResult<()>;
    /// Read a varbit (always int).
    fn varbit_get(&mut self, id: u16, transmog: bool) -> VmResult<i32>;
    /// Write a varbit.
    fn varbit_set(&mut self, id: u16, transmog: bool, value: i32) -> VmResult<()>;
    /// Create/resize an int array (all elements zeroed).
    fn array_define(&mut self, array_id: i32, len: usize) -> VmResult<()>;
    /// Current array length.
    fn array_len(&mut self, array_id: i32) -> VmResult<usize>;
    /// Read one element (bounds-checked).
    fn array_get(&mut self, array_id: i32, index: i32) -> VmResult<i32>;
    /// Write one element (bounds-checked).
    fn array_set(&mut self, array_id: i32, index: i32, value: i32) -> VmResult<()>;
    /// Engine-opcode fallback: mutate the three stacks directly for full
    /// stack effects, and/or return one value for the VM to push. The default
    /// is an explicit [`VmError::UnknownCommand`] — never a silent no-op.
    fn trap(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        strs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let _ = (ints, strs, longs);
        Err(VmError::UnknownCommand {
            command: command.to_string(),
        })
    }
}

/// Adapt a string-only handler without converting null slots to empty strings.
/// Only the suffix above the last null is exposed. A handler attempting to pop
/// through that boundary reports the null dereference. Lower slots are preserved
/// and the handler's remaining suffix is returned to the stack.
pub fn with_string_stack<T>(
    command: &str,
    objects: &mut Vec<Option<String>>,
    run: impl FnOnce(&mut Vec<String>) -> VmResult<T>,
) -> VmResult<T> {
    let boundary = objects
        .iter()
        .rposition(Option::is_none)
        .map_or(0, |i| i + 1);
    let mut strings: Vec<_> = objects.split_off(boundary).into_iter().flatten().collect();
    let result = run(&mut strings);
    objects.extend(strings.into_iter().map(Some));
    match result {
        Err(VmError::StackUnderflow { stack: "object" }) if boundary != 0 => {
            Err(VmError::NullObject {
                command: command.into(),
            })
        }
        result => result,
    }
}

/// Dispatch without consuming arguments, checking established stack contracts.
pub fn dispatch_checked<H: Host + ?Sized>(
    host: &mut H,
    context: &InstructionContext<'_>,
    ints: &mut Vec<i32>,
    strs: &mut Vec<String>,
    longs: &mut Vec<i64>,
) -> VmResult<Option<Value>> {
    let before = [ints.len(), strs.len(), longs.len()];
    let shape = crate::semantics::effect(context.command, context.operand);
    if let crate::semantics::Effect::Fixed { pops, .. } = shape {
        for ((have, need), stack) in before.into_iter().zip(pops).zip(["int", "object", "long"]) {
            if have < usize::from(need) {
                return Err(VmError::StackUnderflow { stack });
            }
        }
    }
    let result = host.trap(context.command, ints, strs, longs)?;
    if let crate::semantics::Effect::Fixed { pops, pushes } = shape {
        let mut after = [ints.len(), strs.len(), longs.len()];
        match &result {
            Some(Value::Int(_)) => after[0] += 1,
            Some(Value::Str(_) | Value::Null) => after[1] += 1,
            Some(Value::Long(_)) => after[2] += 1,
            None => {}
        }
        for lane in 0..3 {
            if after[lane] != before[lane] - usize::from(pops[lane]) + usize::from(pushes[lane]) {
                return Err(VmError::BadOperand {
                    command: context.command.to_string(),
                    expected: "host result matching semantic contract",
                });
            }
        }
    }
    Ok(result)
}

fn dispatch_operation_checked<H: Host + ?Sized>(
    host: &mut H,
    operation: HostOperation,
    resource: Option<&crate::execution::Resource>,
    context: &InstructionContext<'_>,
    ints: &mut Vec<i32>,
    objects: &mut Vec<Option<String>>,
    longs: &mut Vec<i64>,
) -> VmResult<Option<Value>> {
    if context.command != operation.command() {
        return Err(VmError::BadOperand {
            command: context.command.into(),
            expected: "matching host operation consumer",
        });
    }
    let before = [ints.len(), objects.len(), longs.len()];
    let crate::semantics::Effect::Fixed { pops, pushes } = operation.effect(context.operand) else {
        return Err(VmError::BadOperand {
            command: context.command.into(),
            expected: "fixed host operation contract",
        });
    };
    for ((have, need), stack) in before.into_iter().zip(pops).zip(["int", "object", "long"]) {
        if have < usize::from(need) {
            return Err(VmError::StackUnderflow { stack });
        }
    }
    let result = if let HostOperation::VariableBit {
        shift,
        mask,
        default,
    } = operation
    {
        let Operand::VarRef(variable) = context.operand else {
            return Err(VmError::BadOperand {
                command: context.command.into(),
                expected: "bound base variable reference",
            });
        };
        if host.var_integer_default(variable.domain, variable.id)? != default {
            return Err(VmError::BadOperand {
                command: context.command.into(),
                expected: "unchanged bound variable default",
            });
        }
        let Value::Int(value) = host.var_get(variable.domain, variable.id, variable.transmog)?
        else {
            return Err(VmError::BadOperand {
                command: context.command.into(),
                expected: "integer base variable value",
            });
        };
        Some(Value::Int(value.wrapping_shr(u32::from(shift)) & mask))
    } else {
        host.trap_resource_context(operation, context, resource, ints, objects, longs)?
    };
    let mut after = [ints.len(), objects.len(), longs.len()];
    match &result {
        Some(Value::Int(_)) => after[0] += 1,
        Some(Value::Str(_) | Value::Null) => after[1] += 1,
        Some(Value::Long(_)) => after[2] += 1,
        None => {}
    }
    if after.into_iter().enumerate().any(|(lane, count)| {
        count != before[lane] - usize::from(pops[lane]) + usize::from(pushes[lane])
    }) {
        return Err(VmError::BadOperand {
            command: context.command.into(),
            expected: "host operation result matching semantic contract",
        });
    }
    Ok(result)
}

/// Gosub callee resolution.
pub trait ScriptProvider {
    /// Owned callee script, or `None` when absent. A decode failure is an
    /// error; resolution errors must not become absence.
    fn resolve(&self, id: i32) -> VmResult<Option<CompiledScript>>;
    /// Accounting for compiler-generated wrappers/adapters. Ordinary scripts
    /// count every instruction and call. Providers validate emitted identities.
    fn accounting(&self, _id: i32) -> VmResult<Accounting> {
        Ok(Accounting::default())
    }
}

impl<S: std::hash::BuildHasher> ScriptProvider for HashMap<i32, CompiledScript, S> {
    fn resolve(&self, id: i32) -> VmResult<Option<CompiledScript>> {
        Ok(self.get(&id).cloned())
    }
}

/// Decoded scripts with verified compiler accounting, used by tooling.
#[derive(Default)]
pub struct Programs {
    pub scripts: HashMap<i32, CompiledScript>,
    pub accounting: HashMap<i32, Accounting>,
}
impl ScriptProvider for Programs {
    fn resolve(&self, id: i32) -> VmResult<Option<CompiledScript>> {
        Ok(self.scripts.get(&id).cloned())
    }
    fn accounting(&self, id: i32) -> VmResult<Accounting> {
        Ok(self.accounting.get(&id).cloned().unwrap_or_default())
    }
}

impl ScriptProvider for () {
    fn resolve(&self, _id: i32) -> VmResult<Option<CompiledScript>> {
        Ok(None)
    }
}

/// The interpreter. Owns no state between runs; each [`execute`](Self::execute)
/// builds fresh stacks/locals/frames.
pub struct Vm<'h, 'p, H: Host, P: ScriptProvider> {
    host: &'h mut H,
    provider: &'p P,
}

impl<'h, 'p, H: Host, P: ScriptProvider> Vm<'h, 'p, H, P> {
    /// Borrow a host and a provider.
    pub fn new(host: &'h mut H, provider: &'p P) -> Self {
        Self { host, provider }
    }

    /// Run `script` with `args` (split per type against the header) and return
    /// the top leftover stack value, preferring int, then object, then long —
    /// or `None` when every stack is empty.
    pub fn execute(&mut self, script: &CompiledScript, args: &[Value]) -> VmResult<Option<Value>> {
        let mut session = Session::new(script, args)?;
        while !self.step(&mut session)? {}
        Ok(return_value(&session.state))
    }

    /// Execute one instruction. Returns true when the session is finished.
    /// A failed session is poisoned and cannot resume partially applied effects.
    pub fn step(&mut self, session: &mut Session) -> VmResult<bool> {
        if session.failed {
            return Err(VmError::UnknownCommand {
                command: "cannot resume failed session".to_string(),
            });
        }
        if session.finished() {
            return Ok(true);
        }
        session.last_instruction = Some((session.state.script_id, session.state.pc));
        let result = self.step_inner(session);
        session.effects = self.host.take_effects();
        if result.is_err() {
            session.failed = true;
        }
        result
    }

    fn step_inner(&mut self, session: &mut Session) -> VmResult<bool> {
        let state = &mut session.state;
        if !state.accounting_initialized {
            if let Some(id) = state.script_id {
                state.accounting = self.provider.accounting(id)?;
                if state.accounting != Accounting::default()
                    && self.provider.resolve(id)?.as_ref() != Some(&state.cur)
                {
                    return Err(VmError::BadOperand {
                        command: "entry execution".into(),
                        expected: "root script matching its generated identity",
                    });
                }
            }
            state
                .accounting
                .validate_script(&state.cur)
                .map_err(|_| VmError::BadOperand {
                    command: "entry execution".into(),
                    expected: "imported script with its required execution metadata",
                })?;
            state.accounting_initialized = true;
        }
        if state.accounting.role == Role::Source {
            session.steps += 1;
            OPCOUNT.with(|count| count.set(count.get().wrapping_add(1)));
            if session.steps > session.step_limit {
                return Err(VmError::StepLimit);
            }
        }
        let instruction = state
            .cur
            .code
            .get(state.pc)
            .ok_or_else(|| VmError::BadBranch {
                target: slot_as_i32(state.pc),
            })?
            .clone();
        let returning = instruction.command == "return" && state.frames.is_empty();
        let cmd = instruction.command.as_str();
        let operand = &instruction.operand;
        let implemented = if state.accounting.host_operations.contains_key(&state.pc) {
            false
        } else {
            match crate::semantics::execution_family(cmd) {
                crate::semantics::ExecutionFamily::Trap => {
                    return Err(VmError::RetailTrap {
                        command: cmd.into(),
                    });
                }
                crate::semantics::ExecutionFamily::Control => {
                    handle_control(state, self.provider, cmd, operand)?
                }
                crate::semantics::ExecutionFamily::Data => {
                    handle_data(state, self.host, cmd, operand)?
                }
                crate::semantics::ExecutionFamily::Integer => {
                    handle_int_arith(state, cmd, operand)?
                }
                crate::semantics::ExecutionFamily::String => handle_string(state, cmd, operand)?,
                crate::semantics::ExecutionFamily::HostRequired => false,
            }
        };
        if !implemented {
            let context = InstructionContext {
                script_name: state.cur.name.as_deref(),
                script_id: state.script_id,
                event: session.event.as_deref(),
                pc: state.pc,
                command: cmd,
                operand,
                secondary: matches!(operand, Operand::Byte(1) | Operand::Int(1)),
                int_locals: &state.int_locals,
            };
            let result = if let Some(operation) = state.accounting.host_operations.get(&state.pc) {
                dispatch_operation_checked(
                    self.host,
                    *operation,
                    state.accounting.resource.as_ref(),
                    &context,
                    &mut state.ints,
                    &mut state.objs,
                    &mut state.longs,
                )?
            } else {
                self.host.trap_objects_context(
                    &context,
                    &mut state.ints,
                    &mut state.objs,
                    &mut state.longs,
                )?
            };
            if let Some(value) = result {
                push_value(state, value)?;
            }
            if state.ints.len() > MAX_STACK
                || state.objs.len() > MAX_STACK
                || state.longs.len() > MAX_STACK
            {
                return Err(VmError::StackFull { stack: "host" });
            }
            state.pc += 1;
        }
        if returning {
            session.halted = true;
        }
        Ok(session.finished())
    }
}

/// A resumable execution. Breakpoints are implemented by inspecting before step.
pub struct Session {
    state: RunState,
    steps: usize,
    step_limit: usize,
    failed: bool,
    halted: bool,
    event: Option<String>,
    effects: Vec<HostEffect>,
    last_instruction: Option<(Option<i32>, usize)>,
}

/// Fully initialized event locals. A null object local remains distinct from
/// an empty string; hook setup ignores null/non-scalar argument elements.
pub struct HookLocals {
    pub ints: Vec<i32>,
    pub strings: Vec<Option<String>>,
    pub longs: Vec<i64>,
}

impl HookLocals {
    /// Bind typed arguments directly into fresh locals without operand-stack setup.
    fn from_arguments(script: &CompiledScript, args: &[Value]) -> VmResult<Self> {
        let mut int_args: Vec<i32> = Vec::new();
        let mut obj_args: Vec<Option<String>> = Vec::new();
        let mut long_args: Vec<i64> = Vec::new();
        for value in args {
            match value {
                Value::Int(v) => int_args.push(*v),
                Value::Str(s) => obj_args.push(Some(s.clone())),
                Value::Null => obj_args.push(None),
                Value::Long(v) => long_args.push(*v),
            }
        }
        let want_i = usize::from(script.args.int);
        let want_o = usize::from(script.args.obj);
        let want_l = usize::from(script.args.long);
        if int_args.len() != want_i || obj_args.len() != want_o || long_args.len() != want_l {
            return Err(VmError::ArityMismatch {
                expected: format!("({want_i}i,{want_o}o,{want_l}l)"),
                got: format!(
                    "({}i,{}o,{}l)",
                    int_args.len(),
                    obj_args.len(),
                    long_args.len()
                ),
            });
        }
        let total_i = usize::from(script.locals.int);
        let total_o = usize::from(script.locals.obj);
        let total_l = usize::from(script.locals.long);
        if want_i > total_i || want_o > total_o || want_l > total_l {
            return Err(VmError::BadOperand {
                command: "script header".into(),
                expected: "argument counts within total local counts",
            });
        }
        let mut int_locals = vec![0_i32; total_i];
        let mut obj_locals = vec![None; total_o];
        let mut long_locals = vec![0_i64; total_l];
        for (slot, value) in int_args.iter().enumerate() {
            if let Some(cell) = int_locals.get_mut(slot) {
                *cell = *value;
            }
        }
        for (slot, value) in obj_args.iter().enumerate() {
            if let Some(cell) = obj_locals.get_mut(slot) {
                cell.clone_from(value);
            }
        }
        for (slot, value) in long_args.iter().enumerate() {
            if let Some(cell) = long_locals.get_mut(slot) {
                *cell = *value;
            }
        }
        Ok(Self {
            ints: int_locals,
            strings: obj_locals,
            longs: long_locals,
        })
    }
}

/// Owned debugger snapshot. Instruction indices identify the next instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub script_name: Option<String>,
    pub script_id: Option<i32>,
    pub pc: usize,
    pub ints: Vec<i32>,
    pub strings: Vec<Option<String>>,
    pub longs: Vec<i64>,
    pub int_locals: Vec<i32>,
    pub string_locals: Vec<Option<String>>,
    pub long_locals: Vec<i64>,
    pub frames: Vec<(Option<String>, usize)>,
    pub steps: usize,
    pub failed: bool,
    pub effects: Vec<HostEffect>,
    pub last_instruction: Option<(Option<i32>, usize)>,
}

impl Session {
    pub fn new(script: &CompiledScript, args: &[Value]) -> VmResult<Self> {
        OPCOUNT.with(|count| count.set(0));
        Ok(Self {
            state: RunState::setup(script, args)?,
            steps: 0,
            step_limit: MAX_STEPS,
            failed: false,
            halted: false,
            event: None,
            effects: Vec::new(),
            last_instruction: None,
        })
    }
    /// Hook entry fills complete local arrays without consulting the
    /// argument-count header. Execution resets the int/object stacks but
    /// retains the pooled state's long stack, and receives its own instruction
    /// budget (500,000 normally; 5,000,000 for onloads).
    pub fn for_hook(
        id: i32,
        script: &CompiledScript,
        locals: HookLocals,
        retained_longs: &[i64],
        step_limit: usize,
        event: Option<String>,
    ) -> VmResult<Self> {
        if locals.ints.len() != usize::from(script.locals.int)
            || locals.strings.len() != usize::from(script.locals.obj)
            || locals.longs.len() != usize::from(script.locals.long)
        {
            return Err(VmError::BadOperand {
                command: "hook locals".into(),
                expected: "complete typed local arrays",
            });
        }
        if retained_longs.len() > MAX_STACK {
            return Err(VmError::StackFull { stack: "long" });
        }
        OPCOUNT.with(|count| count.set(0));
        Ok(Self {
            state: RunState {
                cur: script.clone(),
                script_id: Some(id),
                pc: 0,
                int_locals: locals.ints,
                obj_locals: locals.strings,
                long_locals: locals.longs,
                ints: Vec::with_capacity(MAX_STACK),
                objs: Vec::with_capacity(MAX_STACK),
                longs: retained_longs.to_vec(),
                frames: Vec::new(),
                source_frames: usize::default(),
                accounting: Accounting::default(),
                accounting_initialized: false,
                entry_arguments: Vec::new(),
            },
            steps: 0,
            step_limit,
            failed: false,
            halted: false,
            event,
            effects: Vec::new(),
            last_instruction: None,
        })
    }
    pub fn retained_longs(&self) -> &[i64] {
        &self.state.longs
    }
    /// Set root cache identity and opaque event context before execution.
    pub fn for_script(
        id: i32,
        script: &CompiledScript,
        args: &[Value],
        event: Option<String>,
    ) -> VmResult<Self> {
        let mut session = Self::new(script, args)?;
        session.state.script_id = Some(id);
        session.event = event;
        Ok(session)
    }
    pub fn finished(&self) -> bool {
        self.halted
    }
    pub fn snapshot(&self) -> Snapshot {
        let state = &self.state;
        Snapshot {
            script_name: state.cur.name.clone(),
            script_id: state.script_id,
            pc: state.pc,
            ints: state.ints.clone(),
            strings: state.objs.clone(),
            longs: state.longs.clone(),
            int_locals: state.int_locals.clone(),
            string_locals: state.obj_locals.clone(),
            long_locals: state.long_locals.clone(),
            frames: state
                .frames
                .iter()
                .map(|frame| (frame.script.name.clone(), frame.pc))
                .collect(),
            steps: self.steps,
            failed: self.failed,
            effects: self.effects.clone(),
            last_instruction: self.last_instruction,
        }
    }
}

/// Machine limits: stack size and the per-execution step guard.
const MAX_STACK: usize = 1000;
/// The call-frame table holds 50 frames.
const MAX_FRAMES: usize = 50;
/// `runHook` passes 500_000; `executeOnLoadComponents` passes 5_000_000.
/// Ordinary entry uses the interactive budget; hook entry supplies its budget.
const MAX_STEPS: usize = 500_000;
/// `define_array` rejects `len < 0 || len > 5000`.
const MAX_ARRAY_LEN: usize = 5000;

/// One saved caller (as saved by `gosub_with_params`).
struct Frame {
    script: CompiledScript,
    script_id: Option<i32>,
    pc: usize,
    int_locals: Vec<i32>,
    obj_locals: Vec<Option<String>>,
    long_locals: Vec<i64>,
    accounting: Accounting,
    source_call: bool,
    entry_arguments: Vec<Value>,
}

/// Per-run machine: current script + pc + typed locals/stacks/frames.
struct RunState {
    cur: CompiledScript,
    script_id: Option<i32>,
    pc: usize,
    int_locals: Vec<i32>,
    obj_locals: Vec<Option<String>>,
    long_locals: Vec<i64>,
    ints: Vec<i32>,
    objs: Vec<Option<String>>,
    longs: Vec<i64>,
    frames: Vec<Frame>,
    source_frames: usize,
    accounting: Accounting,
    accounting_initialized: bool,
    entry_arguments: Vec<Value>,
}

impl RunState {
    /// Allocate the total local count per type (`source::lower` lays
    /// args first) and copy the per-type argument prefix in. Counts must match
    /// exactly for this ordinary-call entry. Event hooks use
    /// `Session::for_hook`, which installs locals independently of this header.
    fn setup(script: &CompiledScript, args: &[Value]) -> VmResult<Self> {
        let locals = HookLocals::from_arguments(script, args)?;
        Ok(Self {
            cur: script.clone(),
            script_id: None,
            pc: 0,
            int_locals: locals.ints,
            obj_locals: locals.strings,
            long_locals: locals.longs,
            ints: Vec::with_capacity(MAX_STACK),
            objs: Vec::with_capacity(MAX_STACK),
            longs: Vec::with_capacity(MAX_STACK),
            frames: Vec::new(),
            source_frames: usize::default(),
            accounting: Accounting::default(),
            accounting_initialized: false,
            entry_arguments: Vec::new(),
        })
    }
}

fn return_value(state: &RunState) -> Option<Value> {
    if let Some(top) = state.ints.last() {
        return Some(Value::Int(*top));
    }
    if let Some(top) = state.objs.last() {
        return Some(top.clone().map_or(Value::Null, Value::Str));
    }
    state.longs.last().map(|top| Value::Long(*top))
}

fn push_value(state: &mut RunState, value: Value) -> VmResult<()> {
    match value {
        Value::Int(v) => push_int(state, v),
        Value::Str(s) => push_obj(state, s),
        Value::Long(v) => push_long(state, v),
        Value::Null => push_object(state, None),
    }
}

fn push_int(state: &mut RunState, value: i32) -> VmResult<()> {
    if state.ints.len() >= MAX_STACK {
        return Err(VmError::StackFull { stack: "int" });
    }
    state.ints.push(value);
    Ok(())
}

fn pop_int(state: &mut RunState) -> VmResult<i32> {
    state
        .ints
        .pop()
        .ok_or(VmError::StackUnderflow { stack: "int" })
}

fn push_obj(state: &mut RunState, value: String) -> VmResult<()> {
    push_object(state, Some(value))
}

fn push_object(state: &mut RunState, value: Option<String>) -> VmResult<()> {
    if state.objs.len() >= MAX_STACK {
        return Err(VmError::StackFull { stack: "object" });
    }
    state.objs.push(value);
    Ok(())
}

fn pop_object(state: &mut RunState) -> VmResult<Option<String>> {
    state
        .objs
        .pop()
        .ok_or(VmError::StackUnderflow { stack: "object" })
}

fn pop_string(state: &mut RunState, command: &str) -> VmResult<String> {
    pop_object(state)?.ok_or_else(|| VmError::NullObject {
        command: command.into(),
    })
}

fn pop_text(state: &mut RunState) -> VmResult<String> {
    Ok(pop_object(state)?.unwrap_or_else(|| "null".into()))
}

fn push_long(state: &mut RunState, value: i64) -> VmResult<()> {
    if state.longs.len() >= MAX_STACK {
        return Err(VmError::StackFull { stack: "long" });
    }
    state.longs.push(value);
    Ok(())
}

fn pop_long(state: &mut RunState) -> VmResult<i64> {
    state
        .longs
        .pop()
        .ok_or(VmError::StackUnderflow { stack: "long" })
}

/// Resolve a local operand to a checked slot.
fn local_slot(operand: &Operand, command: &str) -> VmResult<usize> {
    let slot = match operand {
        Operand::Local(slot) | Operand::Int(slot) => *slot,
        _ => {
            return Err(VmError::BadOperand {
                command: command.to_string(),
                expected: "Local",
            });
        }
    };
    usize::try_from(slot).map_err(|_| VmError::BadLocal { slot, kind: "flat" })
}

fn read_local(state: &RunState, command: &str, operand: &Operand) -> VmResult<Value> {
    let slot = local_slot(operand, command)?;
    let (value, kind) = match command {
        "push_int_local" => (state.int_locals.get(slot).copied().map(Value::Int), "int"),
        "push_string_local" => (
            state
                .obj_locals
                .get(slot)
                .cloned()
                .map(|value| value.map_or(Value::Null, Value::Str)),
            "object",
        ),
        "push_long_local" => (
            state.long_locals.get(slot).copied().map(Value::Long),
            "long",
        ),
        _ => {
            return Err(VmError::BadOperand {
                command: command.into(),
                expected: "typed local read",
            });
        }
    };
    value.ok_or_else(|| VmError::BadLocal {
        slot: slot_as_i32(slot),
        kind,
    })
}

fn branch_target(cur_len: usize, target: i32) -> VmResult<usize> {
    if target < 0 {
        return Err(VmError::BadBranch { target });
    }
    let index = usize::try_from(target).map_err(|_| VmError::BadBranch { target })?;
    if index > cur_len {
        return Err(VmError::BadBranch { target });
    }
    Ok(index)
}

/// Control flow for real: unconditional/conditional int branches, long
/// branches, `branch_if_true/false`, `switch`, `gosub_with_params` and
/// `return` (a return from the root frame exits). Absolute targets jump
/// directly; a target of `code.len()` exits.
fn handle_control(
    state: &mut RunState,
    provider: &impl ScriptProvider,
    cmd: &str,
    operand: &Operand,
) -> VmResult<bool> {
    match cmd {
        "branch" => {
            let target = expect_branch(cmd, operand)?;
            state.pc = branch_target(state.cur.code.len(), target)?;
            Ok(true)
        }
        "branch_not" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            if a == b {
                state.pc = state.pc.saturating_add(1);
            } else {
                state.pc = branch_target(state.cur.code.len(), target)?;
            }
            Ok(true)
        }
        "branch_equals" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            if a == b {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "branch_less_than" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            if a < b {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "branch_greater_than" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            if a > b {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "branch_less_than_or_equals" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            if a <= b {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "branch_greater_than_or_equals" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            if a >= b {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "long_branch_not" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_long(state)?;
            let a = pop_long(state)?;
            if a == b {
                state.pc = state.pc.saturating_add(1);
            } else {
                state.pc = branch_target(state.cur.code.len(), target)?;
            }
            Ok(true)
        }
        "long_branch_equals" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_long(state)?;
            let a = pop_long(state)?;
            if a == b {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "long_branch_less_than" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_long(state)?;
            let a = pop_long(state)?;
            if a < b {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "long_branch_greater_than" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_long(state)?;
            let a = pop_long(state)?;
            if a > b {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "long_branch_less_than_or_equals" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_long(state)?;
            let a = pop_long(state)?;
            if a <= b {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "long_branch_greater_than_or_equals" => {
            let target = expect_branch(cmd, operand)?;
            let b = pop_long(state)?;
            let a = pop_long(state)?;
            if a >= b {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "branch_if_true" => {
            let target = expect_branch(cmd, operand)?;
            let value = pop_int(state)?;
            if value == 1 {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "branch_if_false" => {
            let target = expect_branch(cmd, operand)?;
            let value = pop_int(state)?;
            if value == 0 {
                state.pc = branch_target(state.cur.code.len(), target)?;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "switch" => {
            let Operand::Switch(cases) = operand else {
                return Err(VmError::BadOperand {
                    command: cmd.to_string(),
                    expected: "Switch",
                });
            };
            let key = pop_int(state)?;
            // First-wins linear scan over the absolute table.
            let mut jumped: Option<usize> = None;
            for case in cases {
                if case.value == key {
                    jumped = Some(branch_target(state.cur.code.len(), case.target)?);
                    break;
                }
            }
            if let Some(target) = jumped {
                state.pc = target;
            } else {
                state.pc = state.pc.saturating_add(1);
            }
            Ok(true)
        }
        "gosub_with_params" => {
            let id = match operand {
                Operand::Script(id) | Operand::Int(id) => *id,
                _ => {
                    return Err(VmError::BadOperand {
                        command: cmd.to_string(),
                        expected: "Script",
                    });
                }
            };
            let Some(callee) = provider.resolve(id)? else {
                return Err(VmError::UnknownScript { id });
            };
            let accounting = provider.accounting(id)?;
            accounting
                .validate_script(&callee)
                .map_err(|_| VmError::BadOperand {
                    command: cmd.into(),
                    expected: "callee with its required execution metadata",
                })?;
            if state.accounting.transparent_calls.contains(&state.pc) {
                let expected = match state.accounting.role {
                    Role::Source => Role::Adapter,
                    Role::Entry => Role::Source,
                    Role::Adapter => {
                        return Err(VmError::BadOperand {
                            command: cmd.into(),
                            expected: "adapter without calls",
                        });
                    }
                };
                if accounting.role != expected {
                    return Err(VmError::BadOperand {
                        command: cmd.into(),
                        expected: "linked generated execution identity",
                    });
                }
            }
            gosub(state, id, &callee, accounting)?;
            Ok(true)
        }
        "return" => {
            if state.frames.is_empty() {
                state.pc = state.cur.code.len();
            } else {
                let frame = state
                    .frames
                    .pop()
                    .ok_or(VmError::StackUnderflow { stack: "frame" })?;
                state.cur = frame.script;
                state.script_id = frame.script_id;
                state.pc = frame.pc;
                state.int_locals = frame.int_locals;
                state.obj_locals = frame.obj_locals;
                state.long_locals = frame.long_locals;
                state.accounting = frame.accounting;
                state.source_frames -= usize::from(frame.source_call);
                state.entry_arguments = frame.entry_arguments;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn expect_branch(cmd: &str, operand: &Operand) -> VmResult<i32> {
    match operand {
        Operand::Branch(target) => Ok(*target),
        _ => Err(VmError::BadOperand {
            command: cmd.to_string(),
            expected: "Branch",
        }),
    }
}

/// Callee locals are fresh zero/empty arrays sized by total local counts; the first
/// `args` slots of each type copy off the typed stacks oldest-first, then the
/// stacks shrink (`gosub_with_params`).
fn gosub(
    state: &mut RunState,
    id: i32,
    callee: &CompiledScript,
    accounting: Accounting,
) -> VmResult<()> {
    if state.accounting.role == Role::Entry {
        let locals = HookLocals::from_arguments(callee, &state.entry_arguments)?;
        state.entry_arguments.clear();
        return enter_callee(state, id, callee, accounting, locals);
    }
    let want_i = usize::from(callee.args.int);
    let want_o = usize::from(callee.args.obj);
    let want_l = usize::from(callee.args.long);
    if state.ints.len() < want_i {
        return Err(VmError::StackUnderflow { stack: "int" });
    }
    if state.objs.len() < want_o {
        return Err(VmError::StackUnderflow { stack: "object" });
    }
    if state.longs.len() < want_l {
        return Err(VmError::StackUnderflow { stack: "long" });
    }
    let total_i = usize::from(callee.locals.int);
    let total_o = usize::from(callee.locals.obj);
    let total_l = usize::from(callee.locals.long);
    if want_i > total_i || want_o > total_o || want_l > total_l {
        return Err(VmError::BadOperand {
            command: "callee header".into(),
            expected: "argument counts within total local counts",
        });
    }
    let mut int_locals = vec![0_i32; total_i];
    let mut obj_locals = vec![None; total_o];
    let mut long_locals = vec![0_i64; total_l];
    let base_i = state.ints.len().saturating_sub(want_i);
    for (offset, slot) in (base_i..state.ints.len()).enumerate() {
        if let (Some(dst), Some(src)) = (int_locals.get_mut(offset), state.ints.get(slot)) {
            *dst = *src;
        }
    }
    let base_o = state.objs.len().saturating_sub(want_o);
    for (offset, slot) in (base_o..state.objs.len()).enumerate() {
        if let (Some(dst), Some(src)) = (obj_locals.get_mut(offset), state.objs.get(slot)) {
            dst.clone_from(src);
        }
    }
    let base_l = state.longs.len().saturating_sub(want_l);
    for (offset, slot) in (base_l..state.longs.len()).enumerate() {
        if let (Some(dst), Some(src)) = (long_locals.get_mut(offset), state.longs.get(slot)) {
            *dst = *src;
        }
    }
    state.ints.truncate(base_i);
    state.objs.truncate(base_o);
    state.longs.truncate(base_l);
    enter_callee(
        state,
        id,
        callee,
        accounting,
        HookLocals {
            ints: int_locals,
            strings: obj_locals,
            longs: long_locals,
        },
    )
}

fn enter_callee(
    state: &mut RunState,
    id: i32,
    callee: &CompiledScript,
    accounting: Accounting,
    locals: HookLocals,
) -> VmResult<()> {
    let source_call = !state.accounting.transparent_calls.contains(&state.pc);
    if source_call && state.source_frames >= MAX_FRAMES {
        return Err(VmError::FrameOverflow);
    }
    state.source_frames += usize::from(source_call);
    state.frames.push(Frame {
        script: std::mem::replace(&mut state.cur, callee.clone()),
        script_id: state.script_id.replace(id),
        pc: state.pc.saturating_add(1),
        int_locals: std::mem::replace(&mut state.int_locals, locals.ints),
        obj_locals: std::mem::replace(&mut state.obj_locals, locals.strings),
        long_locals: std::mem::replace(&mut state.long_locals, locals.longs),
        accounting: std::mem::replace(&mut state.accounting, accounting),
        source_call,
        entry_arguments: std::mem::take(&mut state.entry_arguments),
    });
    state.pc = 0;
    Ok(())
}

/// Data movement for real: typed constants (`push_constant_int`,
/// `push_constant_string` with the `0/1/2` int/long/string tags,
/// `push_long_constant`), typed locals, discards, vars/varbits through
/// [`Host`], `join_string`, and all six array spellings (`define_array`,
/// `push_array`, `pop_array` with dispatch `69/186/945 -> push` and
/// `529/531 -> pop`).
fn handle_data(
    state: &mut RunState,
    host: &mut impl Host,
    cmd: &str,
    operand: &Operand,
) -> VmResult<bool> {
    // Export bindings are local initialization, independent of the source
    // operand stacks. In particular, a hook's pooled long stack stays intact.
    let component_setup = state.accounting.role == Role::Entry
        && state
            .accounting
            .host_operations
            .get(&crate::execution::ENTRY_COMPONENT_SELECT)
            == Some(&crate::execution::HostOperation::FindComponent)
        && state.pc <= crate::execution::ENTRY_COMPONENT_DISCARD;
    if state.accounting.role == Role::Entry && !component_setup {
        let value = match cmd {
            "push_constant_string" => match operand {
                Operand::Int(value) => Value::Int(*value),
                Operand::Long(value) => Value::Long(*value),
                Operand::Str(value) => Value::Str(value.clone()),
                _ => {
                    return Err(VmError::BadOperand {
                        command: cmd.into(),
                        expected: "Int/Long/Str",
                    });
                }
            },
            "push_int_local" | "push_string_local" | "push_long_local" => {
                read_local(state, cmd, operand)?
            }
            _ => {
                return Err(VmError::BadOperand {
                    command: cmd.into(),
                    expected: "linear entry argument binding",
                });
            }
        };
        state.entry_arguments.push(value);
        state.pc = state.pc.saturating_add(1);
        return Ok(true);
    }
    match cmd {
        "push_constant_int" => {
            let Operand::Int(value) = operand else {
                return Err(VmError::BadOperand {
                    command: cmd.to_string(),
                    expected: "Int",
                });
            };
            push_int(state, *value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "push_long_constant" => {
            let Operand::Long(value) = operand else {
                return Err(VmError::BadOperand {
                    command: cmd.to_string(),
                    expected: "Long",
                });
            };
            push_long(state, *value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "push_constant_string" => {
            match operand {
                Operand::Int(value) => push_int(state, *value)?,
                Operand::Long(value) => push_long(state, *value)?,
                Operand::Str(text) => push_obj(state, text.clone())?,
                _ => {
                    return Err(VmError::BadOperand {
                        command: cmd.to_string(),
                        expected: "Int/Long/Str",
                    });
                }
            }
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "push_int_local" => {
            let value = read_local(state, cmd, operand)?;
            push_value(state, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "pop_int_local" => {
            let slot = local_slot(operand, cmd)?;
            let value = pop_int(state)?;
            let cell = state
                .int_locals
                .get_mut(slot)
                .ok_or_else(|| VmError::BadLocal {
                    slot: slot_as_i32(slot),
                    kind: "int",
                })?;
            *cell = value;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "push_string_local" => {
            let value = read_local(state, cmd, operand)?;
            push_value(state, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "pop_string_local" => {
            let slot = local_slot(operand, cmd)?;
            let value = pop_object(state)?;
            let cell = state
                .obj_locals
                .get_mut(slot)
                .ok_or_else(|| VmError::BadLocal {
                    slot: slot_as_i32(slot),
                    kind: "object",
                })?;
            *cell = value;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "push_long_local" => {
            let value = read_local(state, cmd, operand)?;
            push_value(state, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "pop_long_local" => {
            let slot = local_slot(operand, cmd)?;
            let value = pop_long(state)?;
            let cell = state
                .long_locals
                .get_mut(slot)
                .ok_or_else(|| VmError::BadLocal {
                    slot: slot_as_i32(slot),
                    kind: "long",
                })?;
            *cell = value;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "pop_int_discard" | "pop_string_discard" | "pop_long_discard" => {
            match cmd {
                "pop_int_discard" => {
                    pop_int(state)?;
                }
                "pop_string_discard" => {
                    pop_object(state)?;
                }
                _ => {
                    pop_long(state)?;
                }
            }
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "push_var" => {
            let Operand::VarRef(var) = operand else {
                return Err(VmError::BadOperand {
                    command: cmd.to_string(),
                    expected: "VarRef",
                });
            };
            let value = host.var_get(var.domain, var.id, var.transmog)?;
            let kind = host.var_type(var.domain, var.id)?;
            if !matches!(
                (&value, kind),
                (Value::Int(_), VarLane::Int)
                    | (Value::Str(_) | Value::Null, VarLane::String)
                    | (Value::Long(_), VarLane::Long)
            ) {
                return Err(VmError::BadOperand {
                    command: cmd.to_string(),
                    expected: "value matching variable definition",
                });
            }
            let value = if value == Value::Null {
                Value::Str("null".into())
            } else {
                value
            };
            push_value(state, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "pop_var" => {
            let Operand::VarRef(var) = operand else {
                return Err(VmError::BadOperand {
                    command: cmd.to_string(),
                    expected: "VarRef",
                });
            };
            let value = match host.var_type(var.domain, var.id)? {
                VarLane::Int => Value::Int(pop_int(state)?),
                VarLane::String => pop_object(state)?.map_or(Value::Null, Value::Str),
                VarLane::Long => Value::Long(pop_long(state)?),
            };
            host.var_set(var.domain, var.id, var.transmog, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "push_varbit" => {
            let Operand::VarBitRef(varbit) = operand else {
                return Err(VmError::BadOperand {
                    command: cmd.to_string(),
                    expected: "VarBitRef",
                });
            };
            let value = host.varbit_get(varbit.id, varbit.transmog)?;
            push_int(state, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "pop_varbit" => {
            let Operand::VarBitRef(varbit) = operand else {
                return Err(VmError::BadOperand {
                    command: cmd.to_string(),
                    expected: "VarBitRef",
                });
            };
            let value = pop_int(state)?;
            host.varbit_set(varbit.id, varbit.transmog, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "join_string" => {
            let count = match operand {
                Operand::Count(value) | Operand::Int(value) => *value,
                _ => {
                    return Err(VmError::BadOperand {
                        command: cmd.to_string(),
                        expected: "Count",
                    });
                }
            };
            if count < 0 {
                return Err(VmError::BadOperand {
                    command: cmd.to_string(),
                    expected: "non-negative Count",
                });
            }
            let count = usize::try_from(count).map_err(|_| VmError::BadOperand {
                command: cmd.to_string(),
                expected: "Count",
            })?;
            if state.objs.len() < count {
                return Err(VmError::StackUnderflow { stack: "object" });
            }
            let base = state.objs.len().saturating_sub(count);
            let mut out = String::new();
            for slot in base..state.objs.len() {
                if let Some(part) = state.objs.get(slot) {
                    out.push_str(part.as_deref().unwrap_or("null"));
                }
            }
            state.objs.truncate(base);
            push_obj(state, crate::jstr::normalize(out))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "define_array" => {
            let encoded = expect_array(cmd, operand)?;
            let id = encoded >> 16;
            if !(0..5).contains(&id) {
                return Err(VmError::BadArray {
                    id,
                    reason: "array ID outside 0..5".into(),
                });
            }
            let len = pop_int(state)?;
            if len < 0 || usize::try_from(len).is_ok_and(|n| n > MAX_ARRAY_LEN) {
                return Err(VmError::BadArray {
                    id,
                    reason: format!("length {len} outside 0..={MAX_ARRAY_LEN}"),
                });
            }
            let len = usize::try_from(len).map_err(|_| VmError::BadArray {
                id,
                reason: format!("negative length {len}"),
            })?;
            host.array_define(id, len).map_err(|error| match error {
                VmError::BadArray { .. } => error,
                other => VmError::BadArray {
                    id,
                    reason: other.to_string(),
                },
            })?;
            if encoded & 0xffff != 0 {
                for index in 0..len {
                    host.array_set(id, index as i32, -1)?;
                }
            }
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "push_array_int" => {
            let id = expect_array(cmd, operand)?;
            let index = pop_int(state)?;
            let value = host.array_get(id, index)?;
            push_int(state, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "push_array_int_leave_index_on_stack" => {
            // Pushes the index, then the value.
            let id = expect_array(cmd, operand)?;
            let index = pop_int(state)?;
            let value = host.array_get(id, index)?;
            push_int(state, index)?;
            push_int(state, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "push_array_int_and_index" => {
            // Pushes the value, then the index.
            let id = expect_array(cmd, operand)?;
            let index = pop_int(state)?;
            let value = host.array_get(id, index)?;
            push_int(state, value)?;
            push_int(state, index)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "pop_array_int" => {
            let id = expect_array(cmd, operand)?;
            let value = pop_int(state)?;
            let index = pop_int(state)?;
            host.array_set(id, index, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "pop_array_int_leave_value_on_stack" => {
            let id = expect_array(cmd, operand)?;
            let value = pop_int(state)?;
            let index = pop_int(state)?;
            host.array_set(id, index, value)?;
            push_int(state, value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn slot_as_i32(slot: usize) -> i32 {
    i32::try_from(slot).unwrap_or(i32::MAX)
}

fn expect_array(cmd: &str, operand: &Operand) -> VmResult<i32> {
    match operand {
        Operand::Array(id) | Operand::Int(id) => Ok(*id),
        _ => Err(VmError::BadOperand {
            command: cmd.to_string(),
            expected: "Array",
        }),
    }
}

fn expect_plain(cmd: &str, operand: &Operand) -> VmResult<()> {
    match operand {
        Operand::Byte(_) | Operand::Int(_) => Ok(()),
        _ => Err(VmError::BadOperand {
            command: cmd.to_string(),
            expected: "Byte",
        }),
    }
}

/// Pure int arithmetic for real (32-bit wrapping semantics): `add`,
/// `multiply`, `divide`, `modulo`, `and`, `or`, `pow`, `invpow`, `setbit`,
/// `clearbit`, `testbit`, `setbit_range_toint`, plus the extended pure set
/// `addpercent`, `interpolate`, `scale`, `min`, `max`, `sqrt`, `abs`, `not`,
/// `bitcount`, `togglebit`, `setbit_range`, `clearbit_range` and
/// `getbit_range`. Everything needing RNG, trig tables, fonts, or language
/// (`random`, `sin_deg`, `compare`, …) traps.
fn handle_int_arith(state: &mut RunState, cmd: &str, operand: &Operand) -> VmResult<bool> {
    match cmd {
        "quickchat_dynamic_command_add" => {
            expect_plain(cmd, operand)?;
            let right = pop_int(state)?;
            let left = pop_int(state)?;
            push_int(state, left.wrapping_sub(right))?;
            state.pc += 1;
            Ok(true)
        }
        "noopDisplayCommand" => {
            state.pc += 1;
            Ok(true)
        }
        "add" => {
            expect_plain(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            push_int(state, a.wrapping_add(b))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "multiply" => {
            expect_plain(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            push_int(state, a.wrapping_mul(b))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "divide" => {
            expect_plain(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            if b == 0 {
                return Err(VmError::DivideByZero {
                    command: cmd.to_string(),
                });
            }
            push_int(state, a.wrapping_div(b))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "modulo" => {
            expect_plain(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            if b == 0 {
                return Err(VmError::DivideByZero {
                    command: cmd.to_string(),
                });
            }
            push_int(state, a.wrapping_rem(b))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "and" => {
            expect_plain(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            push_int(state, a & b)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "or" => {
            expect_plain(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            push_int(state, a | b)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "pow" => {
            expect_plain(cmd, operand)?;
            let exp = pop_int(state)?;
            let base = pop_int(state)?;
            if base == 0 {
                push_int(state, 0)?;
            } else {
                push_int(state, f64::from(base).powf(f64::from(exp)) as i32)?;
            }
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "invpow" => {
            expect_plain(cmd, operand)?;
            let exp = pop_int(state)?;
            let base = pop_int(state)?;
            if base == 0 || exp == 0 {
                push_int(state, 0)?;
            } else if exp == 1 {
                push_int(state, base)?;
            } else if exp == 2 {
                push_int(state, f64::from(base).sqrt() as i32)?;
            } else if exp == 3 {
                push_int(state, f64::from(base).cbrt() as i32)?;
            } else if exp == 4 {
                push_int(state, f64::from(base).sqrt().sqrt() as i32)?;
            } else {
                push_int(state, f64::from(base).powf(1.0 / f64::from(exp)) as i32)?;
            }
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "setbit" => {
            expect_plain(cmd, operand)?;
            let bit = pop_int(state)?;
            let value = pop_int(state)?;
            push_int(state, value | 1_i32.wrapping_shl(bit as u32))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "clearbit" => {
            expect_plain(cmd, operand)?;
            let bit = pop_int(state)?;
            let value = pop_int(state)?;
            push_int(state, value & !(1_i32.wrapping_shl(bit as u32)))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "testbit" => {
            expect_plain(cmd, operand)?;
            let bit = pop_int(state)?;
            let value = pop_int(state)?;
            push_int(
                state,
                i32::from(value & 1_i32.wrapping_shl(bit as u32) != 0),
            )?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "setbit_range_toint" => {
            expect_plain(cmd, operand)?;
            let hi = pop_int(state)?;
            let lo = pop_int(state)?;
            let mut bits = pop_int(state)?;
            let base = pop_int(state)?;
            let count = hi.wrapping_sub(lo).wrapping_add(1);
            let mask = bitmask(cmd, count)?;
            if bits > mask {
                bits = mask;
            }
            let cleared = base & !(mask.wrapping_shl(lo as u32));
            push_int(state, cleared | bits.wrapping_shl(lo as u32))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "addpercent" => {
            expect_plain(cmd, operand)?;
            let pct = pop_int(state)?;
            let base = pop_int(state)?;
            let scaled = i64::from(base)
                .wrapping_mul(i64::from(pct))
                .wrapping_div(100)
                .wrapping_add(i64::from(base)) as i32;
            push_int(state, scaled)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "interpolate" => {
            expect_plain(cmd, operand)?;
            let v5 = pop_int(state)?;
            let v4 = pop_int(state)?;
            let v3 = pop_int(state)?;
            let v2 = pop_int(state)?;
            let v1 = pop_int(state)?;
            let denom = v4.wrapping_sub(v3);
            if denom == 0 {
                return Err(VmError::DivideByZero {
                    command: cmd.to_string(),
                });
            }
            let out = v1.wrapping_add(
                v2.wrapping_sub(v1)
                    .wrapping_mul(v5.wrapping_sub(v3))
                    .wrapping_div(denom),
            );
            push_int(state, out)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "scale" => {
            expect_plain(cmd, operand)?;
            let v5 = pop_int(state)?;
            let v3 = pop_int(state)?;
            let v1 = pop_int(state)?;
            if v3 == 0 {
                return Err(VmError::DivideByZero {
                    command: cmd.to_string(),
                });
            }
            let out = (i64::from(v1)
                .wrapping_mul(i64::from(v5))
                .wrapping_div(i64::from(v3))) as i32;
            push_int(state, out)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "min" => {
            expect_plain(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            push_int(state, if a < b { a } else { b })?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "max" => {
            expect_plain(cmd, operand)?;
            let b = pop_int(state)?;
            let a = pop_int(state)?;
            push_int(state, if a > b { a } else { b })?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "sqrt" => {
            expect_plain(cmd, operand)?;
            let value = pop_int(state)?;
            push_int(state, f64::from(value).sqrt() as i32)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "abs" => {
            expect_plain(cmd, operand)?;
            let value = pop_int(state)?;
            push_int(state, value.wrapping_abs())?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "not" => {
            expect_plain(cmd, operand)?;
            let value = pop_int(state)?;
            push_int(state, !value)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "bitcount" => {
            expect_plain(cmd, operand)?;
            let value = pop_int(state)?;
            push_int(state, value.count_ones() as i32)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "togglebit" => {
            expect_plain(cmd, operand)?;
            let bit = pop_int(state)?;
            let value = pop_int(state)?;
            push_int(state, value ^ 1_i32.wrapping_shl(bit as u32))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "setbit_range" => {
            expect_plain(cmd, operand)?;
            let hi = pop_int(state)?;
            let lo = pop_int(state)?;
            let base = pop_int(state)?;
            let mask = bitmask(cmd, hi.wrapping_sub(lo).wrapping_add(1))?;
            push_int(state, base | mask.wrapping_shl(lo as u32))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "clearbit_range" => {
            expect_plain(cmd, operand)?;
            let hi = pop_int(state)?;
            let lo = pop_int(state)?;
            let base = pop_int(state)?;
            let mask = bitmask(cmd, hi.wrapping_sub(lo).wrapping_add(1))?;
            push_int(state, base & !(mask.wrapping_shl(lo as u32)))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "getbit_range" => {
            expect_plain(cmd, operand)?;
            let hi = pop_int(state)?;
            let lo = pop_int(state)?;
            let base = pop_int(state)?;
            let shl = 31_i32.wrapping_sub(hi) as u32;
            let shr = lo.wrapping_add(31).wrapping_sub(hi) as u32;
            let out = (base as u32).wrapping_shl(shl).wrapping_shr(shr) as i32;
            push_int(state, out)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Bit mask: `mask(n) = 2^n - 1` for `0..=32` (`-1` at 32).
fn bitmask(cmd: &str, count: i32) -> VmResult<i32> {
    if count == 32 {
        return Ok(-1);
    }
    if (0..32).contains(&count) {
        Ok(1_i32.wrapping_shl(count as u32).wrapping_sub(1))
    } else {
        Err(VmError::BadOperand {
            command: cmd.to_string(),
            expected: "bit count 0..=32",
        })
    }
}

/// Pure string ops for real: `append`, `append_num`, `append_signnum` (`+n`
/// for non-negative), `append_char` (`-1` errors), `tostring`,
/// `string_length` (char count), `substring` (char indices),
/// `string_indexof_char` and `string_indexof_string` (char indices, `-1` when
/// absent).
/// Locale/font/engine string ops (`compare`, `tostring_localised`,
/// `string_distance`, `text_switch`, …) trap.
fn handle_string(state: &mut RunState, cmd: &str, operand: &Operand) -> VmResult<bool> {
    match cmd {
        "append" => {
            expect_plain(cmd, operand)?;
            let b = pop_text(state)?;
            let a = pop_text(state)?;
            push_obj(state, crate::jstr::normalize(format!("{a}{b}")))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "append_num" => {
            expect_plain(cmd, operand)?;
            let num = pop_int(state)?;
            let base = pop_text(state)?;
            push_obj(state, format!("{base}{num}"))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "append_signnum" => {
            expect_plain(cmd, operand)?;
            let num = pop_int(state)?;
            let base = pop_text(state)?;
            if num >= 0 {
                push_obj(state, format!("{base}+{num}"))?;
            } else {
                push_obj(state, format!("{base}{num}"))?;
            }
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "append_char" => {
            expect_plain(cmd, operand)?;
            let code = pop_int(state)?;
            let base = pop_text(state)?;
            if code == -1 {
                return Err(VmError::InvalidChar { value: code });
            }
            // Appends `(char) code`: any code unit, surrogates included.
            let ch = crate::jstr::from_unit(code as u16);
            push_obj(state, crate::jstr::normalize(format!("{base}{ch}")))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "tostring" => {
            expect_plain(cmd, operand)?;
            let value = pop_int(state)?;
            push_obj(state, value.to_string())?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "string_length" => {
            expect_plain(cmd, operand)?;
            let text = pop_object(state)?;
            let len = text.as_deref().map_or(0, crate::jstr::len);
            let len = i32::try_from(len).unwrap_or(i32::MAX);
            push_int(state, len)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "substring" => {
            expect_plain(cmd, operand)?;
            let hi = pop_int(state)?;
            let lo = pop_int(state)?;
            let text = pop_string(state, cmd)?;
            push_obj(state, substring(cmd, &text, lo, hi)?)?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "string_indexof_char" => {
            expect_plain(cmd, operand)?;
            let from = pop_int(state)?;
            let code = pop_int(state)?;
            let text = pop_string(state, cmd)?;
            push_int(state, indexof_char(&text, code, from))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        "string_indexof_string" => {
            expect_plain(cmd, operand)?;
            let from = pop_int(state)?;
            let needle = pop_string(state, cmd)?;
            let haystack = pop_string(state, cmd)?;
            push_int(state, indexof_string(&haystack, &needle, from))?;
            state.pc = state.pc.saturating_add(1);
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn substring(cmd: &str, text: &str, lo: i32, hi: i32) -> VmResult<String> {
    if lo < 0 || hi < 0 {
        return Err(VmError::StringBounds {
            reason: format!("{cmd}: negative range {lo}..{hi}"),
        });
    }
    let lo = usize::try_from(lo).map_err(|_| VmError::StringBounds {
        reason: format!("{cmd}: bad start {lo}"),
    })?;
    let hi = usize::try_from(hi).map_err(|_| VmError::StringBounds {
        reason: format!("{cmd}: bad end {hi}"),
    })?;
    let chars = crate::jstr::units(text);
    if lo > hi || hi > chars.len() {
        return Err(VmError::StringBounds {
            reason: format!("{cmd}: range {lo}..{hi} over len {}", chars.len()),
        });
    }
    // `String.substring` may split a surrogate pair; the halves stay
    // single code units (crate::jstr).
    Ok(crate::jstr::from_units(&chars[lo..hi]))
}

fn indexof_char(text: &str, code: i32, from: i32) -> i32 {
    let Ok(code) = u32::try_from(code) else {
        return -1;
    };
    let needle = if code <= 0xffff {
        vec![code as u16]
    } else {
        let Some(ch) = char::from_u32(code) else {
            return -1;
        };
        ch.encode_utf16(&mut [0; 2]).to_vec()
    };
    indexof_units(&crate::jstr::units(text), &needle, from)
}

fn indexof_string(haystack: &str, needle: &str, from: i32) -> i32 {
    indexof_units(
        &crate::jstr::units(haystack),
        &crate::jstr::units(needle),
        from,
    )
}

fn indexof_units(haystack: &[u16], needle: &[u16], from: i32) -> i32 {
    let start = usize::try_from(from.max(0)).unwrap_or(usize::MAX);
    if needle.is_empty() {
        return i32::try_from(start.min(haystack.len())).unwrap_or(-1);
    }
    haystack
        .windows(needle.len())
        .enumerate()
        .skip(start)
        .find(|(_, part)| *part == needle)
        .and_then(|(index, _)| i32::try_from(index).ok())
        .unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::super::script::{Counts, Instruction};
    use super::super::vars::VarScope;
    use super::{Host, Value, Vm, VmError, VmResult};
    use super::{InstructionContext, VarLane, with_string_stack};
    use std::collections::HashMap;

    fn instr(command: &str, operand: super::super::script::Operand) -> Instruction {
        Instruction {
            opcode: 0,
            command: command.to_string(),
            operand,
        }
    }

    fn script(
        code: Vec<Instruction>,
        locals_int: u16,
        locals_obj: u16,
        locals_long: u16,
        args_int: u16,
        args_obj: u16,
        args_long: u16,
    ) -> super::super::script::CompiledScript {
        super::super::script::CompiledScript {
            name: None,
            locals: Counts {
                int: locals_int + args_int,
                obj: locals_obj + args_obj,
                long: locals_long + args_long,
            },
            args: Counts {
                int: args_int,
                obj: args_obj,
                long: args_long,
            },
            code,
        }
    }

    #[derive(Default)]
    struct MemHost {
        vars: HashMap<(VarScope, u16, bool), Value>,
        varbits: HashMap<(u16, bool), i32>,
        arrays: HashMap<i32, Vec<i32>>,
        operation_result: Option<i32>,
        operation_calls: Vec<(crate::execution::HostOperation, i32, i32)>,
    }

    impl Host for MemHost {
        fn trap_operation_context(
            &mut self,
            operation: crate::execution::HostOperation,
            context: &InstructionContext<'_>,
            ints: &mut Vec<i32>,
            _objects: &mut Vec<Option<String>>,
            _longs: &mut Vec<i64>,
        ) -> VmResult<Option<Value>> {
            if let (
                crate::execution::HostOperation::NextRuntimeChildSlot { bank_argument, .. },
                Some(result),
            ) = (operation, self.operation_result)
            {
                self.operation_calls.push((
                    operation,
                    context.int_locals[usize::from(bank_argument)],
                    ints.pop().unwrap(),
                ));
                Ok(Some(Value::Int(result)))
            } else {
                Err(VmError::UnknownCommand {
                    command: operation.spelling(),
                })
            }
        }
        fn trap(
            &mut self,
            command: &str,
            _ints: &mut Vec<i32>,
            _strings: &mut Vec<String>,
            _longs: &mut Vec<i64>,
        ) -> VmResult<Option<Value>> {
            if command == "opcount" {
                Ok(Some(Value::Int(super::opcount())))
            } else {
                Err(VmError::UnknownCommand {
                    command: command.into(),
                })
            }
        }
        fn var_type(&mut self, domain: VarScope, id: u16) -> VmResult<VarLane> {
            Ok(if domain == VarScope::Client && matches!(id, 2479 | 2480) {
                VarLane::String
            } else {
                VarLane::Int
            })
        }

        fn var_get(&mut self, domain: VarScope, id: u16, transmog: bool) -> VmResult<Value> {
            Ok(self
                .vars
                .get(&(domain, id, transmog))
                .cloned()
                .unwrap_or(Value::Int(0)))
        }

        fn var_set(
            &mut self,
            domain: VarScope,
            id: u16,
            transmog: bool,
            value: Value,
        ) -> VmResult<()> {
            self.vars.insert((domain, id, transmog), value);
            Ok(())
        }

        fn varbit_get(&mut self, id: u16, transmog: bool) -> VmResult<i32> {
            Ok(self.varbits.get(&(id, transmog)).copied().unwrap_or(0))
        }

        fn varbit_set(&mut self, id: u16, transmog: bool, value: i32) -> VmResult<()> {
            self.varbits.insert((id, transmog), value);
            Ok(())
        }

        fn array_define(&mut self, array_id: i32, len: usize) -> VmResult<()> {
            if len > super::MAX_ARRAY_LEN {
                return Err(VmError::BadArray {
                    id: array_id,
                    reason: format!("length {len} too large"),
                });
            }
            self.arrays.insert(array_id, vec![0; len]);
            Ok(())
        }

        fn array_len(&mut self, array_id: i32) -> VmResult<usize> {
            self.arrays
                .get(&array_id)
                .map(Vec::len)
                .ok_or_else(|| VmError::BadArray {
                    id: array_id,
                    reason: "undefined array".to_string(),
                })
        }

        fn array_get(&mut self, array_id: i32, index: i32) -> VmResult<i32> {
            let items = self
                .arrays
                .get(&array_id)
                .ok_or_else(|| VmError::BadArray {
                    id: array_id,
                    reason: "undefined array".to_string(),
                })?;
            let slot = usize::try_from(index).map_err(|_| VmError::BadArray {
                id: array_id,
                reason: format!("negative index {index}"),
            })?;
            items.get(slot).copied().ok_or_else(|| VmError::BadArray {
                id: array_id,
                reason: format!("index {index} out of bounds"),
            })
        }

        fn array_set(&mut self, array_id: i32, index: i32, value: i32) -> VmResult<()> {
            let items = self
                .arrays
                .get_mut(&array_id)
                .ok_or_else(|| VmError::BadArray {
                    id: array_id,
                    reason: "undefined array".to_string(),
                })?;
            let slot = usize::try_from(index).map_err(|_| VmError::BadArray {
                id: array_id,
                reason: format!("negative index {index}"),
            })?;
            let cell = items.get_mut(slot).ok_or_else(|| VmError::BadArray {
                id: array_id,
                reason: format!("index {index} out of bounds"),
            })?;
            *cell = value;
            Ok(())
        }
    }

    fn verify_generated_accounting() {
        use super::{HookLocals, MAX_FRAMES, Programs, Session};
        use crate::{
            execution::{Role, Table, decode_accounting},
            opcode::OpcodeBook,
            script::encode_script,
        };
        use std::collections::BTreeMap;
        const SOURCE: i32 = 1;
        const ADAPTER: i32 = 2;
        const ENTRY: i32 = 3;
        const ARGUMENT_SLOT: i32 = 0;
        const ARGUMENT_COUNT: u16 = 1;
        const ADAPTER_CALL_PC: usize = 8;
        const ADAPTER_CALL_TARGET: i32 = 8;
        const LEAF_STEPS: usize = 5;
        const SOURCE_STEPS_PER_FRAME: usize = 8;
        const SOURCE_STEPS_BEFORE_RECURSION: i32 = 7;
        const LEAF_STEPS_AT_HOST: i32 = 4;
        const EXTRA_PHYSICAL_FRAMES: usize = 2;
        let book = OpcodeBook::embedded().unwrap();
        let operation = |command, operand| {
            let mut instruction = instr(command, operand);
            instruction.opcode = book.opcode_for(command).unwrap();
            instruction
        };
        let source = script(
            vec![
                operation("push_int_local", Operand::Local(ARGUMENT_SLOT)),
                operation("push_constant_string", Operand::Int(i32::default())),
                operation("branch_equals", Operand::Branch(ADAPTER_CALL_TARGET)),
                operation("push_int_local", Operand::Local(ARGUMENT_SLOT)),
                operation("push_constant_string", Operand::Int(i32::from(true))),
                operation(
                    "quickchat_dynamic_command_add",
                    Operand::Byte(u8::default()),
                ),
                operation("gosub_with_params", Operand::Script(SOURCE)),
                operation("return", Operand::Byte(u8::default())),
                operation("gosub_with_params", Operand::Script(ADAPTER)),
                operation("return", Operand::Byte(u8::default())),
            ],
            u16::default(),
            u16::default(),
            u16::default(),
            ARGUMENT_COUNT,
            u16::default(),
            u16::default(),
        );
        let adapter = script(
            vec![
                operation("opcount", Operand::Byte(u8::default())),
                operation("return", Operand::Byte(u8::default())),
            ],
            u16::default(),
            u16::default(),
            u16::default(),
            u16::default(),
            u16::default(),
            u16::default(),
        );
        let entry = script(
            vec![
                operation("push_int_local", Operand::Local(ARGUMENT_SLOT)),
                operation("gosub_with_params", Operand::Script(SOURCE)),
                operation("return", Operand::Byte(u8::default())),
            ],
            u16::default(),
            u16::default(),
            u16::default(),
            ARGUMENT_COUNT,
            u16::default(),
            u16::default(),
        );
        let scripts = BTreeMap::from([(SOURCE, source), (ADAPTER, adapter), (ENTRY, entry)]);
        let mut table = Table::default();
        for (id, script) in &scripts {
            let role = match *id {
                SOURCE => Role::Source,
                ADAPTER => Role::Adapter,
                _ => Role::Entry,
            };
            let calls = if *id == SOURCE {
                BTreeMap::from([(ADAPTER_CALL_PC, ADAPTER)])
            } else {
                BTreeMap::new()
            };
            table
                .insert(*id, &encode_script(script, &book).unwrap(), role, calls)
                .unwrap();
        }
        let table = Table::parse(&table.emit()).unwrap();
        table.validate_links(&scripts).unwrap();
        let mut provider = Programs::default();
        for (id, script) in &scripts {
            let bytes = encode_script(script, &book).unwrap();
            let metadata = table.group_bytes(*id).unwrap();
            let accounting = decode_accounting(*id, &bytes, Some(&metadata), script).unwrap();
            provider.scripts.insert(*id, script.clone());
            provider.accounting.insert(*id, accounting);
            let mut changed = bytes.clone();
            changed.push(u8::default());
            assert!(decode_accounting(*id, &changed, Some(&metadata), script).is_err());
            assert!(Table::from_group(*id + i32::from(true), &metadata).is_err());
        }
        let mut host = MemHost::default();
        let root = &scripts[&ENTRY];
        let mut mismatched = Session::for_script(
            ENTRY,
            &scripts[&SOURCE],
            &[Value::Int(i32::default())],
            None,
        )
        .unwrap();
        assert!(matches!(
            Vm::new(&mut host, &provider).step(&mut mismatched),
            Err(VmError::BadOperand { .. })
        ));
        for depth in [usize::default(), MAX_FRAMES] {
            let mut session = Session::for_script(
                ENTRY,
                root,
                &[Value::Int(i32::try_from(depth).unwrap())],
                None,
            )
            .unwrap();
            let mut vm = Vm::new(&mut host, &provider);
            let mut physical_frames = usize::default();
            while !vm.step(&mut session).unwrap() {
                physical_frames = physical_frames.max(session.state.frames.len());
            }
            assert_eq!(
                session.snapshot().steps,
                depth * SOURCE_STEPS_PER_FRAME + LEAF_STEPS
            );
            assert_eq!(
                session.snapshot().ints,
                [
                    i32::try_from(depth).unwrap() * SOURCE_STEPS_BEFORE_RECURSION
                        + LEAF_STEPS_AT_HOST
                ]
            );
            if depth == MAX_FRAMES {
                assert_eq!(physical_frames, MAX_FRAMES + EXTRA_PHYSICAL_FRAMES);
            }
        }
        let mut session = Session::for_script(
            ENTRY,
            root,
            &[Value::Int(
                i32::try_from(MAX_FRAMES + usize::from(true)).unwrap(),
            )],
            None,
        )
        .unwrap();
        let error = loop {
            match Vm::new(&mut host, &provider).step(&mut session) {
                Ok(false) => {}
                Ok(true) => panic!("source frame overflow accepted"),
                Err(error) => break error,
            }
        };
        assert!(matches!(error, VmError::FrameOverflow));
        for budget in [LEAF_STEPS, LEAF_STEPS - usize::from(true)] {
            let locals = HookLocals {
                ints: vec![i32::default()],
                strings: vec![],
                longs: vec![],
            };
            let mut session = Session::for_hook(ENTRY, root, locals, &[], budget, None).unwrap();
            let result = loop {
                match Vm::new(&mut host, &provider).step(&mut session) {
                    Ok(false) => {}
                    result => break result,
                }
            };
            assert_eq!(session.snapshot().steps, LEAF_STEPS);
            if budget == LEAF_STEPS {
                assert!(result.unwrap());
                assert_eq!(super::opcount(), LEAF_STEPS as i32);
            } else {
                assert!(matches!(result, Err(VmError::StepLimit)));
            }
        }
        provider.accounting.remove(&ADAPTER);
        let mut session =
            Session::for_script(ENTRY, root, &[Value::Int(i32::default())], None).unwrap();
        let result = loop {
            match Vm::new(&mut host, &provider).step(&mut session) {
                Ok(false) => {}
                result => break result,
            }
        };
        assert!(matches!(result, Err(VmError::BadOperand { .. })));
        let mut native =
            Session::for_script(ENTRY, root, &[Value::Int(MAX_FRAMES as i32)], None).unwrap();
        let result = loop {
            match Vm::new(&mut host, &provider.scripts).step(&mut native) {
                Ok(false) => {}
                result => break result,
            }
        };
        assert!(matches!(result, Err(VmError::FrameOverflow)));
        let bytes = encode_script(&scripts[&SOURCE], &book).unwrap();
        let mut cyclic = Table::default();
        cyclic
            .insert(SOURCE, &bytes, Role::Adapter, BTreeMap::new())
            .unwrap();
        assert!(cyclic.validate(SOURCE, &bytes, &scripts[&SOURCE]).is_err());
        verify_entry_argument_binding();
        verify_imported_host_operation();
        verify_imported_bank_argument();
    }

    fn verify_imported_host_operation() {
        use super::{Programs, Session};
        use crate::{
            execution::{HostOperation, Role, Specification, Table, decode_accounting},
            opcode::OpcodeBook,
        };
        use std::collections::BTreeMap;
        const ADAPTER: i32 = 1;
        const ARGUMENT_SLOT: i32 = 0;
        const ARGUMENT_COUNT: u16 = 1;
        const HOST_PC: usize = 1;
        const RETURN_PC: usize = 2;
        const ABSENT_COMPONENT: i32 = -1;
        let book = OpcodeBook::embedded().unwrap();
        let operation = |command, operand| {
            let mut instruction = instr(command, operand);
            instruction.opcode = book.opcode_for(command).unwrap();
            instruction
        };
        let mut adapter = script(
            vec![
                operation("push_int_local", Operand::Local(ARGUMENT_SLOT)),
                operation("cc_deleteall", Operand::Byte(u8::default())),
                operation("return", Operand::Byte(u8::default())),
            ],
            u16::default(),
            u16::default(),
            u16::default(),
            ARGUMENT_COUNT,
            u16::default(),
            u16::default(),
        );
        adapter.name = Some("proc,clear_children".into());
        let mut table = Table::default();
        let bytes = table
            .bind_import(
                ADAPTER,
                &mut adapter,
                &book,
                Specification {
                    resource: None,
                    role: Role::Adapter,
                    adapter_calls: BTreeMap::new(),
                    host_operations: BTreeMap::from([(
                        HOST_PC,
                        HostOperation::ClearRuntimeChildren,
                    )]),
                },
            )
            .unwrap();
        let table = Table::parse(&table.emit()).unwrap();
        table.validate(ADAPTER, &bytes, &adapter).unwrap();
        let metadata = table.group_bytes(ADAPTER).unwrap();
        let accounting = decode_accounting(ADAPTER, &bytes, Some(&metadata), &adapter).unwrap();
        assert!(decode_accounting(ADAPTER, &bytes, None, &adapter).is_err());
        let stripped_operation = table
            .emit()
            .lines()
            .filter(|line| !line.starts_with("host-operation "))
            .collect::<Vec<_>>()
            .join("\n");
        let changed = Table::parse(&stripped_operation).unwrap();
        assert!(changed.validate(ADAPTER, &bytes, &adapter).is_err());
        assert!(
            Table::default()
                .validate_links(&BTreeMap::from([(ADAPTER, adapter.clone())]))
                .is_err()
        );
        let mut unlinked =
            Session::for_script(ADAPTER, &adapter, &[Value::Int(ABSENT_COMPONENT)], None).unwrap();
        let mut unlinked_host = MemHost::default();
        assert!(matches!(
            Vm::new(&mut unlinked_host, &()).step(&mut unlinked),
            Err(VmError::BadOperand { .. })
        ));
        assert_eq!(unlinked.snapshot().steps, usize::default());
        assert!(matches!(
            Vm::new(&mut unlinked_host, &()).execute(&adapter, &[Value::Int(ABSENT_COMPONENT)]),
            Err(VmError::BadOperand { .. })
        ));
        let native_caller = script(
            vec![
                operation("push_constant_string", Operand::Int(ABSENT_COMPONENT)),
                operation("gosub_with_params", Operand::Script(ADAPTER)),
                operation("return", Operand::Byte(u8::default())),
            ],
            u16::default(),
            u16::default(),
            u16::default(),
            u16::default(),
            u16::default(),
            u16::default(),
        );
        let stripped_provider = std::collections::HashMap::from([(ADAPTER, adapter.clone())]);
        assert!(matches!(
            Vm::new(&mut unlinked_host, &stripped_provider).execute(&native_caller, &[]),
            Err(VmError::BadOperand { .. })
        ));
        assert_eq!(
            accounting.host_operations,
            BTreeMap::from([(HOST_PC, HostOperation::ClearRuntimeChildren)])
        );
        let mut provider = Programs::default();
        provider.scripts.insert(ADAPTER, adapter.clone());
        provider.accounting.insert(ADAPTER, accounting);
        let mut session =
            Session::for_script(ADAPTER, &adapter, &[Value::Int(ABSENT_COMPONENT)], None).unwrap();
        let mut host = MemHost::default();
        assert!(!Vm::new(&mut host, &provider).step(&mut session).unwrap());
        assert!(matches!(Vm::new(&mut host, &provider).step(&mut session),
            Err(VmError::UnknownCommand { command }) if command == "clear-runtime-children"));
        for role in [Role::Source, Role::Entry] {
            let mut invalid = Table::default();
            invalid
                .insert(ADAPTER, &bytes, role, BTreeMap::new())
                .unwrap();
            assert!(
                invalid
                    .insert_host_operation(ADAPTER, HOST_PC, HostOperation::ClearRuntimeChildren)
                    .is_err()
            );
        }
        let mut mismatch = Table::default();
        mismatch
            .insert(ADAPTER, &bytes, Role::Adapter, BTreeMap::new())
            .unwrap();
        mismatch
            .insert_host_operation(ADAPTER, RETURN_PC, HostOperation::ClearRuntimeChildren)
            .unwrap();
        assert!(mismatch.validate(ADAPTER, &bytes, &adapter).is_err());
        assert!(
            Table::parse(
                &table
                    .emit()
                    .replace("clear-runtime-children", "unverified-operation")
            )
            .is_err()
        );
    }

    fn verify_imported_bank_argument() {
        use super::{CompiledScript, Programs};
        use crate::{
            execution::{
                HostOperation, Role, RuntimeChildBanks, Specification, Table, decode_accounting,
            },
            opcode::OpcodeBook,
        };
        use std::collections::BTreeMap;
        const ADAPTER: i32 = 1;
        const BANK_SLOT: u16 = 0;
        const COMPONENT_SLOT: i32 = 1;
        const ARGUMENT_COUNT: u16 = 2;
        const HOST_PC: usize = 1;
        const SYNTHETIC_BANK_COUNT: u16 = 3;
        const SYNTHETIC_ROOT_WIDTH: u16 = 8;
        const SYNTHETIC_OTHER_WIDTH: u16 = 4;
        const BANK: i32 = 2;
        const PACKED: i32 = i32::MAX;
        const SLOT: i32 = 3;
        let book = OpcodeBook::embedded().unwrap();
        let instruction = |command, operand| {
            let mut instruction = instr(command, operand);
            instruction.opcode = book.opcode_for(command).unwrap();
            instruction
        };
        let operation = HostOperation::NextRuntimeChildSlot {
            banks: RuntimeChildBanks::new(
                SYNTHETIC_BANK_COUNT,
                SYNTHETIC_ROOT_WIDTH,
                SYNTHETIC_OTHER_WIDTH,
            )
            .unwrap(),
            bank_argument: BANK_SLOT,
        };
        assert_eq!(
            HostOperation::parse(&operation.spelling()).unwrap(),
            operation
        );
        let mut adapter = CompiledScript {
            name: Some("proc,next_child_slot".into()),
            args: Counts {
                int: ARGUMENT_COUNT,
                ..Counts::default()
            },
            locals: Counts {
                int: ARGUMENT_COUNT,
                ..Counts::default()
            },
            code: vec![
                instruction("push_int_local", Operand::Local(COMPONENT_SLOT)),
                instruction(operation.command(), Operand::Byte(u8::default())),
                instruction("return", Operand::Byte(u8::default())),
            ],
        };
        let mut table = Table::default();
        let specification = |operation| Specification {
            resource: None,
            role: Role::Adapter,
            adapter_calls: BTreeMap::new(),
            host_operations: BTreeMap::from([(HOST_PC, operation)]),
        };
        let bytes = table
            .bind_import(ADAPTER, &mut adapter, &book, specification(operation))
            .unwrap();
        let metadata = table.group_bytes(ADAPTER).unwrap();
        let accounting = decode_accounting(ADAPTER, &bytes, Some(&metadata), &adapter).unwrap();
        let mut provider = Programs::default();
        provider.scripts.insert(ADAPTER, adapter.clone());
        provider.accounting.insert(ADAPTER, accounting);
        let mut host = MemHost {
            operation_result: Some(SLOT),
            ..MemHost::default()
        };
        let mut session = super::Session::for_script(
            ADAPTER,
            &adapter,
            &[Value::Int(BANK), Value::Int(PACKED)],
            None,
        )
        .unwrap();
        while !Vm::new(&mut host, &provider).step(&mut session).unwrap() {}
        assert_eq!(super::return_value(&session.state), Some(Value::Int(SLOT)));
        assert_eq!(host.operation_calls, [(operation, BANK, PACKED)]);
        let invalid = HostOperation::NextRuntimeChildSlot {
            banks: match operation {
                HostOperation::NextRuntimeChildSlot { banks, .. } => banks,
                HostOperation::ComponentText { .. }
                | HostOperation::ComponentPaint { .. }
                | HostOperation::CreateFlatTextChild { .. }
                | HostOperation::FindComponent
                | HostOperation::ClearRuntimeChildren
                | HostOperation::FindRuntimeChild { .. }
                | HostOperation::FindFlatChild { .. }
                | HostOperation::RetainedPlayerTransmit { .. }
                | HostOperation::Enum { .. }
                | HostOperation::VariableBit { .. }
                | HostOperation::DatabaseField { .. }
                | HostOperation::DatabaseFieldCount { .. } => {
                    unreachable!()
                }
            },
            bank_argument: ARGUMENT_COUNT,
        };
        let mut changed = adapter.clone();
        assert!(
            Table::default()
                .bind_import(ADAPTER, &mut changed, &book, specification(invalid))
                .is_err()
        );
        let changed = Table::parse(
            &table
                .emit()
                .replace(&operation.spelling(), &invalid.spelling()),
        )
        .unwrap();
        assert!(changed.validate(ADAPTER, &bytes, &adapter).is_err());
        adapter.args.int = BANK_SLOT;
        assert!(
            Table::default()
                .bind_import(ADAPTER, &mut adapter, &book, specification(operation))
                .is_err()
        );
    }

    fn verify_entry_argument_binding() {
        use super::{HookLocals, MAX_STACK, Programs, Session};
        use crate::{
            execution::{Role, Table, decode_accounting},
            opcode::OpcodeBook,
            script::{CompiledScript, encode_script},
        };
        use std::collections::BTreeMap;
        const SOURCE: i32 = 1;
        const ENTRY: i32 = 2;
        const INPUT_SLOT: i32 = 0;
        const FIXED_SLOT: i32 = 0;
        const BOUND_INPUT_SLOT: i32 = 1;
        const SCRATCH_SLOT: i32 = 2;
        const BOUND_ARGUMENTS: u16 = 2;
        const LONG_LOCALS: u16 = 3;
        const INPUT_INT: i32 = 17;
        const FIXED_INT: i32 = 23;
        const INPUT_LONG: i64 = 101;
        const FIXED_LONG: i64 = 103;
        const RETAINED_LONG: i64 = -7;
        const FIXED_STRING: &str = "bound object";
        const SOURCE_STEPS: usize = 9;
        let book = OpcodeBook::embedded().unwrap();
        let operation = |command, operand| {
            let mut instruction = instr(command, operand);
            instruction.opcode = book.opcode_for(command).unwrap();
            instruction
        };
        let source = CompiledScript {
            name: None,
            args: Counts {
                int: BOUND_ARGUMENTS,
                obj: BOUND_ARGUMENTS,
                long: BOUND_ARGUMENTS,
            },
            locals: Counts {
                int: BOUND_ARGUMENTS,
                obj: BOUND_ARGUMENTS,
                long: LONG_LOCALS,
            },
            code: vec![
                operation("pop_long_discard", Operand::Byte(u8::default())),
                operation("push_long_local", Operand::Local(BOUND_INPUT_SLOT)),
                operation("pop_long_local", Operand::Local(SCRATCH_SLOT)),
                operation("push_long_local", Operand::Local(FIXED_SLOT)),
                operation("push_int_local", Operand::Local(FIXED_SLOT)),
                operation("push_int_local", Operand::Local(BOUND_INPUT_SLOT)),
                operation("push_string_local", Operand::Local(FIXED_SLOT)),
                operation("push_string_local", Operand::Local(BOUND_INPUT_SLOT)),
                operation("return", Operand::Byte(u8::default())),
            ],
        };
        let entry = script(
            vec![
                operation("push_constant_string", Operand::Int(FIXED_INT)),
                operation("push_int_local", Operand::Local(INPUT_SLOT)),
                operation("push_constant_string", Operand::Str(FIXED_STRING.into())),
                operation("push_string_local", Operand::Local(INPUT_SLOT)),
                operation("push_constant_string", Operand::Long(FIXED_LONG)),
                operation("push_long_local", Operand::Local(INPUT_SLOT)),
                operation("gosub_with_params", Operand::Script(SOURCE)),
                operation("return", Operand::Byte(u8::default())),
            ],
            u16::default(),
            u16::default(),
            u16::default(),
            u16::from(true),
            u16::from(true),
            u16::from(true),
        );
        let mut table = Table::default();
        let mut provider = Programs::default();
        for (id, script, role) in [
            (SOURCE, &source, Role::Source),
            (ENTRY, &entry, Role::Entry),
        ] {
            let bytes = encode_script(script, &book).unwrap();
            table.insert(id, &bytes, role, BTreeMap::new()).unwrap();
            let metadata = table.group_bytes(id).unwrap();
            provider.scripts.insert(id, script.clone());
            provider.accounting.insert(
                id,
                decode_accounting(id, &bytes, Some(&metadata), script).unwrap(),
            );
        }
        let retained = vec![RETAINED_LONG; MAX_STACK];
        let entry_locals = || HookLocals {
            ints: vec![INPUT_INT],
            strings: vec![None],
            longs: vec![INPUT_LONG],
        };
        let source_locals = || HookLocals {
            ints: vec![FIXED_INT, INPUT_INT],
            strings: vec![Some(FIXED_STRING.into()), None],
            longs: vec![FIXED_LONG, INPUT_LONG, i64::default()],
        };
        let mut session =
            Session::for_hook(ENTRY, &entry, entry_locals(), &retained, SOURCE_STEPS, None)
                .unwrap();
        let mut host = MemHost::default();
        let mut vm = Vm::new(&mut host, &provider);
        while session.snapshot().script_id != Some(SOURCE) {
            assert!(!vm.step(&mut session).unwrap());
            assert_eq!(session.retained_longs(), retained);
            assert!(session.snapshot().ints.is_empty());
            assert!(session.snapshot().strings.is_empty());
            assert_eq!(session.snapshot().steps, usize::default());
        }
        let bound = session.snapshot();
        assert_eq!(bound.int_locals, source_locals().ints);
        assert_eq!(bound.string_locals, source_locals().strings);
        assert_eq!(bound.long_locals, source_locals().longs);
        while !vm.step(&mut session).unwrap() {}
        let imported = session.snapshot();
        assert_eq!(imported.steps, SOURCE_STEPS);
        assert_eq!(imported.ints, [FIXED_INT, INPUT_INT]);
        assert_eq!(imported.strings, [Some(FIXED_STRING.into()), None]);
        let mut expected_longs = retained.clone();
        *expected_longs.last_mut().unwrap() = FIXED_LONG;
        assert_eq!(imported.longs, expected_longs);
        let mut direct = Session::for_hook(
            SOURCE,
            &source,
            source_locals(),
            &retained,
            SOURCE_STEPS,
            None,
        )
        .unwrap();
        while !vm.step(&mut direct).unwrap() {}
        assert_eq!(direct.snapshot().ints, imported.ints);
        assert_eq!(direct.snapshot().strings, imported.strings);
        assert_eq!(direct.snapshot().longs, imported.longs);
        assert_eq!(direct.snapshot().steps, imported.steps);
        // Ordinary 910 setup still uses its operand stacks and overflows here.
        let mut ordinary =
            Session::for_hook(ENTRY, &entry, entry_locals(), &retained, SOURCE_STEPS, None)
                .unwrap();
        let error = loop {
            match Vm::new(&mut host, &provider.scripts).step(&mut ordinary) {
                Ok(false) => {}
                Ok(true) => panic!("ordinary stack overflow accepted"),
                Err(error) => break error,
            }
        };
        assert!(matches!(error, VmError::StackFull { stack: "long" }));
    }

    use super::super::script::Operand;

    #[test]
    fn straight_line_int_math_returns_sum() {
        let prog = script(
            vec![
                instr("push_constant_int", Operand::Int(2)),
                instr("push_constant_int", Operand::Int(3)),
                instr("add", Operand::Byte(0)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = MemHost::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]).unwrap();
        assert_eq!(out, Some(Value::Int(5)));
    }

    #[test]
    fn branch_if_false_skips_dead_push() {
        let prog = script(
            vec![
                instr("push_constant_int", Operand::Int(0)),
                instr("branch_if_false", Operand::Branch(3)),
                instr("push_constant_int", Operand::Int(99)),
                instr("push_constant_int", Operand::Int(7)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = MemHost::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]).unwrap();
        assert_eq!(out, Some(Value::Int(7)));
    }

    #[test]
    fn switch_dispatches_on_match() {
        use super::super::script::SwitchCase;
        let prog = script(
            vec![
                instr("push_constant_int", Operand::Int(2)),
                instr(
                    "switch",
                    Operand::Switch(vec![
                        SwitchCase {
                            value: 1,
                            target: 4,
                        },
                        SwitchCase {
                            value: 2,
                            target: 6,
                        },
                        SwitchCase {
                            value: 3,
                            target: 8,
                        },
                    ]),
                ),
                instr("push_constant_int", Operand::Int(0)),
                instr("branch", Operand::Branch(9)),
                instr("push_constant_int", Operand::Int(10)),
                instr("branch", Operand::Branch(9)),
                instr("push_constant_int", Operand::Int(20)),
                instr("branch", Operand::Branch(9)),
                instr("push_constant_int", Operand::Int(30)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = MemHost::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]).unwrap();
        assert_eq!(out, Some(Value::Int(20)));
    }

    #[test]
    fn gosub_return_passes_typed_args() {
        verify_generated_accounting();
        let sub = script(
            vec![
                instr("push_int_local", Operand::Local(0)),
                instr("push_int_local", Operand::Local(1)),
                instr("add", Operand::Byte(0)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            2,
            0,
            0,
        );
        let top = script(
            vec![
                instr("push_constant_int", Operand::Int(10)),
                instr("push_constant_int", Operand::Int(32)),
                instr("gosub_with_params", Operand::Script(1)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = MemHost::default();
        let provider = HashMap::from([(1, sub)]);
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&top, &[]).unwrap();
        assert_eq!(out, Some(Value::Int(42)));
    }

    #[test]
    fn join_string_concatenates_in_order() {
        let prog = script(
            vec![
                instr("push_constant_string", Operand::Str("hello".to_string())),
                instr("push_constant_string", Operand::Str(" ".to_string())),
                instr("push_constant_string", Operand::Str("world".to_string())),
                instr("join_string", Operand::Count(3)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = MemHost::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]).unwrap();
        assert_eq!(out, Some(Value::Str("hello world".to_string())));
    }

    #[test]
    fn null_objects_survive_locals_calls_and_follow_string_rules() {
        const CALLEE_ID: i32 = 900_010;
        const LOCAL_SLOT: i32 = 0;
        const SYNTHETIC_VAR: u16 = 0;
        let recorded = include_str!("../tests/fixtures/cs2-traces/null-objects.tsv");
        let outcome = |command: &str| {
            let (_, value) = recorded
                .lines()
                .filter_map(|line| line.split_once('\t'))
                .find(|(name, _)| *name == command)
                .unwrap();
            match value {
                "n" => Ok(Some(Value::Null)),
                "none" => Ok(None),
                "e:null_object" => Err(()),
                value if value.starts_with("i:") => {
                    Ok(Some(Value::Int(value[2..].parse().unwrap())))
                }
                value if value.starts_with("s:") => {
                    let hex = &value[2..];
                    let units = (0..hex.len())
                        .step_by(4)
                        .map(|n| u16::from_str_radix(&hex[n..n + 4], 16).unwrap())
                        .collect::<Vec<_>>();
                    Ok(Some(Value::Str(crate::jstr::from_units(&units))))
                }
                _ => panic!("unexpected recording {value}"),
            }
        };
        let sub = script(
            vec![
                instr("push_string_local", Operand::Local(LOCAL_SLOT)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            1,
            0,
        );
        let provider = HashMap::from([(CALLEE_ID, sub.clone())]);
        let mut host = MemHost::default();
        let mut vm = Vm::new(&mut host, &provider);
        assert_eq!(vm.execute(&sub, &[Value::Null]).unwrap(), Some(Value::Null));
        let round_trip = script(
            vec![
                instr("push_string_local", Operand::Local(LOCAL_SLOT)),
                instr("gosub_with_params", Operand::Script(CALLEE_ID)),
                instr("pop_string_local", Operand::Local(LOCAL_SLOT)),
                instr("push_string_local", Operand::Local(LOCAL_SLOT)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            1,
            0,
            0,
            0,
            0,
        );
        assert_eq!(
            vm.execute(&round_trip, &[]).map_err(|_| ()),
            outcome("push_string_local")
        );
        for command in [
            "pop_string_discard",
            "string_length",
            "join_string",
            "append",
            "append_num",
            "append_signnum",
            "append_char",
            "substring",
            "string_indexof_char",
            "string_indexof_string",
        ] {
            let mut code = vec![instr("push_string_local", Operand::Local(LOCAL_SLOT))];
            match command {
                "append" => code.push(instr("push_constant_string", Operand::Str("!".into()))),
                "append_num" | "append_signnum" => {
                    code.push(instr("push_constant_int", Operand::Int(7)));
                }
                "append_char" => {
                    code.push(instr("push_constant_int", Operand::Int(i32::from(b'x'))));
                }
                "substring" | "string_indexof_char" => code.extend([
                    instr("push_constant_int", Operand::Int(0)),
                    instr("push_constant_int", Operand::Int(0)),
                ]),
                "string_indexof_string" => code.extend([
                    instr("push_constant_string", Operand::Str("x".into())),
                    instr("push_constant_int", Operand::Int(0)),
                ]),
                _ => {}
            }
            code.push(instr(
                command,
                if command == "join_string" {
                    Operand::Count(1)
                } else {
                    Operand::Byte(0)
                },
            ));
            code.push(instr("return", Operand::Byte(0)));
            let root = script(code, 0, 1, 0, 0, 0, 0);
            let result = vm.execute(&root, &[]);
            if outcome(command).is_err() {
                assert!(
                    matches!(&result, Err(VmError::NullObject { command: failed }) if failed == command)
                );
            }
            assert_eq!(result.map_err(|_| ()), outcome(command), "{command}");
        }
        let var = crate::script::VarRef {
            domain: VarScope::Client,
            id: SYNTHETIC_VAR,
            transmog: false,
        };
        let root = script(
            vec![
                instr("push_string_local", Operand::Local(LOCAL_SLOT)),
                instr("pop_var", Operand::VarRef(var.clone())),
                instr("push_var", Operand::VarRef(var)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            1,
            0,
            0,
            0,
            0,
        );
        let mut host = crate::runtime::RuntimeHost::default();
        host.definitions
            .insert((VarScope::Client, SYNTHETIC_VAR), VarLane::String);
        assert_eq!(
            Vm::new(&mut host, &()).execute(&root, &[]).unwrap(),
            Some(Value::Str("null".into()))
        );
        assert_eq!(
            host.variables[&(VarScope::Client, SYNTHETIC_VAR, false)],
            Value::Null
        );
        assert!(
            host.var_set(VarScope::Client, SYNTHETIC_VAR, false, Value::Int(0))
                .is_err()
        );
        let mut objects = vec![None, Some("top".into())];
        with_string_stack("string-only host", &mut objects, |strings| {
            assert_eq!(strings.pop().as_deref(), Some("top"));
            strings.push("result".into());
            Ok(())
        })
        .unwrap();
        assert_eq!(objects, [None, Some("result".into())]);
        assert!(matches!(
            with_string_stack("string-only host", &mut objects, |strings| {
                strings.pop();
                strings
                    .pop()
                    .ok_or(VmError::StackUnderflow { stack: "object" })
            }),
            Err(VmError::NullObject { .. })
        ));
        assert_eq!(objects, [None]);
    }

    #[test]
    fn var_varbit_and_array_roundtrip() {
        use super::super::script::{VarBitRef, VarRef};
        let prog = script(
            vec![
                instr("push_constant_int", Operand::Int(100)),
                instr(
                    "pop_var",
                    Operand::VarRef(VarRef {
                        domain: VarScope::Player,
                        id: 7,
                        transmog: false,
                    }),
                ),
                instr(
                    "push_var",
                    Operand::VarRef(VarRef {
                        domain: VarScope::Player,
                        id: 7,
                        transmog: false,
                    }),
                ),
                instr("push_constant_int", Operand::Int(7)),
                instr(
                    "pop_varbit",
                    Operand::VarBitRef(VarBitRef {
                        id: 100,
                        transmog: false,
                    }),
                ),
                instr(
                    "push_varbit",
                    Operand::VarBitRef(VarBitRef {
                        id: 100,
                        transmog: false,
                    }),
                ),
                instr("add", Operand::Byte(0)),
                instr("push_constant_int", Operand::Int(2)),
                instr("define_array", Operand::Array(0)),
                instr("push_constant_int", Operand::Int(0)),
                instr("push_constant_int", Operand::Int(55)),
                instr("pop_array_int", Operand::Array(0)),
                instr("push_constant_int", Operand::Int(0)),
                instr("push_array_int", Operand::Array(0)),
                instr("add", Operand::Byte(0)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = MemHost::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]).unwrap();
        assert_eq!(out, Some(Value::Int(162)));
    }

    #[test]
    fn long_locals_roundtrip() {
        let prog = script(
            vec![
                instr("push_long_constant", Operand::Long(1_i64 << 40)),
                instr("pop_long_local", Operand::Local(0)),
                instr("push_long_local", Operand::Local(0)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            1,
            0,
            0,
            0,
        );
        let mut host = MemHost::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]).unwrap();
        assert_eq!(out, Some(Value::Long(1_i64 << 40)));
    }

    #[test]
    fn string_locals_and_typed_constant_tags() {
        let prog = script(
            vec![
                instr("push_constant_string", Operand::Int(41)),
                instr("pop_int_local", Operand::Local(0)),
                instr("push_int_local", Operand::Local(0)),
                instr("tostring", Operand::Byte(0)),
                instr("pop_string_local", Operand::Local(0)),
                instr("push_string_local", Operand::Local(0)),
                instr("return", Operand::Byte(0)),
            ],
            1,
            1,
            0,
            0,
            0,
            0,
        );
        let mut host = MemHost::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]).unwrap();
        assert_eq!(out, Some(Value::Str("41".to_string())));
    }

    #[test]
    fn unknown_engine_opcode_errors_without_panic() {
        let prog = script(
            vec![
                instr("if_opentop", Operand::Byte(0)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = MemHost::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let error = vm.execute(&prog, &[]).unwrap_err();
        assert!(matches!(error, VmError::UnknownCommand { .. }));
    }

    /// Spot-check the [`super::arity`] table against the retail command
    /// handlers (leading pops plus the shared `cc_if_*` helper body, trailing
    /// pushes).
    #[test]
    fn arity_spot_checks_match_script_runner() {
        use super::{Arity, arity};
        // `if_getlayer`: pops the packed component id, pushes its layer.
        assert_eq!(
            arity("if_getlayer"),
            Some(Arity {
                int_pop: 1,
                str_pop: 0,
                long_pop: 0,
                int_push: 1,
                str_push: 0,
                long_push: 0,
            }),
        );
        // `cc_gettext`: active component, no args, pushes its text.
        assert_eq!(
            arity("cc_gettext"),
            Some(Arity {
                int_pop: 0,
                str_pop: 0,
                long_pop: 0,
                int_push: 0,
                str_push: 1,
                long_push: 0,
            }),
        );
        // `parawidth`: pops the string plus 2 ints, pushes the width.
        assert_eq!(
            arity("parawidth"),
            Some(Arity {
                int_pop: 2,
                str_pop: 1,
                long_pop: 0,
                int_push: 1,
                str_push: 0,
                long_push: 0,
            }),
        );
        // `paraheight` (book id 389, hook 10058 `:32`): same shape as
        // `parawidth`; `paraline` pops `1o + 3i` and pushes one string.
        assert_eq!(
            arity("paraheight"),
            Some(Arity {
                int_pop: 2,
                str_pop: 1,
                long_pop: 0,
                int_push: 1,
                str_push: 0,
                long_push: 0,
            }),
        );
        assert_eq!(
            arity("paraline"),
            Some(Arity {
                int_pop: 3,
                str_pop: 1,
                long_pop: 0,
                int_push: 0,
                str_push: 1,
                long_push: 0,
            }),
        );
        // `chat_gethistory_byuid`: pops the uid; both branches push 4i+6s.
        assert_eq!(
            arity("chat_gethistory_byuid"),
            Some(Arity {
                int_pop: 1,
                str_pop: 0,
                long_pop: 0,
                int_push: 4,
                str_push: 6,
                long_push: 0,
            }),
        );
        // `_enum`: pops `(inputtype, outputtype, enumid, key)`; pushes one
        // int OR one string depending on the output type.
        assert_eq!(
            arity("_enum"),
            Some(Arity {
                int_pop: 4,
                str_pop: 0,
                long_pop: 0,
                int_push: 1,
                str_push: 1,
                long_push: 0,
            }),
        );
        // `format_datetime_from_minutes`: pops minutes, pushes the string.
        assert_eq!(
            arity("format_datetime_from_minutes"),
            Some(Arity {
                int_pop: 1,
                str_pop: 0,
                long_pop: 0,
                int_push: 0,
                str_push: 1,
                long_push: 0,
            }),
        );
        // `struct_param`: pops `(structid, paramid)`; pushes int or string.
        assert_eq!(
            arity("struct_param"),
            Some(Arity {
                int_pop: 2,
                str_pop: 0,
                long_pop: 0,
                int_push: 1,
                str_push: 1,
                long_push: 0,
            }),
        );
        // `stringwidth`: pops the string and the font id, pushes the width.
        assert_eq!(
            arity("stringwidth"),
            Some(Arity {
                int_pop: 1,
                str_pop: 1,
                long_pop: 0,
                int_push: 1,
                str_push: 0,
                long_push: 0,
            }),
        );
        // `compare`: pops two strings, pushes the ordering int.
        assert_eq!(
            arity("compare"),
            Some(Arity {
                int_pop: 0,
                str_pop: 2,
                long_pop: 0,
                int_push: 1,
                str_push: 0,
                long_push: 0,
            }),
        );
        // `cc_create`: pops 3 ints, pushes nothing.
        assert_eq!(
            arity("cc_create"),
            Some(Arity {
                int_pop: 3,
                str_pop: 0,
                long_pop: 0,
                int_push: 0,
                str_push: 0,
                long_push: 0,
            }),
        );
        // `if_setposition`: pops the component id plus the helper's 4 ints.
        assert_eq!(
            arity("if_setposition"),
            Some(Arity {
                int_pop: 5,
                str_pop: 0,
                long_pop: 0,
                int_push: 0,
                str_push: 0,
                long_push: 0,
            }),
        );
        // `random`: pops the bound, pushes the roll.
        assert_eq!(
            arity("random"),
            Some(Arity {
                int_pop: 1,
                str_pop: 0,
                long_pop: 0,
                int_push: 1,
                str_push: 0,
                long_push: 0,
            }),
        );
        // Pure ops and unknown names stay `None` (executed for real / trap
        // default), so the table never interferes with them.
        for pure in [
            "add",
            "branch",
            "branch_not",
            "push_constant_int",
            "push_constant_string",
            "gosub_with_params",
            "return",
            "tostring",
            "if_opentop",
            "no_such_command",
        ] {
            assert_eq!(arity(pure), None, "{pure} must not be a trap arity");
        }
    }

    /// The trap arity table ([`super::arity`], consumed by
    /// `preview::dispatch` to pre-pop engine args and top up pushes) against
    /// the committed stack-contract table (`cs2_stack_contracts.rs`). Every
    /// row with a fixed shape in the table must match it value by value; rows
    /// the table cannot state (hook setters with descriptor-sized tails,
    /// either/or pushes) keep their hand-checked spot checks above. Synthetic
    /// opcodes (> 1431, no original handler) stay zero-pop.
    #[test]
    fn arity_matches_stack_contract_table() {
        use super::super::opcode::OpcodeBook;
        use super::arity;
        use crate::semantics::Effect;
        let book = OpcodeBook::embedded().unwrap();
        let mut compared = 0_usize;
        let mut mismatches = Vec::new();
        let names: std::collections::BTreeSet<&str> = book.commands().collect();
        for command in names {
            // Duplicate names resolve to their real dispatch slot.
            let opcode = book.opcode_for(command).unwrap();
            let Some(row) = arity(command) else {
                // Every trapped interface command needs a row, or its args
                // leak (the table's reason to exist).
                assert!(
                    !(command.starts_with("if_") || command.starts_with("cc_")),
                    "trap arity missing for book command '{command}'"
                );
                continue;
            };
            let table = (
                [
                    u16::from(row.int_pop),
                    u16::from(row.str_pop),
                    u16::from(row.long_pop),
                ],
                [
                    u16::from(row.int_push),
                    u16::from(row.str_push),
                    u16::from(row.long_push),
                ],
            );
            if opcode > 1431 {
                assert_eq!(table.0, [0, 0, 0], "synthetic {command} must not pop");
                continue;
            }
            let Some(Effect::Fixed { pops, pushes }) = crate::cs2_stack_contracts::effect(command)
            else {
                continue;
            };
            compared += 1;
            if table != (pops, pushes) {
                mismatches.push(format!(
                    "{command}: table pops {:?} pushes {:?}, contract pops {pops:?} pushes {pushes:?}",
                    table.0, table.1
                ));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
        // 326 rows carry a fixed shape today; a collapse means the join broke.
        assert!(compared >= 320, "only {compared} rows compared");
    }

    /// Pack-free trap probe: answers `Int(-1)` to every engine call while
    /// recording the stack depths the trap observed (i.e. after the VM's
    /// pre-pop) — the same neutral-answer policy as the login host.
    #[derive(Default)]
    struct TrapProbe {
        seen_ints: usize,
        seen_objs: usize,
        seen_longs: usize,
        calls: usize,
    }

    impl Host for TrapProbe {
        fn trap_context(
            &mut self,
            context: &InstructionContext<'_>,
            ints: &mut Vec<i32>,
            strs: &mut Vec<String>,
            longs: &mut Vec<i64>,
        ) -> VmResult<Option<Value>> {
            crate::preview::dispatch(self, context, ints, strs, longs)
        }

        fn var_get(&mut self, _domain: VarScope, _id: u16, _transmog: bool) -> VmResult<Value> {
            Ok(Value::Int(0))
        }

        fn var_set(
            &mut self,
            _domain: VarScope,
            _id: u16,
            _transmog: bool,
            _value: Value,
        ) -> VmResult<()> {
            Ok(())
        }

        fn varbit_get(&mut self, _id: u16, _transmog: bool) -> VmResult<i32> {
            Ok(0)
        }

        fn varbit_set(&mut self, _id: u16, _transmog: bool, _value: i32) -> VmResult<()> {
            Ok(())
        }

        fn array_define(&mut self, array_id: i32, _len: usize) -> VmResult<()> {
            Err(VmError::BadArray {
                id: array_id,
                reason: "probe has no arrays".to_string(),
            })
        }

        fn array_len(&mut self, array_id: i32) -> VmResult<usize> {
            Err(VmError::BadArray {
                id: array_id,
                reason: "probe has no arrays".to_string(),
            })
        }

        fn array_get(&mut self, array_id: i32, _index: i32) -> VmResult<i32> {
            Err(VmError::BadArray {
                id: array_id,
                reason: "probe has no arrays".to_string(),
            })
        }

        fn array_set(&mut self, array_id: i32, _index: i32, _value: i32) -> VmResult<()> {
            Err(VmError::BadArray {
                id: array_id,
                reason: "probe has no arrays".to_string(),
            })
        }

        fn trap(
            &mut self,
            _command: &str,
            ints: &mut Vec<i32>,
            strs: &mut Vec<String>,
            longs: &mut Vec<i64>,
        ) -> VmResult<Option<Value>> {
            self.seen_ints = ints.len();
            self.seen_objs = strs.len();
            self.seen_longs = longs.len();
            self.calls += 1;
            Ok(Some(Value::Int(-1)))
        }
    }

    /// `cc_create` declares `int_pop: 3`: pushing 3 ints and trapping must
    /// leave the stack depth unchanged apart from the trap's own neutral
    /// answer (previously the 3 args leaked, one per call).
    #[test]
    fn trap_consumes_declared_args() {
        let prog = script(
            vec![
                instr("push_constant_int", Operand::Int(1)),
                instr("push_constant_int", Operand::Int(2)),
                instr("push_constant_int", Operand::Int(3)),
                instr("cc_create", Operand::Byte(0)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = TrapProbe::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]).unwrap();
        assert_eq!(out, Some(Value::Int(-1)));
        assert_eq!(host.calls, 1);
        assert_eq!(host.seen_ints, 0, "trap must see post-pop stacks");
        assert_eq!(host.seen_objs, 0);
        assert_eq!(host.seen_longs, 0);
    }

    /// Calling a 3-arg trap with only 2 values stacked is an honest
    /// [`VmError::StackUnderflow`], never a panic.
    #[test]
    fn trap_arg_underflow_errors_without_panic() {
        let prog = script(
            vec![
                instr("push_constant_int", Operand::Int(1)),
                instr("push_constant_int", Operand::Int(2)),
                instr("cc_create", Operand::Byte(0)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = TrapProbe::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let error = vm.execute(&prog, &[]).unwrap_err();
        assert!(
            matches!(error, VmError::StackUnderflow { stack: "int" }),
            "unexpected error: {error}"
        );
        assert_eq!(host.calls, 0, "trap must not run after underflow");
    }

    /// The hook-12611 sentinel shape (`push id; if_getlayer; push -1;
    /// branch_not`): the layer walk answers non-`-1` 1200 times and then the
    /// `-1` terminator. With per-op popping the walk holds a flat stack and
    /// returns; without it each pass leaks its id arg and breaches the
    /// 1000-slot cap before the terminator arrives.
    #[test]
    fn if_getlayer_sentinel_loop_terminates_without_overflow() {
        struct SentinelHost {
            calls: usize,
        }

        impl Host for SentinelHost {
            fn trap_context(
                &mut self,
                context: &InstructionContext<'_>,
                ints: &mut Vec<i32>,
                strs: &mut Vec<String>,
                longs: &mut Vec<i64>,
            ) -> VmResult<Option<Value>> {
                crate::preview::dispatch(self, context, ints, strs, longs)
            }

            fn var_get(&mut self, _domain: VarScope, _id: u16, _transmog: bool) -> VmResult<Value> {
                Ok(Value::Int(0))
            }

            fn var_set(
                &mut self,
                _domain: VarScope,
                _id: u16,
                _transmog: bool,
                _value: Value,
            ) -> VmResult<()> {
                Ok(())
            }

            fn varbit_get(&mut self, _id: u16, _transmog: bool) -> VmResult<i32> {
                Ok(0)
            }

            fn varbit_set(&mut self, _id: u16, _transmog: bool, _value: i32) -> VmResult<()> {
                Ok(())
            }

            fn array_define(&mut self, array_id: i32, _len: usize) -> VmResult<()> {
                Err(VmError::BadArray {
                    id: array_id,
                    reason: "probe has no arrays".to_string(),
                })
            }

            fn array_len(&mut self, array_id: i32) -> VmResult<usize> {
                Err(VmError::BadArray {
                    id: array_id,
                    reason: "probe has no arrays".to_string(),
                })
            }

            fn array_get(&mut self, array_id: i32, _index: i32) -> VmResult<i32> {
                Err(VmError::BadArray {
                    id: array_id,
                    reason: "probe has no arrays".to_string(),
                })
            }

            fn array_set(&mut self, array_id: i32, _index: i32, _value: i32) -> VmResult<()> {
                Err(VmError::BadArray {
                    id: array_id,
                    reason: "probe has no arrays".to_string(),
                })
            }

            fn trap(
                &mut self,
                _command: &str,
                _ints: &mut Vec<i32>,
                _strs: &mut Vec<String>,
                _longs: &mut Vec<i64>,
            ) -> VmResult<Option<Value>> {
                self.calls += 1;
                if self.calls <= 1200 {
                    Ok(Some(Value::Int(0)))
                } else {
                    Ok(Some(Value::Int(-1)))
                }
            }
        }

        let prog = script(
            vec![
                instr("push_constant_int", Operand::Int(0)),
                instr("if_getlayer", Operand::Byte(0)),
                instr("push_constant_int", Operand::Int(-1)),
                instr("branch_not", Operand::Branch(0)),
                instr("push_constant_int", Operand::Int(42)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = SentinelHost { calls: 0 };
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]);
        assert_eq!(out.unwrap(), Some(Value::Int(42)));
        assert_eq!(host.calls, 1201, "walk must run to the terminator");
    }

    /// `userdetail_lobby_membership` pushes 3 ints
    /// but the neutral login host answers a single `Int(-1)`: the hook-2601
    /// fragment (`:18-21`, reached from hook 10058) pops three int locals
    /// right after the call, which underflowed before the post-trap top-up.
    #[test]
    fn trap_top_up_multi_push_userdetail_shape() {
        let prog = script(
            vec![
                instr("userdetail_lobby_membership", Operand::Byte(0)),
                instr("pop_int_local", Operand::Local(0)),
                instr("pop_int_local", Operand::Local(1)),
                instr("pop_int_local", Operand::Local(2)),
                instr("push_int_local", Operand::Local(0)),
                instr("push_int_local", Operand::Local(1)),
                instr("push_int_local", Operand::Local(2)),
                instr("add", Operand::Byte(0)),
                instr("add", Operand::Byte(0)),
                instr("return", Operand::Byte(0)),
            ],
            3,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = TrapProbe::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]);
        assert_eq!(out.unwrap(), Some(Value::Int(-3)));
    }

    /// `escape` pops a string and pushes one
    /// but the neutral host answers `Int(-1)`: the hook-3161 fragment
    /// (`:29-32`, reached from hook 10022) needs the object back for the
    /// following `if_settext`-style consumer, which underflowed before the
    /// top-up supplied the declared string push.
    #[test]
    fn trap_top_up_string_escape_shape() {
        let prog = script(
            vec![
                instr("push_constant_string", Operand::Str("a<b".to_string())),
                instr("escape", Operand::Byte(0)),
                instr("pop_string_local", Operand::Local(0)),
                instr("push_string_local", Operand::Local(0)),
                instr("string_length", Operand::Byte(0)),
                instr("return", Operand::Byte(0)),
            ],
            0,
            1,
            0,
            0,
            0,
            0,
        );
        let mut host = TrapProbe::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]);
        assert_eq!(out.unwrap(), Some(Value::Int(0)));
    }

    /// First-touch `pop_var` on the login tree's string client vars must pop
    /// the object stack even though a fresh host defaults every unread var to
    /// `Int`: hook 4554 (`push ""` at `:105`, `pop_var(client:2480)` at
    /// `:106`, re-push for the gosub string arg at `:119-120`) and hook 3159
    /// (same shape for `client:2479` at `:109-113` / `:123`) underflowed the
    /// object stack at the gosub/`escape` once the var was mis-typed as int.
    #[test]
    fn pop_var_string_default_roundtrip() {
        use super::super::script::VarRef;
        use super::super::vars::VarScope as Domain;
        let string_var = |id| {
            Operand::VarRef(VarRef {
                domain: Domain::Client,
                id,
                transmog: false,
            })
        };
        let prog = script(
            vec![
                instr("push_constant_string", Operand::Str("hi".to_string())),
                instr("pop_var", string_var(2480)),
                instr("push_var", string_var(2480)),
                instr("string_length", Operand::Byte(0)),
                instr("pop_int_local", Operand::Local(0)),
                instr("push_constant_string", Operand::Str("hey".to_string())),
                instr("pop_var", string_var(2479)),
                instr("push_var", string_var(2479)),
                instr("string_length", Operand::Byte(0)),
                instr("push_int_local", Operand::Local(0)),
                instr("add", Operand::Byte(0)),
                instr("return", Operand::Byte(0)),
            ],
            1,
            0,
            0,
            0,
            0,
            0,
        );
        let mut host = MemHost::default();
        let provider = HashMap::new();
        let mut vm = Vm::new(&mut host, &provider);
        let out = vm.execute(&prog, &[]);
        assert_eq!(out.unwrap(), Some(Value::Int(5)));
    }

    #[test]
    fn string_indices_use_utf16_units() {
        assert_eq!(super::indexof_string("a😀z", "z", 0), 3);
        assert_eq!(super::indexof_char("a😀z", 0x1f600, 0), 1);
        assert_eq!(super::indexof_string("a😀z", "", 99), 4);
        assert_eq!(super::substring("substring", "a😀z", 1, 3).unwrap(), "😀");
        let high = super::substring("substring", "a😀z", 1, 2).unwrap();
        assert_eq!(crate::jstr::units(&high), vec![0xD83D]);
    }
}
