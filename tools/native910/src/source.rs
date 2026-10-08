//! Native source model for 910 scripts: the editable form between text and bytes.
//!
//! [`SourceScript`] mirrors [`CompiledScript`](crate::script::CompiledScript)
//! with two human-scale changes: branch targets are labels instead of absolute
//! indices (inserting an instruction never renumbers jumps by hand), and local
//! slots are `$names` instead of numbers (counts derive from declarations, so
//! they cannot drift). Commands stay verbatim — the 910 vocabulary is the
//! language's stdlib — and every generic operand is written explicitly.
//!
//! Canonical text form (what [`format_source`] emits; the parser accepts the
//! same shape with free blank lines and comments):
//!
//! ```text
//! [clientscript,example](int $param0)
//! int $temp0, $temp1;
//! string $temp2;
//!
//!     push_constant_int(41);
//!     push_int_local($param0);
//!     add(0);
//!     pop_int_local($temp0);
//!     branch_if_false(end);
//!     push_constant_string("done");
//!     pop_string_local($temp2);
//! end:
//!     return(0);
//! ```
//!
//! Rules: header `[tag](type $arg, ...)` (empty parens when arg-less, `[]`
//! when anonymous); one `int|string|long $a, $b;` declaration line per
//! non-empty local group; a blank line; then statements indented four spaces
//! with labels at column 0. Switch blocks:
//!
//! ```text
//!     switch {
//!         case 0 -> found;
//!         case -1 -> other;
//!     }
//! ```
//!
//! [`lift`] names generated slots `$argN`/`$localN` (numbered across types in
//! int/string/long order); hand authors rename freely. [`lower`] re-derives
//! slots (args occupy each type's first slots, in header order) and resolves
//! labels, so the text has no countable quantities to get wrong except the
//! values themselves.
//!
//! Calls to registry-named scripts fold into `~name(args);` statements:
//!
//! ```text
//!     ~bank_build_init(41, $total, "main");
//! ```
//!
//! Folding is exact or not at all: the preceding argument run must be pure
//! constant/local pushes whose type MULTISET matches the callee signature
//! (cross-type push order is free on the client's separate typed stacks, so
//! original order is preserved, never canonicalized), with no jump target
//! strictly inside and none on the gosub itself (which would run it against
//! an ambient stack). Computed arguments (`add` results, var reads, command
//! results) stay an explicit push sequence with a numeric `gosub` — folding
//! those needs typed stack-effect recovery, a later milestone.
//!
//! Pure computations lift into expression assignments:
//!
//! ```text
//!     $temp0 = $param0 + 41;
//!     $temp2 = ~bank_balance($param0);
//! ```
//!
//! A `pop_*_local` terminating a straight-line run of canonical pushes
//! (`push_constant_string`-family constants, typed `push_*_local`), verified
//! operators (`add`, `min`, `compare`, `not`, ... — every fixed-lane shape
//! read off the shared effect contract, never a local table), config-typed
//! reads (`struct_param` and siblings, `_enum` — fixed int pops with the push
//! lane resolved through [`ConfigTypes`], same as [`lower`]), and nested
//! single-value calls becomes one `$target = <expr>;`, the exact inverse of
//! what [`lower`] re-emits. A folded `~name(args)` call fuses with its
//! consuming pop the same way when the callee has exactly one statically
//! inferred return value of the popped type, and nests inside a larger
//! operator tree (for example `push a; push b; gosub f; push c; add; pop $y`
//! becomes `$y = ~f(a, b) + $c;`) under the same single-value rule. Labels,
//! branches, multi/unknown-return calls, unknown config ids, non-literal
//! config ids, and anything outside the closed leaf/operator/call sets ends
//! a run: lifting is all-or-nothing per pop, so whatever it cannot rebuild
//! byte-exactly stays flat.
//!
//! Nested-call drift is closed by provenance, not by re-reading pushes: a
//! folded `Call` statement's arguments came from `fold_kind` pushes that may
//! include `push_constant_int` (which [`lower`](crate::expr::lower_expr)
//! re-emits as the const-string family — different bytes) or varbits (no
//! expression syntax at all), while a nested [`Expr::Call`](crate::expr::Expr::Call)
//! re-lowers through `lower_expr`. The post-pass no longer sees the original
//! pushes, so the main loop records one canonical-spelling bit per folded
//! call (true only when every folded push used the const-string/local
//! spellings `lower_expr` re-emits); the assignment folder nests only
//! canonical calls with expression-mappable arguments (zero-arg calls nest
//! freely — no arguments, no drift). Anything else stays a statement and
//! ends the window. When in doubt, flat.
//!
//! Lowered argument literals use the corpus-measured `push_constant_string`
//! family spellings (int-tagged ints are the uniform arg convention — bare
//! `push_constant_int` never appears as a call argument); `$locals` lower
//! through their declared types.
//!
//! Packed component ids spell symbolically wherever an int literal fits:
//!
//! ```text
//!     push_constant_string(bank/main);
//!     push_constant_int(bank/7);
//!     $cid = bank/main;
//! ```
//!
//! The shape is NAME-led only — interface name + `/` + child name-or-number —
//! so `1253/4` never collides (in expressions it stays division; in statement
//! operands it fails as a bad int). Parsing validates the pair against the
//! [`InterfaceRegistry`](crate::inames::InterfaceRegistry) roster carried
//! alongside the symbol tables; formatting renders the symbolic shape whenever
//! the registry names the interface, else the packed decimal. Both spellings
//! lower to the identical `Operand::Int(packed)`, and each spelling
//! round-trips through its own shape (`format(parse(x)) == x` both ways:
//! numeric text never auto-symbolizes, so hand-written numbers stay numbers).

use crate::config::ConfigTypes;
use crate::effects::{Effect, effect};
use crate::error::{NativeError, Result};
use crate::expr::{BinaryOp, Expr, MIN_JOIN_ARITY, NaryOp, ParamOp, UnaryOp};
use crate::inames::InterfaceRegistry;
use crate::opcode::OpcodeBook;
use crate::script::{
    CompiledScript, Counts, Instruction, Operand, SwitchCase, VarBitRef, VarRef, decode_script,
    encode_script, is_branch_command,
};
use crate::symbols::SymbolRegistry;
use crate::vars::VarScope;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// Whether text is a valid source name (labels, `$locals`, `~calls` alike):
/// ASCII alphanumeric/underscore, never starting with a digit.
#[must_use]
pub fn is_valid_name(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    text.chars()
        .all(|char| char.is_ascii_alphanumeric() || char == '_')
}

/// A value type: int, object (string), or long.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValType {
    /// 32-bit int.
    Int,
    /// Object (string).
    String,
    /// 64-bit long.
    Long,
}

impl ValType {
    /// Source keyword for the type.
    #[must_use]
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::String => "string",
            Self::Long => "long",
        }
    }

    /// Parse a source keyword.
    pub fn parse_keyword(word: &str) -> Result<Self> {
        match word {
            "int" => Ok(Self::Int),
            "string" => Ok(Self::String),
            "long" => Ok(Self::Long),
            _ => Err(NativeError::Invalid(format!(
                "unknown type '{word}' (expected int, string, or long)"
            ))),
        }
    }
}

/// A named argument or local.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Param {
    /// Value type.
    pub ty: ValType,
    /// `$`-less name (`param0`, not `$param0`).
    pub name: String,
}

/// A script in editable form.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceScript {
    /// Raw tag text (`clientscript,example`), or `None` when anonymous.
    pub name: Option<String>,
    /// Arguments, in slot order per type.
    pub args: Vec<Param>,
    /// Locals, in slot order per type.
    pub locals: Vec<Param>,
    /// Labeled instruction stream.
    pub body: Vec<SourceStmt>,
}

/// One source statement: a label definition, an instruction, or an
/// expression assignment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceStmt {
    /// Jump target definition.
    Label(String),
    /// One instruction.
    Instr(SourceInstr),
    /// Expression assignment (`$name = <expr>;`): surface syntax for a
    /// push/operate/pop run. [`lower`] expands it before label indexing, and
    /// [`lift`] emits it for pure runs and single-value calls (see
    /// [`try_fold_assign`] and [`try_fold_value_call`).
    Assign {
        /// `$`-less target name.
        target: String,
        /// Right-hand-side computation.
        expr: crate::expr::Expr,
    },
}

/// One source instruction: verbatim command plus a symbolic operand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceInstr {
    /// Canonical 910 command (`branch`, never the `jump` sugar).
    pub command: String,
    /// Symbolic operand.
    pub operand: SourceOperand,
}

/// A symbolic instruction operand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceOperand {
    /// Int literal (`42`).
    Int(i32),
    /// Packed component reference (`bank/7`, `bank/main`): the symbolic
    /// spelling of the packed `iface << 16 | child` int, valid only on the
    /// int-literal slots of `push_constant_string`/`push_constant_int`.
    /// Parsing validates the pair against the interface roster; lowering
    /// packs it to the identical `Operand::Int` the numeric spelling yields.
    /// Formatting renders the symbolic shape when the registry names the
    /// interface, else the packed decimal (never numeric/numeric, which would
    /// reparse as division).
    Component {
        /// Interface (pack group) id.
        iface: i32,
        /// Child (pack file) index.
        child: u32,
    },
    /// Long literal (`42L`).
    Long(i64),
    /// String literal (`"text"`).
    Str(String),
    /// Named local or argument slot (`$total`).
    Local(String),
    /// Variable reference (`player:1234`, `player:1234:transmog`).
    Var(VarScope, u16, bool),
    /// Varbit reference (`varbit:5678`, `varbit:5678:transmog`).
    VarBit(u16, bool),
    /// Branch target label.
    Label(String),
    /// Switch cases (`value -> label`).
    Cases(Vec<(i32, String)>),
    /// Callee script id (`gosub_with_params`).
    Script(i32),
    /// A call to a named script with explicit arguments (`~bank(41, $x)`).
    /// Arguments keep their original push order; the client's separate typed
    /// stacks make cross-type order irrelevant, so validation matches the
    /// type MULTISET against the callee signature, not positions.
    Call(String, Vec<CallArg>),
    /// Array id.
    Array(i32),
    /// Part count (`join_string`).
    Count(i32),
    /// Generic operand slot, written explicitly (`add(0)`).
    Raw(i32),
}

/// One call argument: a literal, a `$local`, or a varbit read — the forms
/// provably pushing exactly one statically-typed value. Anything computed
/// (`add` results, var reads, command results) stays outside folded calls:
/// folding those needs typed stack-effect recovery, a later milestone.
/// Varbits qualify (always int-valued); plain vars do not (per-id types live
/// in config data, milestone 3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallArg {
    /// Int literal.
    Int(i32),
    /// Long literal.
    Long(i64),
    /// String literal.
    Str(String),
    /// Named local or argument slot.
    Local(String),
    /// Varbit read (`varbit:123`, `varbit:123:transmog`).
    VarBit(u16, bool),
}

/// Lift a decoded script to editable form. Fails on models the source language
/// cannot express (a command carrying an operand kind outside its class), so
/// only genuine scripts round-trip — never silently reshaped ones.
///
/// Calls to registry-named callees fold their argument pushes into `~name()`
/// statements when the pattern is exact (see [`try_fold_call`]); everything
/// else — unnamed callees, computed arguments, jumps into the argument run —
/// stays an explicit push sequence with a numeric `gosub`.
///
/// `configs` resolves the config-typed family during assignment folding (see
/// [`try_fold_assign`]): an empty [`ConfigTypes`] lifts every fixed-lane
/// window as before but leaves every `struct_param`/`_enum` window flat
/// (unknown lane, never guessed).
///
/// `inames` symbolizes packed component ids between call folding and
/// assignment folding (see [`symbolize_components`]): ints the roster holds
/// become [`SourceOperand::Component`] (rendering `bank/7`-style where named,
/// packed decimals elsewhere), so dumps read symbolically without changing a
/// byte. An empty registry lifts exactly as before.
pub fn lift(
    script: &CompiledScript,
    symbols: &SymbolRegistry,
    configs: &ConfigTypes,
    inames: &InterfaceRegistry,
) -> Result<SourceScript> {
    let mut arg_counter = 0_usize;
    let mut local_counter = 0_usize;
    let mut int_names: Vec<String> = Vec::new();
    let mut obj_names: Vec<String> = Vec::new();
    let mut long_names: Vec<String> = Vec::new();
    for _ in 0..script.args.int {
        int_names.push(arg_name(&mut arg_counter));
    }
    for _ in 0..script.args.obj {
        obj_names.push(arg_name(&mut arg_counter));
    }
    for _ in 0..script.args.long {
        long_names.push(arg_name(&mut arg_counter));
    }
    let int_arg_count = int_names.len();
    let obj_arg_count = obj_names.len();
    for _ in 0..script
        .locals
        .int
        .checked_sub(script.args.int)
        .ok_or_else(|| NativeError::Invalid("argument count exceeds local count".to_string()))?
    {
        int_names.push(local_name(&mut local_counter));
    }
    for _ in 0..script
        .locals
        .obj
        .checked_sub(script.args.obj)
        .ok_or_else(|| NativeError::Invalid("argument count exceeds local count".to_string()))?
    {
        obj_names.push(local_name(&mut local_counter));
    }
    for _ in 0..script
        .locals
        .long
        .checked_sub(script.args.long)
        .ok_or_else(|| NativeError::Invalid("argument count exceeds local count".to_string()))?
    {
        long_names.push(local_name(&mut local_counter));
    }

    let mut args = Vec::new();
    for name in int_names.iter().take(int_arg_count) {
        args.push(Param {
            ty: ValType::Int,
            name: name.clone(),
        });
    }
    for name in obj_names.iter().take(obj_arg_count) {
        args.push(Param {
            ty: ValType::String,
            name: name.clone(),
        });
    }
    // Long args and all locals follow the same take/skip pattern over their
    // per-type name tables (args occupy each type's first slots).
    let long_arg_count = usize::from(script.args.long);
    for name in long_names.iter().take(long_arg_count) {
        args.push(Param {
            ty: ValType::Long,
            name: name.clone(),
        });
    }
    let mut locals = Vec::new();
    for name in int_names.iter().skip(int_arg_count) {
        locals.push(Param {
            ty: ValType::Int,
            name: name.clone(),
        });
    }
    for name in obj_names.iter().skip(obj_arg_count) {
        locals.push(Param {
            ty: ValType::String,
            name: name.clone(),
        });
    }
    for name in long_names.iter().skip(long_arg_count) {
        locals.push(Param {
            ty: ValType::Long,
            name: name.clone(),
        });
    }

    let mut targets = BTreeSet::new();
    for instr in &script.code {
        match &instr.operand {
            Operand::Branch(target) => {
                targets.insert(*target);
            }
            Operand::Switch(cases) => {
                for case in cases {
                    targets.insert(case.target);
                }
            }
            _ => {}
        }
    }

    let mut body = Vec::new();
    // Parallel to `body`: each entry's pushed-value kind when it is a pure
    // single push (`None` for labels and everything else). Drives call folding.
    let mut push_kinds: Vec<Option<FoldKind>> = Vec::new();
    // Parallel to `body`: whether a folded `Call` statement used only the
    // canonical const-string/local push spellings `lower_expr` re-emits, so
    // the assignment folder may nest it as an `Expr::Call` without drifting
    // bytes (see the module docs). Every non-`Call` entry is `false`; a
    // skipped pop never gets an entry (it carries no jump target, like the
    // fused assignment itself — see `try_fold_value_call`).
    let mut call_canonical: Vec<bool> = Vec::new();
    let names = SlotNames {
        ints: &int_names,
        objs: &obj_names,
        longs: &long_names,
    };
    let ctx = LiftCtx {
        targets: &targets,
        symbols,
        code: &script.code,
        names: &names,
    };
    // Set when a folded value call consumes the following pop as well: the
    // next instruction is already represented, so the loop skips it (it
    // carries no jump target by construction — see `try_fold_value_call`).
    let mut skip_next = false;
    for (position, instr) in script.code.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        let index = i32::try_from(position)
            .map_err(|_| NativeError::Invalid("script too large".to_string()))?;
        if targets.contains(&index) {
            body.push(SourceStmt::Label(label_for(index)));
            push_kinds.push(None);
            call_canonical.push(false);
        }
        if instr.command == "gosub_with_params"
            && let Operand::Script(callee) = &instr.operand
            && let Some((name, args, drop)) = try_fold_call(*callee, index, &push_kinds, &ctx)?
        {
            for _ in 0..drop {
                body.pop();
                push_kinds.pop();
                call_canonical.pop();
            }
            if let Some(assign) =
                try_fold_value_call(&name, &args, drop, *callee, index, position, &ctx)
            {
                body.push(assign);
                push_kinds.push(None);
                call_canonical.push(false);
                skip_next = true;
                continue;
            }
            let canonical = folded_range_is_canonical(drop, index, &ctx);
            body.push(SourceStmt::Instr(SourceInstr {
                command: "gosub_with_params".to_string(),
                operand: SourceOperand::Call(name, args),
            }));
            push_kinds.push(None);
            call_canonical.push(canonical);
            continue;
        }
        let kind = fold_kind(&instr.command, &instr.operand);
        body.push(SourceStmt::Instr(SourceInstr {
            command: instr.command.clone(),
            operand: lift_operand(&instr.command, &instr.operand, &names)?,
        }));
        push_kinds.push(kind);
        call_canonical.push(false);
    }
    // Component symbolization runs between call folding (which reads the
    // binary stream, so its integer arguments stay numeric) and assignment
    // folding (so `lift_leaf` rebuilds symbolized pushes as symbolic leaves).
    symbolize_components(&mut body, inames);
    fold_assignments(&mut body, symbols, configs, &mut call_canonical);

    Ok(SourceScript {
        name: script.name.clone(),
        args,
        locals,
        body,
    })
}

/// A single pushed value's stack type, for call folding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FoldKind {
    Int,
    Obj,
    Long,
}

/// Which pure single pushes an instruction is, if any. Only constants, locals,
/// and varbit reads qualify: anything computed (`add`, `_enum`, `push_var`,
/// command results, `join_string`) stays outside folded calls in v1 — folding
/// those needs typed stack-effect recovery, a later milestone.
fn fold_kind(command: &str, operand: &Operand) -> Option<FoldKind> {
    match operand {
        Operand::Int(_) => match command {
            "push_constant_int" | "push_constant_string" => Some(FoldKind::Int),
            _ => None,
        },
        Operand::Long(_) => match command {
            "push_constant_string" => Some(FoldKind::Long),
            _ => None,
        },
        Operand::Str(_) => match command {
            "push_constant_string" => Some(FoldKind::Obj),
            _ => None,
        },
        Operand::Local(_) => match command {
            "push_int_local" => Some(FoldKind::Int),
            "push_string_local" => Some(FoldKind::Obj),
            "push_long_local" => Some(FoldKind::Long),
            _ => None,
        },
        Operand::VarBitRef(_) => match command {
            "push_varbit" => Some(FoldKind::Int),
            _ => None,
        },
        _ => None,
    }
}

/// Fold a `gosub` and its argument pushes into a `Call` when the pattern is
/// exact: the callee is registry-named with a known signature, the preceding
/// `arity` emitted statements are all pure pushes whose type MULTISET matches
/// the signature (cross-type order is free on the client's separate typed
/// stacks), and no jump target lands strictly inside the folded run (a
/// boundary label stays put on the call itself). Returns the name, the
/// arguments in original push order, and how many emitted statements to drop.
/// Anything less than exact yields `None` — the call stays numeric.
fn try_fold_call(
    callee: i32,
    gosub_index: i32,
    push_kinds: &[Option<FoldKind>],
    ctx: &LiftCtx<'_>,
) -> Result<Option<(String, Vec<CallArg>, usize)>> {
    let (Some(name), Some(sig)) = (
        ctx.symbols.name_for_call(callee),
        ctx.symbols.args_of(callee),
    ) else {
        return Ok(None);
    };
    let arity = usize::from(sig.int) + usize::from(sig.obj) + usize::from(sig.long);
    if arity == 0 {
        return Ok(Some((name.to_string(), Vec::new(), 0)));
    }
    // Walk the emitted tail over pure pushes only: a label or any other
    // statement ends the run (a label between pushes marks a mid-run target,
    // which the range check below would reject anyway).
    let mut kinds = Vec::with_capacity(arity);
    for kind in push_kinds.iter().rev() {
        if kinds.len() == arity {
            break;
        }
        match kind {
            Some(kind) => kinds.push(*kind),
            None => break,
        }
    }
    if kinds.len() != arity {
        return Ok(None);
    }
    let mut ints = 0_usize;
    let mut objs = 0_usize;
    let mut longs = 0_usize;
    for kind in &kinds {
        match kind {
            FoldKind::Int => ints += 1,
            FoldKind::Obj => objs += 1,
            FoldKind::Long => longs += 1,
        }
    }
    if ints != usize::from(sig.int)
        || objs != usize::from(sig.obj)
        || longs != usize::from(sig.long)
    {
        return Ok(None);
    }
    let start = gosub_index - arity as i32;
    // No jump target strictly inside the folded run — and none on the gosub
    // itself, which would run it against an ambient stack the pushes never
    // built. A target exactly on `start` is fine (the run still executes
    // whole from there).
    if ctx
        .targets
        .iter()
        .any(|at| *at > start && *at <= gosub_index)
    {
        return Ok(None);
    }
    // Rebuild the arguments oldest-first from the folded instructions. The
    // range is safe: exactly `arity` emitted pushes precede the gosub with no
    // labels between, so these code indices exist and are contiguous.
    let mut args = Vec::with_capacity(arity);
    let range = (start as usize)..(gosub_index as usize);
    for instr in &ctx.code[range] {
        let lifted = lift_operand(&instr.command, &instr.operand, ctx.names)?;
        match lifted {
            SourceOperand::Int(value) => args.push(CallArg::Int(value)),
            SourceOperand::Long(value) => args.push(CallArg::Long(value)),
            SourceOperand::Str(text) => args.push(CallArg::Str(text)),
            SourceOperand::Local(name) => args.push(CallArg::Local(name)),
            SourceOperand::VarBit(id, transmog) => args.push(CallArg::VarBit(id, transmog)),
            _ => return Ok(None),
        }
    }
    Ok(Some((name.to_string(), args, arity)))
}

/// Everything a lift-time folding helper needs beyond the statement under
/// inspection: jump targets, the registry (names, signatures, returns), the
/// decoded instruction stream, and the slot-name tables. Bundled so the
/// helpers stay under the argument-count lint.
struct LiftCtx<'a> {
    targets: &'a BTreeSet<i32>,
    symbols: &'a SymbolRegistry,
    code: &'a [Instruction],
    names: &'a SlotNames<'a>,
}

/// A canonical push in the const-string/local family: exactly what
/// [`lower_expr`](crate::expr::lower_expr) re-emits, so a folded call built
/// only from these re-lowers byte-identically as a nested [`Expr::Call`](crate::expr::Expr).
/// Every other push spelling (`push_constant_int`, `push_long_constant`,
/// `push_varbit`, ...) has different bytes or no expression syntax.
fn is_canonical_push(command: &str) -> bool {
    matches!(
        command,
        "push_constant_string" | "push_int_local" | "push_string_local" | "push_long_local"
    )
}

/// Whether the `drop` pushes folded into the gosub at `gosub_index` all used
/// the canonical spellings above. Zero-arg calls hold vacuously (no
/// arguments, no drift). Anything unprovable is `false` — the caller bails to
/// flat, never guesses. Uses fallible indexing so a corrupt range bails
/// instead of panicking.
fn folded_range_is_canonical(drop: usize, gosub_index: i32, ctx: &LiftCtx<'_>) -> bool {
    let Ok(drop_count) = i32::try_from(drop) else {
        return false;
    };
    let Some(start) = gosub_index.checked_sub(drop_count) else {
        return false;
    };
    let (Ok(start_usize), Ok(end_usize)) = (usize::try_from(start), usize::try_from(gosub_index))
    else {
        return false;
    };
    let Some(slice) = ctx.code.get(start_usize..end_usize) else {
        return false;
    };
    slice
        .iter()
        .all(|instr| is_canonical_push(instr.command.as_str()))
}

/// Fuse a just-folded `~name(args)` call with the pop consuming its result
/// into one `$target = ~name(args);` assignment, when the pattern is exact:
/// the next instruction pops one local, no jump targets that pop, the callee
/// has exactly one statically inferred return value of the popped type, every
/// argument maps to expression syntax (varbit reads have none), and every
/// folded push used the canonical const-string/local spellings the assignment
/// lowerer re-emits (a bare `push_constant_int` argument would drift bytes).
/// Anything less than exact yields `None` — the call stays a statement and
/// the pop stays flat. The caller skips the consumed pop instruction.
fn try_fold_value_call(
    name: &str,
    args: &[CallArg],
    drop: usize,
    callee: i32,
    gosub_index: i32,
    position: usize,
    ctx: &LiftCtx<'_>,
) -> Option<SourceStmt> {
    let next = ctx.code.get(position + 1)?;
    let (pop_ty, slot) = match (next.command.as_str(), &next.operand) {
        ("pop_int_local", Operand::Local(slot)) => (ValType::Int, *slot),
        ("pop_string_local", Operand::Local(slot)) => (ValType::String, *slot),
        ("pop_long_local", Operand::Local(slot)) => (ValType::Long, *slot),
        _ => return None,
    };
    if ctx.targets.contains(&(gosub_index + 1)) {
        return None;
    }
    // The name just came out of the registry, so resolution cannot fail in
    // practice; unknown returns (or a void/multi value) still bail, leaving
    // the call as a statement.
    let id = ctx.symbols.resolve_call(name).filter(|id| *id == callee)?;
    let result = crate::expr::single_value(ctx.symbols.returns_of(id)?)?;
    if result != pop_ty {
        return None;
    }
    // Canonical pushes only (see the doc above): the folded range holds
    // exactly `drop` instructions ending at the gosub.
    if !folded_range_is_canonical(drop, gosub_index, ctx) {
        return None;
    }
    let mut expr_args = Vec::with_capacity(args.len());
    for arg in args {
        let expr = match arg {
            CallArg::Int(value) => Expr::LitInt(*value),
            CallArg::Long(value) => Expr::LitLong(*value),
            CallArg::Str(text) => Expr::LitStr(text.clone()),
            CallArg::Local(local) => Expr::Local(local.clone()),
            CallArg::VarBit(..) => return None,
        };
        expr_args.push(expr);
    }
    let table = match pop_ty {
        ValType::Int => ctx.names.ints,
        ValType::String => ctx.names.objs,
        ValType::Long => ctx.names.longs,
    };
    let Ok(target) = lifted_slot_name(table, slot, &next.command) else {
        return None;
    };
    Some(SourceStmt::Assign {
        target: target.to_string(),
        expr: Expr::Call {
            name: name.to_string(),
            args: expr_args,
        },
    })
}

/// A folded `~name(args)` call as an expression leaf inside a larger
/// assignment window, if it is provably re-lowerable: the main loop marked it
/// canonical (every folded push used the const-string/local spellings
/// `lower_expr` re-emits — a bare `push_constant_int` or `push_varbit` would
/// drift bytes or has no syntax), the command is the gosub itself, every
/// argument maps to expression syntax, and the callee has exactly one
/// statically inferred return value (unknown, void, and multi-value callees
/// can never sit in expression position). Anything else yields `None` and
/// ends the window — all-or-nothing per pop, like everything else.
fn lift_call_leaf(
    instr: &SourceInstr,
    is_canonical: bool,
    symbols: &SymbolRegistry,
) -> Option<(ValType, Expr)> {
    if !is_canonical {
        return None;
    }
    if instr.command != "gosub_with_params" {
        return None;
    }
    let SourceOperand::Call(name, args) = &instr.operand else {
        return None;
    };
    let id = symbols.resolve_call(name)?;
    let result = crate::expr::single_value(symbols.returns_of(id)?)?;
    let mut expr_args = Vec::with_capacity(args.len());
    for arg in args {
        let expr = match arg {
            CallArg::Int(value) => Expr::LitInt(*value),
            CallArg::Long(value) => Expr::LitLong(*value),
            CallArg::Str(text) => Expr::LitStr(text.clone()),
            CallArg::Local(local) => Expr::Local(local.clone()),
            CallArg::VarBit(..) => return None,
        };
        expr_args.push(expr);
    }
    Some((
        result,
        Expr::Call {
            name: name.clone(),
            args: expr_args,
        },
    ))
}

/// Rewrite roster-known packed ints into symbolic component operands: every
/// `push_constant_string`/`push_constant_int` statement whose int the roster
/// holds becomes [`SourceOperand::Component`]. Runs AFTER call folding (which
/// reads the binary stream, so folded `~call` integer arguments stay numeric,
/// the status-quo spelling there) but BEFORE assignment folding (so
/// [`lift_leaf`] rebuilds them as symbolic expression leaves). Rendering
/// decides symbolic-vs-numeric per pair at format time; an empty registry
/// rewrites nothing, and every rewrite re-lowers to the identical
/// `Operand::Int`, so bytes never move.
fn symbolize_components(body: &mut [SourceStmt], inames: &InterfaceRegistry) {
    for stmt in body.iter_mut() {
        let SourceStmt::Instr(instr) = stmt else {
            continue;
        };
        if !matches!(
            instr.command.as_str(),
            "push_constant_string" | "push_constant_int"
        ) {
            continue;
        }
        let SourceOperand::Int(value) = &instr.operand else {
            continue;
        };
        if let Some((iface, child)) = inames.unpack_known(*value) {
            instr.operand = SourceOperand::Component { iface, child };
        }
    }
}

/// Fold pure push/operate/call runs into expression assignments: after the
/// main lift loop, every `pop_*_local` terminating a straight-line run of
/// canonical pushes, verified operators, and provably-canonical single-value
/// calls becomes `$target = <expr>;` (see [`try_fold_assign`]). Labels,
/// non-canonical or multi/unknown-return calls, branches, and anything
/// outside the closed leaf/operator/call sets ends a run — folding is
/// all-or-nothing per pop, so whatever cannot be rebuilt byte-exactly stays
/// flat.
fn fold_assignments(
    body: &mut Vec<SourceStmt>,
    symbols: &SymbolRegistry,
    configs: &ConfigTypes,
    canonical: &mut Vec<bool>,
) {
    let mut out: Vec<SourceStmt> = Vec::with_capacity(body.len());
    let mut out_canonical: Vec<bool> = Vec::with_capacity(canonical.len());
    for (stmt, is_canonical) in body.drain(..).zip(canonical.drain(..)) {
        out.push(stmt);
        out_canonical.push(is_canonical);
        if let Some((assign, consumed)) = try_fold_assign(&out, &out_canonical, symbols, configs) {
            out.truncate(out.len() - consumed);
            out_canonical.truncate(out_canonical.len() - consumed);
            out.push(assign);
            // Assignments barrier later windows (they are not leaves), so
            // they carry no canonical bit.
            out_canonical.push(false);
        }
    }
    *body = out;
    *canonical = out_canonical;
}

/// Try to fold the tail of `stmts` (which must end in a `pop_*_local`) into
/// one assignment: walk back over leaves, nested single-value calls, and
/// verified operators tracking stack depth, then rebuild the tree forward
/// with full type checks. `canonical` runs parallel to `stmts` (see
/// [`fold_assignments`]): a `Call` nests only when its bit is set, proving
/// its folded pushes used the const-string/local spellings `lower_expr`
/// re-emits. `configs` resolves the config-typed family in the forward pass
/// (unknown or non-literal ids bail to flat, never guessed). Returns the
/// assignment and how many trailing statements it consumed. This is the exact
/// inverse of [`lower_expr`](crate::expr::lower_expr) emission order (leaves,
/// then left, right, operator; calls lower their arguments in order, then the
/// gosub; config reads lower holder/type pushes in listed order with the
/// literal id push in place, then the command), so a folded window re-lowers
/// to the identical instruction stream.
fn try_fold_assign(
    stmts: &[SourceStmt],
    canonical: &[bool],
    symbols: &SymbolRegistry,
    configs: &ConfigTypes,
) -> Option<(SourceStmt, usize)> {
    let (target, want) = match stmts.last()? {
        SourceStmt::Instr(instr) => match (&instr.command[..], &instr.operand) {
            ("pop_int_local", SourceOperand::Local(target)) => (target, ValType::Int),
            ("pop_string_local", SourceOperand::Local(target)) => (target, ValType::String),
            ("pop_long_local", SourceOperand::Local(target)) => (target, ValType::Long),
            _ => return None,
        },
        SourceStmt::Label(_) | SourceStmt::Assign { .. } => return None,
    };
    enum Token {
        Leaf(ValType, Expr),
        Op(OpShape),
    }
    // Backward: collect tokens until the popped value is fully explained.
    // A folded `Call` is a leaf producing its single return (its arguments
    // were already consumed by the fold, so it pops nothing from the window).
    let mut tokens: Vec<Token> = Vec::new();
    let mut depth = 1_i32;
    let inner = &stmts[..stmts.len() - 1];
    for (offset, stmt) in inner.iter().rev().enumerate() {
        let stmt_index = inner.len().checked_sub(offset + 1)?;
        let is_canonical = canonical.get(stmt_index).copied().unwrap_or(false);
        let SourceStmt::Instr(instr) = stmt else {
            return None;
        };
        if let Some((ty, expr)) = lift_leaf(instr) {
            tokens.push(Token::Leaf(ty, expr));
            depth -= 1;
        } else if let Some((ty, expr)) = lift_call_leaf(instr, is_canonical, symbols) {
            tokens.push(Token::Leaf(ty, expr));
            depth -= 1;
        } else if let Some(shape) = op_shape(&instr.command, &instr.operand) {
            tokens.push(Token::Op(shape));
            let produced: i32 = shape.pushes.iter().map(|count| i32::from(*count)).sum();
            let consumed: i32 = shape.pops.iter().map(|count| i32::from(*count)).sum();
            depth += consumed - produced;
        } else {
            return None;
        }
        if depth == 0 {
            break;
        }
        if depth < 0 {
            return None;
        }
    }
    if depth != 0 {
        return None;
    }
    // Forward: rebuild the tree exactly as the stack machine would evaluate
    // it, type-checking every operand against its operator's shape. The
    // config-typed family resolves its push lane through `configs` (unknown
    // or non-literal ids bail to flat); every other operator reads its shape
    // off the shared contract via `op_shape`.
    let mut stack: Vec<(ValType, Expr)> = Vec::new();
    for token in tokens.iter().rev() {
        match token {
            Token::Leaf(ty, expr) => stack.push((*ty, expr.clone())),
            Token::Op(shape) => {
                if let LiftOp::Param(op) = shape.op {
                    let (expect, count) = single_lane(shape.pops)?;
                    if expect != ValType::Int || count != 2 {
                        return None;
                    }
                    let (second_ty, second_expr) = stack.pop()?;
                    let (first_ty, first_expr) = stack.pop()?;
                    if first_ty != ValType::Int || second_ty != ValType::Int {
                        return None;
                    }
                    let Expr::LitInt(param) = second_expr else {
                        return None;
                    };
                    let is_string = configs.param_is_string(param).ok()?;
                    let result = if is_string {
                        ValType::String
                    } else {
                        ValType::Int
                    };
                    stack.push((
                        result,
                        Expr::Param {
                            op,
                            obj: Box::new(first_expr),
                            param,
                        },
                    ));
                    continue;
                }
                if shape.op == LiftOp::Enum {
                    let (expect, count) = single_lane(shape.pops)?;
                    if expect != ValType::Int || count != 4 {
                        return None;
                    }
                    let (key_ty, key_expr) = stack.pop()?;
                    let (enum_ty, enum_expr) = stack.pop()?;
                    let (output_ty, output_expr) = stack.pop()?;
                    let (input_ty, input_expr) = stack.pop()?;
                    if input_ty != ValType::Int
                        || output_ty != ValType::Int
                        || enum_ty != ValType::Int
                        || key_ty != ValType::Int
                    {
                        return None;
                    }
                    let Expr::LitInt(enumeration) = enum_expr else {
                        return None;
                    };
                    let is_string = configs.enum_output_is_string(enumeration).ok()?;
                    let result = if is_string {
                        ValType::String
                    } else {
                        ValType::Int
                    };
                    stack.push((
                        result,
                        Expr::Enum {
                            input: Box::new(input_expr),
                            output: Box::new(output_expr),
                            enumeration,
                            key: Box::new(key_expr),
                        },
                    ));
                    continue;
                }
                let (expect, count) = single_lane(shape.pops)?;
                let (result, pushes) = single_lane(shape.pushes)?;
                if pushes != 1 {
                    return None;
                }
                let mut operands = Vec::with_capacity(count);
                for _ in 0..count {
                    let (got, expr) = stack.pop()?;
                    if got != expect {
                        return None;
                    }
                    operands.push(expr);
                }
                operands.reverse();
                let mut inputs = operands.into_iter();
                let (Some(first), second, none) = (inputs.next(), inputs.next(), inputs.next())
                else {
                    return None;
                };
                let tree = match shape.op {
                    LiftOp::Unary(op) => {
                        if second.is_some() || none.is_some() {
                            return None;
                        }
                        Expr::Unary(op, Box::new(first))
                    }
                    LiftOp::Binary(op) => {
                        let (Some(second), None) = (second, none) else {
                            return None;
                        };
                        Expr::Binary(op, Box::new(first), Box::new(second))
                    }
                    LiftOp::Nary(op) => {
                        // The triple split above already consumed up to three
                        // operands; gather whatever remains and pin the exact
                        // arity — a short or overlong stack run stays flat.
                        let mut all = vec![first];
                        if let Some(second) = second {
                            all.push(second);
                        }
                        if let Some(third) = none {
                            all.push(third);
                        }
                        all.extend(inputs);
                        if all.len() != op.arity() {
                            return None;
                        }
                        Expr::Nary(op, all)
                    }
                    LiftOp::Join => {
                        // The operand-counted row gathers exactly like the
                        // fixed-arity arm above; the rebuilt length is pinned
                        // against the popped count (which `op_shape` already
                        // restricted to the expression-accepted 2+) — a short
                        // or overlong stack run stays flat, like everything
                        // else. The fixed-arity arms above are untouched.
                        let mut all = vec![first];
                        if let Some(second) = second {
                            all.push(second);
                        }
                        if let Some(third) = none {
                            all.push(third);
                        }
                        all.extend(inputs);
                        if all.len() != count || all.len() < MIN_JOIN_ARITY {
                            return None;
                        }
                        Expr::Join(all)
                    }
                    // Config-typed reads never reach here: the early arms
                    // above resolve them through `configs` and `continue`.
                    // This stays flat if that ever stops holding.
                    LiftOp::Param(_) | LiftOp::Enum => return None,
                };
                stack.push((result, tree));
            }
        }
    }
    if stack.len() != 1 {
        return None;
    }
    let (ty, expr) = stack.pop()?;
    if ty != want {
        return None;
    }
    Some((
        SourceStmt::Assign {
            target: target.clone(),
            expr,
        },
        tokens.len() + 1,
    ))
}

/// A canonical push in lifted form, if it is one [`lower_expr`](crate::expr::lower_expr)
/// re-emits identically: const-string-family constants and typed `$local`
/// pushes. Every other push spelling (`push_constant_int`, var/varbit reads,
/// command results) has no expression syntax or different bytes, and ends
/// the window instead.
fn lift_leaf(instr: &SourceInstr) -> Option<(ValType, Expr)> {
    match (&instr.command[..], &instr.operand) {
        ("push_constant_string", SourceOperand::Int(value)) => {
            Some((ValType::Int, Expr::LitInt(*value)))
        }
        // A symbolized packed id folds as its symbolic leaf (same int lane,
        // same bytes — see `lower_expr`'s `Component` arm).
        ("push_constant_string", SourceOperand::Component { iface, child }) => Some((
            ValType::Int,
            Expr::Component {
                iface: *iface,
                child: *child,
            },
        )),
        ("push_constant_string", SourceOperand::Long(value)) => {
            Some((ValType::Long, Expr::LitLong(*value)))
        }
        ("push_constant_string", SourceOperand::Str(text)) => {
            Some((ValType::String, Expr::LitStr(text.clone())))
        }
        ("push_int_local", SourceOperand::Local(name)) => {
            Some((ValType::Int, Expr::Local(name.clone())))
        }
        ("push_string_local", SourceOperand::Local(name)) => {
            Some((ValType::String, Expr::Local(name.clone())))
        }
        ("push_long_local", SourceOperand::Local(name)) => {
            Some((ValType::Long, Expr::Local(name.clone())))
        }
        _ => None,
    }
}

/// Which tree node a lifted operator builds. Fixed-lane shapes are NOT
/// stored here — they are read off the shared effect contract at every use
/// (see [`op_shape`]), so drift between this module and the verified table
/// folds nothing instead of misshaping trees. The config-typed family is the
/// principled exception: pops are fixed ints verified against the same
/// handler bodies, but pushes vary by config, so `op_shape` records the
/// placeholder int push for depth tracking and the forward builder resolves
/// the real lane through [`ConfigTypes`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiftOp {
    Unary(UnaryOp),
    Binary(BinaryOp),
    Nary(NaryOp),
    /// Operand-counted string join (`join_string`): pops its explicit count
    /// off the object stack, pushes one object. The arity rides in the
    /// recorded pops triple — no extra field to drift — and the forward
    /// builder pins the rebuilt length against it (see `try_fold_assign`).
    Join,
    /// Config-backed param read (`struct_param` and siblings): 2 int pops,
    /// one config-resolved push. The holder rides in the variant — no shape
    /// to drift — and the forward builder extracts the literal id from the
    /// popped operand and resolves the lane (see `try_fold_assign`).
    Param(ParamOp),
    /// Config-backed enum read (`_enum`): 4 int pops, one config-resolved
    /// push. Same literal-id-plus-resolver discipline as [`LiftOp::Param`].
    Enum,
}

/// A verified operator occurrence inside a lifted window: which tree node to
/// build plus the (pops, pushes) triple it was verified against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OpShape {
    op: LiftOp,
    pops: [u16; 3],
    pushes: [u16; 3],
}

/// A lifted operator, if the statement is one `lower_expr` re-emits
/// identically: a known fixed-lane operator word with the canonical zero
/// generic operand whose shared-contract shape is a homogeneous single-push
/// row (every fixed-lane operator today), `join_string` with its explicit
/// `Count` operand whose contract row is the matching homogeneous n-ary row,
/// or a config-typed read (`struct_param` and siblings, `_enum`) with the
/// canonical zero generic operand and its handler-verified fixed int pops
/// (the push lane resolves later through [`ConfigTypes`] — see
/// `try_fold_assign`). Anything else — unknown words, nonzero generics,
/// degenerate join counts, contract drift — ends the window instead of
/// guessing.
fn op_shape(command: &str, operand: &SourceOperand) -> Option<OpShape> {
    // `join_string` carries its arity in the operand (`join_string(3)` pops
    // three objects): the count IS the static shape, read off the shared
    // contract against the count itself. Only the expression-accepted range
    // folds (2+, fitting the triple) — degenerate counts, negative or
    // overflowing counts, and contract drift all stay flat, never guessed.
    // This is the one principled extension to the homogeneous-row rule below:
    // the n-ary homogeneous-object row, with the count pinned at every use.
    if command == "join_string" {
        let SourceOperand::Count(count) = operand else {
            return None;
        };
        if *count < 2 {
            return None;
        }
        let narrow = u16::try_from(*count).ok()?;
        let Effect::Fixed { pops, pushes } = effect(command, &Operand::Count(*count)) else {
            return None;
        };
        if pops != [0, narrow, 0] || pushes != [0, 1, 0] {
            return None;
        }
        return Some(OpShape {
            op: LiftOp::Join,
            pops,
            pushes,
        });
    }
    if *operand != SourceOperand::Raw(0) {
        return None;
    }
    // Config-typed reads bypass the shared contract (which is `Unknown` for
    // them by design — no config access there): pops are the handler-verified
    // fixed int counts, pushes the placeholder int single for depth tracking
    // (the forward builder resolves the real lane and bails on unknown ids).
    // All five param words and `_enum` carry `Byte(0)` at every corpus use.
    if let Some(op) = ParamOp::from_command(command) {
        return Some(OpShape {
            op: LiftOp::Param(op),
            pops: [2, 0, 0],
            pushes: [1, 0, 0],
        });
    }
    if command == "_enum" {
        return Some(OpShape {
            op: LiftOp::Enum,
            pops: [4, 0, 0],
            pushes: [1, 0, 0],
        });
    }
    let op = match command {
        "not" => LiftOp::Unary(UnaryOp::Not),
        "random" => LiftOp::Unary(UnaryOp::Random),
        "randominc" => LiftOp::Unary(UnaryOp::RandomInc),
        "string_length" => LiftOp::Unary(UnaryOp::StringLength),
        "tostring" => LiftOp::Unary(UnaryOp::ToString),
        "add" => LiftOp::Binary(BinaryOp::Add),
        "multiply" => LiftOp::Binary(BinaryOp::Multiply),
        "divide" => LiftOp::Binary(BinaryOp::Divide),
        "modulo" => LiftOp::Binary(BinaryOp::Modulo),
        "min" => LiftOp::Binary(BinaryOp::Min),
        "max" => LiftOp::Binary(BinaryOp::Max),
        "and" => LiftOp::Binary(BinaryOp::And),
        "or" => LiftOp::Binary(BinaryOp::Or),
        "addpercent" => LiftOp::Binary(BinaryOp::AddPercent),
        "setbit" => LiftOp::Binary(BinaryOp::SetBit),
        "clearbit" => LiftOp::Binary(BinaryOp::ClearBit),
        "testbit" => LiftOp::Binary(BinaryOp::TestBit),
        "pow" => LiftOp::Binary(BinaryOp::Pow),
        "quickchat_dynamic_command_add" => LiftOp::Binary(BinaryOp::QuickchatDynamicCommandAdd),
        "append" => LiftOp::Binary(BinaryOp::Append),
        "compare" => LiftOp::Binary(BinaryOp::Compare),
        "scale" => LiftOp::Nary(NaryOp::Scale),
        "interpolate" => LiftOp::Nary(NaryOp::Interpolate),
        _ => return None,
    };
    let Effect::Fixed { pops, pushes } = effect(command, &Operand::Byte(0)) else {
        return None;
    };
    // Homogeneous pops, exactly one push: the only shapes the tree builder
    // below understands. A future heterogeneous operator stays flat until the
    // builder learns it — visibly, not silently.
    single_lane(pops)?;
    let (_, pushed) = single_lane(pushes)?;
    if pushed != 1 {
        return None;
    }
    Some(OpShape { op, pops, pushes })
}

/// The single typed lane of a homogeneous count triple: which type and how
/// many. `None` unless exactly one lane is nonzero. Shared with
/// [`crate::expr`] so human-side n-ary typing and dump-side tree building
/// agree on what "homogeneous" means.
pub(crate) fn single_lane(triple: [u16; 3]) -> Option<(ValType, usize)> {
    let mut lanes = [
        (ValType::Int, triple[0]),
        (ValType::String, triple[1]),
        (ValType::Long, triple[2]),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0);
    let (ty, count) = lanes.next()?;
    if lanes.next().is_some() {
        return None;
    }
    Some((ty, usize::from(count)))
}

/// Per-type slot-name tables, bundled so lifting helpers stay under the
/// argument-count lint while sharing one view of the names.
struct SlotNames<'a> {
    ints: &'a [String],
    objs: &'a [String],
    longs: &'a [String],
}

fn arg_name(counter: &mut usize) -> String {
    let name = format!("param{}", *counter);
    *counter += 1;
    name
}

fn local_name(counter: &mut usize) -> String {
    let name = format!("temp{}", *counter);
    *counter += 1;
    name
}

fn label_for(index: i32) -> String {
    format!("L{index}")
}

fn lift_operand(command: &str, operand: &Operand, names: &SlotNames<'_>) -> Result<SourceOperand> {
    let mismatch = |expected: &str| {
        NativeError::Invalid(format!(
            "cannot lift {command}: expected {expected} operand"
        ))
    };
    match command {
        "push_constant_int" => match operand {
            Operand::Int(value) => Ok(SourceOperand::Int(*value)),
            _ => Err(mismatch("Int")),
        },
        "push_constant_string" => match operand {
            Operand::Int(value) => Ok(SourceOperand::Int(*value)),
            Operand::Long(value) => Ok(SourceOperand::Long(*value)),
            Operand::Str(text) => Ok(SourceOperand::Str(text.clone())),
            _ => Err(mismatch("Int/Long/Str")),
        },
        "push_int_local" | "pop_int_local" => match operand {
            Operand::Local(slot) => Ok(SourceOperand::Local(
                lifted_slot_name(names.ints, *slot, command)?.to_string(),
            )),
            _ => Err(mismatch("Local")),
        },
        "push_string_local" | "pop_string_local" => match operand {
            Operand::Local(slot) => Ok(SourceOperand::Local(
                lifted_slot_name(names.objs, *slot, command)?.to_string(),
            )),
            _ => Err(mismatch("Local")),
        },
        "push_long_local" | "pop_long_local" => match operand {
            Operand::Local(slot) => Ok(SourceOperand::Local(
                lifted_slot_name(names.longs, *slot, command)?.to_string(),
            )),
            _ => Err(mismatch("Local")),
        },
        "push_var" | "pop_var" => match operand {
            Operand::VarRef(var) => Ok(SourceOperand::Var(var.domain, var.id, var.transmog)),
            _ => Err(mismatch("VarRef")),
        },
        "push_varbit" | "pop_varbit" => match operand {
            Operand::VarBitRef(varbit) => Ok(SourceOperand::VarBit(varbit.id, varbit.transmog)),
            _ => Err(mismatch("VarBitRef")),
        },
        command if is_branch_command(command) => match operand {
            Operand::Branch(target) => Ok(SourceOperand::Label(label_for(*target))),
            _ => Err(mismatch("Branch")),
        },
        "switch" => match operand {
            Operand::Switch(cases) => Ok(SourceOperand::Cases(
                cases
                    .iter()
                    .map(|case| (case.value, label_for(case.target)))
                    .collect(),
            )),
            _ => Err(mismatch("Switch")),
        },
        "join_string" => match operand {
            Operand::Count(value) | Operand::Int(value) => Ok(SourceOperand::Count(*value)),
            _ => Err(mismatch("Count")),
        },
        "gosub_with_params" => match operand {
            Operand::Script(id) | Operand::Int(id) => Ok(SourceOperand::Script(*id)),
            _ => Err(mismatch("Script")),
        },
        "define_array"
        | "push_array_int"
        | "pop_array_int"
        | "push_array_int_leave_index_on_stack"
        | "push_array_int_and_index"
        | "pop_array_int_leave_value_on_stack" => match operand {
            Operand::Array(id) | Operand::Int(id) => Ok(SourceOperand::Array(*id)),
            _ => Err(mismatch("Array")),
        },
        _ => match operand {
            Operand::Byte(value) => Ok(SourceOperand::Raw(i32::from(*value))),
            Operand::Int(value) => Ok(SourceOperand::Raw(*value)),
            _ => Err(mismatch("Byte/Int")),
        },
    }
}

/// Slot index → generated name within one type's table (args occupy the first
/// slots, so the index addresses args-then-locals directly).
fn lifted_slot_name<'a>(names: &'a [String], slot: i32, command: &str) -> Result<&'a str> {
    let index = usize::try_from(slot)
        .map_err(|_| NativeError::Invalid(format!("negative local slot for {command}")))?;
    names
        .get(index)
        .ok_or_else(|| {
            NativeError::Invalid(format!("local slot {slot} out of range for {command}"))
        })
        .map(String::as_str)
}

/// Lower editable form back to a compiled script: re-derive slots (each type's
/// args occupy its first slots, in header order), resolve labels to absolute
/// targets, and fill canonical opcodes from the book.
///
/// `~name()` calls desugar first — one call statement becomes its argument
/// pushes plus the numeric `gosub` — so label indexing below never sees them.
/// `configs` resolves config-typed reads during assignment expansion (same
/// empty-means-loud rule as [`lift`]).
pub fn lower(
    source: &SourceScript,
    book: &OpcodeBook,
    symbols: &SymbolRegistry,
    configs: &ConfigTypes,
) -> Result<CompiledScript> {
    Ok(lower_mapped(source, book, symbols, configs)?.0)
}

/// Lower with a body-statement index for every emitted instruction.
pub fn lower_mapped(
    source: &SourceScript,
    book: &OpcodeBook,
    symbols: &SymbolRegistry,
    configs: &ConfigTypes,
) -> Result<(CompiledScript, Vec<usize>)> {
    let mut slots: BTreeMap<&str, (ValType, i32)> = BTreeMap::new();
    let mut counts = Counts::default();
    for param in &source.args {
        if slots.contains_key(param.name.as_str()) {
            return Err(NativeError::Invalid(format!(
                "duplicate parameter '${}'",
                param.name
            )));
        }
        let slot = take_slot(&mut counts, param.ty)?;
        slots.insert(param.name.as_str(), (param.ty, slot));
    }
    for param in &source.locals {
        if slots.contains_key(param.name.as_str()) {
            return Err(NativeError::Invalid(format!(
                "duplicate local '${}'",
                param.name
            )));
        }
        let slot = take_slot(&mut counts, param.ty)?;
        slots.insert(param.name.as_str(), (param.ty, slot));
    }

    let mut expanded = Vec::new();
    let mut statement_map = Vec::new();
    let mut single = source.clone();
    for (index, stmt) in source.body.iter().enumerate() {
        single.body = vec![stmt.clone()];
        let calls = desugar_calls(&single, symbols, &slots)?;
        let statements = desugar_assigns(&calls, &slots, book, symbols, configs)?;
        for expanded_stmt in statements {
            if matches!(expanded_stmt, SourceStmt::Instr(_)) {
                statement_map.push(index);
            }
            expanded.push(expanded_stmt);
        }
    }

    // Argument counts are the args' per-type totals (they were assigned first).
    let mut arg_counts = Counts::default();
    for param in &source.args {
        match param.ty {
            ValType::Int => arg_counts.int += 1,
            ValType::String => arg_counts.obj += 1,
            ValType::Long => arg_counts.long += 1,
        }
    }
    // Pass 1: label → instruction index (labels occupy no slots).
    let mut labels: BTreeMap<&str, i32> = BTreeMap::new();
    let mut index = 0_i32;
    for stmt in &expanded {
        match stmt {
            SourceStmt::Label(name) => {
                if labels.contains_key(name.as_str()) {
                    return Err(NativeError::Invalid(format!("duplicate label '{name}'")));
                }
                labels.insert(name.as_str(), index);
            }
            SourceStmt::Instr(_) => {
                index = index
                    .checked_add(1)
                    .ok_or_else(|| NativeError::Invalid("script too large".to_string()))?;
            }
            // Unreachable: `desugar_assigns` above expands every Assign, so
            // label indexing never sees one. Loud if that ever stops holding.
            SourceStmt::Assign { target, .. } => {
                return Err(NativeError::Invalid(format!(
                    "unexpanded assignment to '${target}' reached lowering"
                )));
            }
        }
    }

    // Pass 2: convert.
    let mut code = Vec::new();
    for stmt in &expanded {
        let SourceStmt::Instr(instr) = stmt else {
            // Labels occupy no slots; Assigns are gone (see pass 1).
            continue;
        };
        let opcode = book.opcode_for(&instr.command)?;
        let operand = lower_operand(
            &instr.command,
            &instr.operand,
            book.has_large_operand(opcode),
            &slots,
            &labels,
        )?;
        code.push(Instruction {
            opcode,
            command: instr.command.clone(),
            operand,
        });
    }

    Ok((
        CompiledScript {
            name: source.name.clone(),
            locals: counts,
            args: arg_counts,
            code,
        },
        statement_map,
    ))
}

/// Expand `~name()` calls into argument pushes plus a numeric `gosub`, in the
/// listed order. Literal arguments lower to the corpus-measured
/// `push_constant_string` family spellings — int-tagged ints are the uniform
/// arg convention (bare `push_constant_int` never appears as a call argument
/// in the corpus), longs follow the family by consistency, `$locals` lower
/// through their declared types. This is semantically exact, not just
/// byte-stable: the client itself rewrites int/long-tagged constants to the
/// plain push commands during decode. The argument TYPE MULTISET must match
/// the callee signature; unknown names, arity mismatches, and undeclared
/// locals fail loudly.
fn desugar_calls(
    source: &SourceScript,
    symbols: &SymbolRegistry,
    slots: &BTreeMap<&str, (ValType, i32)>,
) -> Result<Vec<SourceStmt>> {
    let mut out = Vec::with_capacity(source.body.len());
    for stmt in &source.body {
        let SourceStmt::Instr(instr) = stmt else {
            out.push(stmt.clone());
            continue;
        };
        let SourceOperand::Call(name, args) = &instr.operand else {
            out.push(stmt.clone());
            continue;
        };
        let id = symbols
            .resolve_call(name)
            .ok_or_else(|| NativeError::Invalid(format!("unknown script '~{name}'")))?;
        let sig = symbols
            .args_of(id)
            .ok_or_else(|| NativeError::Invalid(format!("unknown script '~{name}'")))?;
        let mut ints = 0_usize;
        let mut objs = 0_usize;
        let mut longs = 0_usize;
        for arg in args {
            match arg {
                CallArg::Int(_) | CallArg::VarBit(..) => ints += 1,
                CallArg::Long(_) => longs += 1,
                CallArg::Str(_) => objs += 1,
                CallArg::Local(local) => match slots.get(local.as_str()).map(|slot| slot.0) {
                    Some(ValType::Int) => ints += 1,
                    Some(ValType::String) => objs += 1,
                    Some(ValType::Long) => longs += 1,
                    None => {
                        return Err(NativeError::Invalid(format!("undeclared local '${local}'")));
                    }
                },
            }
        }
        if ints != usize::from(sig.int)
            || objs != usize::from(sig.obj)
            || longs != usize::from(sig.long)
        {
            return Err(NativeError::Invalid(format!(
                "'~{name}' expects ({}i,{}o,{}l), got ({ints}i,{objs}o,{longs}l)",
                sig.int, sig.obj, sig.long
            )));
        }
        for arg in args {
            let (command, operand) = match arg {
                CallArg::Int(value) => ("push_constant_string", SourceOperand::Int(*value)),
                CallArg::Long(value) => ("push_constant_string", SourceOperand::Long(*value)),
                CallArg::Str(text) => ("push_constant_string", SourceOperand::Str(text.clone())),
                CallArg::VarBit(id, transmog) => {
                    ("push_varbit", SourceOperand::VarBit(*id, *transmog))
                }
                CallArg::Local(local) => {
                    let (ty, _) = slots.get(local.as_str()).copied().ok_or_else(|| {
                        NativeError::Invalid(format!("undeclared local '${local}'"))
                    })?;
                    let command = match ty {
                        ValType::Int => "push_int_local",
                        ValType::String => "push_string_local",
                        ValType::Long => "push_long_local",
                    };
                    (command, SourceOperand::Local(local.clone()))
                }
            };
            out.push(SourceStmt::Instr(SourceInstr {
                command: command.to_string(),
                operand,
            }));
        }
        out.push(SourceStmt::Instr(SourceInstr {
            command: "gosub_with_params".to_string(),
            operand: SourceOperand::Script(id),
        }));
    }
    Ok(out)
}

/// Expand `$target = <expr>;` assignments into their push/operate/pop
/// sequences, in order, mirroring [`desugar_calls`]: this runs BEFORE label
/// indexing in [`lower`], so labels never see an assignment and a jump into
/// the middle of an expression stays impossible by construction.
///
/// The target must be declared (the ONLY new declaration check — the slot
/// tables built above are reused as-is) and the expression's type must match
/// it. [`crate::expr::lower_expr`] re-validates every operator type and arity
/// with message-only errors for programmatic models; its binary-model output
/// maps back to symbolic operands below (the mapping is total over what
/// `lower_expr` emits, loud otherwise), then the final `pop_<type>_local`
/// for the target is appended.
fn desugar_assigns(
    stmts: &[SourceStmt],
    slots: &BTreeMap<&str, (ValType, i32)>,
    book: &OpcodeBook,
    symbols: &SymbolRegistry,
    configs: &ConfigTypes,
) -> Result<Vec<SourceStmt>> {
    let mut out = Vec::with_capacity(stmts.len());
    for stmt in stmts {
        let SourceStmt::Assign { target, expr } = stmt else {
            out.push(stmt.clone());
            continue;
        };
        let (ty, _) = slots
            .get(target.as_str())
            .copied()
            .ok_or_else(|| NativeError::Invalid(format!("undeclared local '${target}'")))?;
        let mut flat = Vec::new();
        let produced = crate::expr::lower_expr(expr, slots, book, symbols, configs, &mut flat)?;
        if produced != ty {
            return Err(NativeError::Invalid(format!(
                "cannot assign {} expression to {} '${target}'",
                produced.keyword(),
                ty.keyword()
            )));
        }
        for instr in &flat {
            out.push(SourceStmt::Instr(SourceInstr {
                command: instr.command.clone(),
                operand: assign_operand(instr, slots)?,
            }));
        }
        let pop = match ty {
            ValType::Int => "pop_int_local",
            ValType::String => "pop_string_local",
            ValType::Long => "pop_long_local",
        };
        out.push(SourceStmt::Instr(SourceInstr {
            command: pop.to_string(),
            operand: SourceOperand::Local(target.clone()),
        }));
    }
    Ok(out)
}

/// Map one `lower_expr` binary-model instruction back to its symbolic
/// operand: the const-string family spellings round-trip by variant, typed
/// local pushes resolve their slot back to its `$name`, verified operators
/// and config-typed reads carry the canonical `0` generic operand, and
/// `join_string` carries its explicit count back. Anything outside that
/// closed output shape is a loud error, never a guessed operand.
fn assign_operand(
    instr: &Instruction,
    slots: &BTreeMap<&str, (ValType, i32)>,
) -> Result<SourceOperand> {
    match instr.command.as_str() {
        "push_constant_string" => match &instr.operand {
            Operand::Int(value) => Ok(SourceOperand::Int(*value)),
            Operand::Long(value) => Ok(SourceOperand::Long(*value)),
            Operand::Str(text) => Ok(SourceOperand::Str(text.clone())),
            _ => Err(NativeError::Invalid(format!(
                "cannot assign through `{}` with a non-constant operand",
                instr.command
            ))),
        },
        "push_int_local" => Ok(SourceOperand::Local(assign_slot_name(
            slots,
            ValType::Int,
            &instr.operand,
        )?)),
        "push_string_local" => Ok(SourceOperand::Local(assign_slot_name(
            slots,
            ValType::String,
            &instr.operand,
        )?)),
        "push_long_local" => Ok(SourceOperand::Local(assign_slot_name(
            slots,
            ValType::Long,
            &instr.operand,
        )?)),
        // Calls-as-values lower to a resolved numeric gosub (the only
        // `gosub_with_params` shape `lower_expr` emits); it maps straight
        // back so the desugared statements stay symbolic.
        "gosub_with_params" => match &instr.operand {
            Operand::Script(id) => Ok(SourceOperand::Script(*id)),
            _ => Err(NativeError::Invalid(format!(
                "cannot assign through `{}` with a non-script operand",
                instr.command
            ))),
        },
        // `join_string` lowers to the command with its explicit count (the
        // only operand-carried shape `lower_expr` emits); it maps straight
        // back so the desugared statements stay symbolic.
        "join_string" => match &instr.operand {
            Operand::Count(value) | Operand::Int(value) => Ok(SourceOperand::Count(*value)),
            _ => Err(NativeError::Invalid(format!(
                "cannot assign through `{}` with a non-count operand",
                instr.command
            ))),
        },
        _ => match &instr.operand {
            Operand::Byte(0) => Ok(SourceOperand::Raw(0)),
            _ => Err(NativeError::Invalid(format!(
                "cannot assign through `{}` with a non-zero generic operand",
                instr.command
            ))),
        },
    }
}

/// Binary-model local slot back to its `$`-less name (slots are unique per
/// type: args take each type's first slots, in header order).
fn assign_slot_name(
    slots: &BTreeMap<&str, (ValType, i32)>,
    ty: ValType,
    operand: &Operand,
) -> Result<String> {
    let Operand::Local(slot) = operand else {
        return Err(NativeError::Invalid(format!(
            "cannot assign through a non-local {} operand",
            ty.keyword()
        )));
    };
    slots
        .iter()
        .find(|(_, (found, at))| *found == ty && *at == *slot)
        .map(|(name, _)| (*name).to_string())
        .ok_or_else(|| NativeError::Invalid(format!("local slot {slot} has no declared name")))
}

fn take_slot(counts: &mut Counts, ty: ValType) -> Result<i32> {
    let slot = match ty {
        ValType::Int => {
            let slot = counts.int;
            counts.int = counts
                .int
                .checked_add(1)
                .ok_or_else(|| NativeError::Invalid("too many int slots".to_string()))?;
            slot
        }
        ValType::String => {
            let slot = counts.obj;
            counts.obj = counts
                .obj
                .checked_add(1)
                .ok_or_else(|| NativeError::Invalid("too many object slots".to_string()))?;
            slot
        }
        ValType::Long => {
            let slot = counts.long;
            counts.long = counts
                .long
                .checked_add(1)
                .ok_or_else(|| NativeError::Invalid("too many long slots".to_string()))?;
            slot
        }
    };
    Ok(i32::from(slot))
}

fn lower_operand(
    command: &str,
    operand: &SourceOperand,
    is_large_operand: bool,
    slots: &BTreeMap<&str, (ValType, i32)>,
    labels: &BTreeMap<&str, i32>,
) -> Result<Operand> {
    let mismatch =
        |expected: &str| NativeError::Invalid(format!("{command} expects {expected} here"));
    // Local-slot commands additionally check the name's declared type.
    let local_slot = |name: &str, ty: ValType| {
        let (found, slot) = slots
            .get(name)
            .copied()
            .ok_or_else(|| NativeError::Invalid(format!("undeclared local '${name}'")))?;
        if found != ty {
            return Err(NativeError::Invalid(format!(
                "{command} needs a {} slot, but '${name}' is {}",
                ty.keyword(),
                found.keyword()
            )));
        }
        Ok(slot)
    };
    match command {
        "push_constant_int" => match operand {
            SourceOperand::Int(value) => Ok(Operand::Int(*value)),
            // The identical packed int the numeric spelling yields.
            SourceOperand::Component { iface, child } => {
                Ok(Operand::Int(pack_component(*iface, *child, command)?))
            }
            _ => Err(mismatch("an int literal")),
        },
        "push_constant_string" => match operand {
            SourceOperand::Int(value) => Ok(Operand::Int(*value)),
            // The identical packed int the numeric spelling yields.
            SourceOperand::Component { iface, child } => {
                Ok(Operand::Int(pack_component(*iface, *child, command)?))
            }
            SourceOperand::Long(value) => Ok(Operand::Long(*value)),
            SourceOperand::Str(text) => Ok(Operand::Str(text.clone())),
            _ => Err(mismatch("an int, long, or string literal")),
        },
        "push_int_local" | "pop_int_local" => match operand {
            SourceOperand::Local(name) => Ok(Operand::Local(local_slot(name, ValType::Int)?)),
            _ => Err(mismatch("a $local")),
        },
        "push_string_local" | "pop_string_local" => match operand {
            SourceOperand::Local(name) => Ok(Operand::Local(local_slot(name, ValType::String)?)),
            _ => Err(mismatch("a $local")),
        },
        "push_long_local" | "pop_long_local" => match operand {
            SourceOperand::Local(name) => Ok(Operand::Local(local_slot(name, ValType::Long)?)),
            _ => Err(mismatch("a $local")),
        },
        "push_var" | "pop_var" => match operand {
            SourceOperand::Var(domain, id, transmog) => Ok(Operand::VarRef(VarRef {
                domain: *domain,
                id: *id,
                transmog: *transmog,
            })),
            _ => Err(mismatch("a domain:id var")),
        },
        "push_varbit" | "pop_varbit" => match operand {
            SourceOperand::VarBit(id, transmog) => Ok(Operand::VarBitRef(VarBitRef {
                id: *id,
                transmog: *transmog,
            })),
            _ => Err(mismatch("a varbit:id")),
        },
        command if is_branch_command(command) => match operand {
            SourceOperand::Label(name) => Ok(Operand::Branch(resolve_label(labels, name)?)),
            _ => Err(mismatch("a label")),
        },
        "switch" => match operand {
            SourceOperand::Cases(cases) => {
                let mut out = Vec::with_capacity(cases.len());
                for (value, name) in cases {
                    out.push(SwitchCase {
                        value: *value,
                        target: resolve_label(labels, name)?,
                    });
                }
                Ok(Operand::Switch(out))
            }
            _ => Err(mismatch("a case block")),
        },
        "join_string" => match operand {
            SourceOperand::Count(value) | SourceOperand::Int(value) | SourceOperand::Raw(value) => {
                Ok(Operand::Count(*value))
            }
            _ => Err(mismatch("a count")),
        },
        "gosub_with_params" => match operand {
            SourceOperand::Script(id) | SourceOperand::Int(id) | SourceOperand::Raw(id) => {
                Ok(Operand::Script(*id))
            }
            _ => Err(mismatch("a script id")),
        },
        "define_array"
        | "push_array_int"
        | "pop_array_int"
        | "push_array_int_leave_index_on_stack"
        | "push_array_int_and_index"
        | "pop_array_int_leave_value_on_stack" => match operand {
            SourceOperand::Array(id) | SourceOperand::Int(id) | SourceOperand::Raw(id) => {
                Ok(Operand::Array(*id))
            }
            _ => Err(mismatch("an array id")),
        },
        _ => match operand {
            // The generic slot decodes as Byte or Int depending on the
            // command's width flag; lower to the same variant so the lowered
            // model equals what decode produces (the verify gate requires it).
            SourceOperand::Raw(value) | SourceOperand::Int(value) => {
                if is_large_operand {
                    Ok(Operand::Int(*value))
                } else if (-128..=255).contains(value) {
                    Ok(Operand::Byte(*value as u8))
                } else {
                    Err(NativeError::Invalid(format!(
                        "operand {value} does not fit in the 1-byte slot of {command}"
                    )))
                }
            }
            _ => Err(mismatch("a numeric operand")),
        },
    }
}

fn resolve_label(labels: &BTreeMap<&str, i32>, name: &str) -> Result<i32> {
    labels
        .get(name)
        .copied()
        .ok_or_else(|| NativeError::Invalid(format!("undefined label '{name}'")))
}

/// Pack a resolved component pair for lowering: validated pairs always pack,
/// so only programmatic garbage reaches the loud error (never a wrap).
fn pack_component(iface: i32, child: u32, command: &str) -> Result<i32> {
    crate::expr::pack_component_ref(iface, child).ok_or_else(|| {
        NativeError::Invalid(format!(
            "{command} has a component pair {iface}/{child} that does not pack into an int"
        ))
    })
}

/// Render canonical source text. A `gosub` whose callee the registry names
/// renders as a `~call();` statement; unknown callees stay numeric.
/// Component operands render `Bank/7`-style whenever `inames` names the
/// interface, else the packed decimal (each spelling re-parses to itself).
///
/// Fallible: a `Call` operand on any other command is a malformed model, and
/// rendering it anyway would silently change the command on reparse.
pub fn format_source(
    source: &SourceScript,
    symbols: &SymbolRegistry,
    inames: &InterfaceRegistry,
) -> Result<String> {
    let mut out = String::new();
    out.push('[');
    out.push_str(source.name.as_deref().unwrap_or(""));
    out.push(']');
    out.push('(');
    let mut first = true;
    for param in &source.args {
        if !first {
            out.push_str(", ");
        }
        first = false;
        let _ = write!(out, "{} ${}", param.ty.keyword(), param.name);
    }
    out.push_str(")\n");
    for ty in [ValType::Int, ValType::String, ValType::Long] {
        let names: Vec<&str> = source
            .locals
            .iter()
            .filter(|param| param.ty == ty)
            .map(|param| param.name.as_str())
            .collect();
        if !names.is_empty() {
            let _ = writeln!(out, "{} ${};", ty.keyword(), names.join(", $"));
        }
    }
    out.push('\n');
    for stmt in &source.body {
        match stmt {
            SourceStmt::Label(name) => {
                out.push_str(name);
                out.push_str(":\n");
            }
            // Expression assignments render back in surface syntax
            // (`$name = <expr>;`); the formatter parenthesizes only when
            // needed, so the line always reparses to an equal model.
            SourceStmt::Assign { target, expr } => {
                let _ = writeln!(
                    out,
                    "    ${target} = {};",
                    crate::expr::format_expr(expr, inames)
                );
            }
            SourceStmt::Instr(instr) => {
                // Calls render as house-style `~name(args);` — but only on
                // the gosub command itself. A `Call` anywhere else is a
                // malformed model; rendering it anyway would silently change
                // the command on reparse, so fail instead.
                if let SourceOperand::Call(name, args) = &instr.operand {
                    if instr.command != "gosub_with_params" {
                        return Err(NativeError::Invalid(format!(
                            "'~{name}' call on non-gosub command `{}`",
                            instr.command
                        )));
                    }
                    out.push_str("    ");
                    out.push_str(&format_call(name, args));
                    out.push_str(";\n");
                    continue;
                }
                // Switch blocks render multi-line; every other instruction is
                // one `command(operand);` line.
                if instr.command == "switch"
                    && let SourceOperand::Cases(cases) = &instr.operand
                {
                    out.push_str("    switch {\n");
                    for (value, target) in cases {
                        let _ = writeln!(out, "        case {value} -> {target};");
                    }
                    out.push_str("    }\n");
                    continue;
                }
                // A bare `~name();` is only unambiguous for zero-argument
                // callees: a named call WITH arguments that failed to fold
                // (computed arguments) must stay numeric, because `~name();`
                // would reparse as a zero-argument call and fail arity.
                if instr.command == "gosub_with_params"
                    && let SourceOperand::Script(id) = &instr.operand
                    && let Some(name) = symbols.name_for_call(*id)
                    && symbols.args_of(*id) == Some(Counts::default())
                {
                    let _ = writeln!(out, "    ~{name}();");
                    continue;
                }
                out.push_str("    ");
                out.push_str(&instr.command);
                out.push('(');
                out.push_str(&format_operand(&instr.operand, inames));
                out.push_str(");\n");
            }
        }
    }
    Ok(out)
}

/// Render one call argument: literals canonically, `$locals` by name,
/// varbits symbolically.
fn format_call_arg(arg: &CallArg) -> String {
    match arg {
        CallArg::Int(value) => value.to_string(),
        CallArg::Long(value) => format!("{value}L"),
        CallArg::Str(text) => format!(
            "\"{}\"",
            text.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        ),
        CallArg::Local(name) => format!("${name}"),
        CallArg::VarBit(id, transmog) => {
            if *transmog {
                format!("varbit:{id}:transmog")
            } else {
                format!("varbit:{id}")
            }
        }
    }
}

/// Render a whole `~name(args)` call. Also used by `format_operand`'s
/// degenerate arm (a `Call` on any other command), where the result is only
/// ever re-read by the parser — which rejects it loudly.
fn format_call(name: &str, args: &[CallArg]) -> String {
    let mut out = String::from("~");
    out.push_str(name);
    out.push('(');
    let mut first = true;
    for arg in args {
        if !first {
            out.push_str(", ");
        }
        first = false;
        out.push_str(&format_call_arg(arg));
    }
    out.push(')');
    out
}

fn format_operand(operand: &SourceOperand, inames: &InterfaceRegistry) -> String {
    match operand {
        SourceOperand::Int(value) | SourceOperand::Raw(value) => value.to_string(),
        SourceOperand::Component { iface, child } => inames.display_component_ref(*iface, *child),
        SourceOperand::Long(value) => format!("{value}L"),
        SourceOperand::Str(text) => format!(
            "\"{}\"",
            text.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        ),
        SourceOperand::Local(name) => format!("${name}"),
        SourceOperand::Var(domain, id, transmog) => {
            let base = format!("{}:{id}", domain.as_label());
            if *transmog {
                format!("{base}:transmog")
            } else {
                base
            }
        }
        SourceOperand::VarBit(id, transmog) => {
            if *transmog {
                format!("varbit:{id}:transmog")
            } else {
                format!("varbit:{id}")
            }
        }
        SourceOperand::Label(name) => name.clone(),
        SourceOperand::Call(name, args) => format_call(name, args),
        SourceOperand::Cases(cases) => {
            let mut out = String::from("{\n");
            for (value, target) in cases {
                let _ = writeln!(out, "        case {value} -> {target};");
            }
            out.push_str("    }");
            out
        }
        SourceOperand::Script(id) | SourceOperand::Array(id) | SourceOperand::Count(id) => {
            id.to_string()
        }
    }
}

/// Decode binary, lift, and render canonical source. `configs` resolves the
/// config-typed family during folding (empty leaves those windows flat);
/// `inames` symbolizes roster-known packed ids (empty renders numeric).
pub fn dump_script(
    data: &[u8],
    book: &OpcodeBook,
    symbols: &SymbolRegistry,
    configs: &ConfigTypes,
    inames: &InterfaceRegistry,
) -> Result<String> {
    format_source(
        &lift(&decode_script(data, book)?, symbols, configs, inames)?,
        symbols,
        inames,
    )
}

/// What a pack dump produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DumpReport {
    /// Source files written.
    pub files: usize,
    /// Call sites rendered as `~names` (proof the registry engaged).
    pub named_calls: usize,
}

/// Build the symbol registry over the runtime scripts pack: every script id
/// known with its mined argument counts, plus `curated` names. Shared by both
/// dump verbs so script and interface sources resolve the same names.
///
/// Return counts ride along via return-arity inference over the decoded
/// corpus (unknown stays unknown — the registry records only `Known`
/// results, so a failed analysis degrades to numeric-only value calls
/// instead of corrupting anything).
pub fn registry_for_scripts_pack(
    pack_root: &Path,
    curated: &[(i32, String)],
) -> Result<SymbolRegistry> {
    registry_for_scripts_pack_roots(pack_root, curated, None)
}

/// Retain the complete name/argument registry while inferring returns only for
/// the requested call closure. Other return contracts remain explicitly unknown.
pub(crate) fn registry_for_scripts_pack_roots(
    pack_root: &Path,
    curated: &[(i32, String)],
    roots: Option<&std::collections::BTreeSet<i32>>,
) -> Result<SymbolRegistry> {
    let scripts = crate::xref::load_scripts(pack_root)?;
    let dependencies =
        roots.map(|roots| crate::dataflow::dependency_closure(&scripts, roots.iter().copied()));
    let analyzed = dependencies.as_ref().unwrap_or(&scripts);
    let inferred = if analyzed.is_empty() {
        BTreeMap::new()
    } else {
        crate::dataflow::infer_returns(analyzed, &ConfigTypes::load(pack_root)?).0
    };
    registry_for_decoded_scripts(&scripts, curated, &inferred)
}

pub fn registry_for_decoded_scripts(
    scripts: &BTreeMap<i32, CompiledScript>,
    curated: &[(i32, String)],
    inferred: &BTreeMap<i32, crate::returns::ReturnArity>,
) -> Result<SymbolRegistry> {
    use crate::returns::ReturnArity;
    let known = scripts
        .iter()
        .map(|(id, script)| (*id, script.args))
        .collect();
    let mut returns = BTreeMap::new();
    for (id, arity) in inferred {
        if let ReturnArity::Known { int, obj, long } = arity {
            returns.insert(
                *id,
                Counts {
                    int: *int,
                    obj: *obj,
                    long: *long,
                },
            );
        }
    }
    Ok(SymbolRegistry::build(curated.to_vec(), &known)?.with_returns(&returns))
}

/// Dump every script in the runtime scripts pack to `<group>_<file>.rs2` files
/// under `out_dir`, creating it, plus the generated `symbols.txt` the
/// `assemble --symbols` verb reads back. `curated` names ride along when given.
/// `inames` renders roster-known packed ids symbolically (mirroring
/// `dump-interfaces --inames`); the dumped text re-assembles through the same
/// registry before touching disk, so symbolic dumps prove byte-identical.
/// Config tables load from the same `pack_root` so assignment folding resolves
/// the config-typed family; a script that fails to lift is a hard error naming
/// its group/file — reported, never skipped.
pub fn dump_scripts_pack(
    pack_root: &Path,
    out_dir: &Path,
    curated: &[(i32, String)],
    inames: &InterfaceRegistry,
) -> Result<DumpReport> {
    dump_selected_scripts_pack(pack_root, out_dir, curated, inames, None)
}

/// Dump one script, or the full corpus when no script is selected.
pub fn dump_selected_scripts_pack(
    pack_root: &Path,
    out_dir: &Path,
    curated: &[(i32, String)],
    inames: &InterfaceRegistry,
    selected: Option<i32>,
) -> Result<DumpReport> {
    use crate::pack::PackArchive;
    let archive = PackArchive::open(&pack_root.join("client.scripts.js5"))?;
    if let Some(id) = selected
        && !u32::try_from(id).is_ok_and(|id| archive.has_group(id))
    {
        return Err(NativeError::Invalid(format!("unknown script {id}")));
    }
    let book = OpcodeBook::embedded()?;
    std::fs::create_dir_all(out_dir).map_err(NativeError::Io)?;
    let roots = selected.map(|id| std::collections::BTreeSet::from([id]));
    let symbols = registry_for_scripts_pack_roots(pack_root, curated, roots.as_ref())?;
    let configs = ConfigTypes::load(pack_root)?;

    // Decode everything up front so lift errors name their group/file.
    let mut decoded = Vec::new();
    let metadata_path = out_dir.join(crate::execution::PROJECT_FILENAME);
    let mut execution = if metadata_path.exists() {
        crate::execution::Table::parse(&std::fs::read_to_string(&metadata_path)?)?
    } else {
        crate::execution::Table::default()
    };
    for group in archive.group_ids() {
        if selected.is_some_and(|id| i32::try_from(group) != Ok(id)) {
            continue;
        }
        let Some(files) = archive.group_files(group)? else {
            continue;
        };
        let id = i32::try_from(group)
            .map_err(|_| NativeError::Invalid("script ID out of range".into()))?;
        let (script, metadata) = crate::execution::decode_group(id, &files, &book)
            .map_err(|error| NativeError::Invalid(format!("script {group}: {error}")))?;
        execution.remove(id);
        execution.extend(metadata)?;
        let bytes = files[&crate::execution::SCRIPT_FILE].clone();
        decoded.push((group, crate::execution::SCRIPT_FILE, bytes, script));
    }

    // Pass 2: render with names resolved.
    let mut report = DumpReport {
        files: 0,
        named_calls: 0,
    };
    for (group, file, original, script) in &decoded {
        let source = lift(script, &symbols, &configs, inames)
            .map_err(|error| NativeError::Invalid(format!("{group}/{file}: {error}")))?;
        for stmt in &source.body {
            // Count what the formatter renders as `~names`: folded calls plus
            // bare numeric gosubs to named zero-argument callees.
            if let SourceStmt::Instr(instr) = stmt {
                let named = match &instr.operand {
                    SourceOperand::Call(..) => true,
                    SourceOperand::Script(id) => {
                        symbols.name_for_call(*id).is_some()
                            && symbols.args_of(*id) == Some(Counts::default())
                    }
                    _ => false,
                };
                if named {
                    report.named_calls += 1;
                }
            }
        }
        let text = format_source(&source, &symbols, inames)
            .map_err(|error| NativeError::Invalid(format!("{group}/{file}: {error}")))?;
        // The text we write must be the text that assembles: parse it back
        // through the same registry and configs before touching disk.
        let rebuilt = assemble_source(&text, &book, &symbols, &configs, inames)
            .map_err(|error| NativeError::Invalid(format!("{group}/{file}: {error}")))?;
        if &rebuilt != original {
            return Err(NativeError::Invalid(format!(
                "{group}/{file}: source round trip changed bytes"
            )));
        }
        std::fs::write(out_dir.join(format!("{group}_{file}.rs2")), text)
            .map_err(NativeError::Io)?;
        report.files += 1;
    }
    std::fs::write(out_dir.join("symbols.txt"), symbols.emit_symbols_txt())
        .map_err(NativeError::Io)?;
    if !execution.is_empty() {
        std::fs::write(metadata_path, execution.emit()).map_err(NativeError::Io)?;
    } else if metadata_path.exists() {
        std::fs::remove_file(metadata_path).map_err(NativeError::Io)?;
    }
    Ok(report)
}

/// Parse source, lower, encode, and verify: the output must decode back to the
/// lowered model exactly (command, operand, and header — the byte fidelity the
/// corpus gate holds us to). `configs` resolves config-typed reads at parse
/// and lower; empty rejects every such read as unknown. `inames` resolves
/// symbolic component refs at parse (empty rejects every symbolic ref as
/// unknown, numeric text unaffected).
pub fn assemble_source(
    text: &str,
    book: &OpcodeBook,
    symbols: &SymbolRegistry,
    configs: &ConfigTypes,
    inames: &InterfaceRegistry,
) -> Result<Vec<u8>> {
    let lowered = lower(
        &crate::parse::parse_source(text, symbols, configs, inames)?,
        book,
        symbols,
        configs,
    )?;
    let bytes = encode_script(&lowered, book)?;
    let decoded = decode_script(&bytes, book)?;
    if decoded != lowered {
        return Err(NativeError::Invalid(
            "post-assemble verification failed: re-decoded script differs from the lowered model"
                .to_string(),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> OpcodeBook {
        OpcodeBook::embedded().unwrap()
    }

    fn configs() -> ConfigTypes {
        ConfigTypes::empty()
    }

    fn inames() -> InterfaceRegistry {
        InterfaceRegistry::empty()
    }

    fn param(ty: ValType, name: &str) -> Param {
        Param {
            ty,
            name: name.to_string(),
        }
    }

    fn instr(command: &str, operand: SourceOperand) -> SourceStmt {
        SourceStmt::Instr(SourceInstr {
            command: command.to_string(),
            operand,
        })
    }

    /// Hand-written source exercising labels (forward + backward), all slot
    /// types, and a switch.
    fn example_source() -> SourceScript {
        SourceScript {
            name: Some("clientscript,example".to_string()),
            args: vec![param(ValType::Int, "param0")],
            locals: vec![
                param(ValType::Int, "temp0"),
                param(ValType::String, "temp1"),
            ],
            body: vec![
                instr("push_constant_int", SourceOperand::Int(41)),
                instr("push_int_local", SourceOperand::Local("param0".to_string())),
                instr("add", SourceOperand::Raw(0)),
                instr("pop_int_local", SourceOperand::Local("temp0".to_string())),
                instr("branch_if_false", SourceOperand::Label("end".to_string())),
                instr(
                    "push_constant_string",
                    SourceOperand::Str("done".to_string()),
                ),
                instr(
                    "pop_string_local",
                    SourceOperand::Local("temp1".to_string()),
                ),
                SourceStmt::Label("end".to_string()),
                instr("push_int_local", SourceOperand::Local("temp0".to_string())),
                instr(
                    "switch",
                    SourceOperand::Cases(vec![(0, "end".to_string()), (-1, "start".to_string())]),
                ),
                SourceStmt::Label("start".to_string()),
                instr("return", SourceOperand::Raw(0)),
            ],
        }
    }

    #[test]
    fn lower_resolves_labels_and_slots() {
        use crate::symbols::SymbolRegistry;
        let book = book();
        let symbols = SymbolRegistry::empty();
        let lowered = lower(&example_source(), &book, &symbols, &configs()).unwrap();
        assert_eq!(lowered.args.int, 1);
        assert_eq!(lowered.locals.int, 2);
        assert_eq!(lowered.locals.obj, 1);
        assert_eq!(lowered.code.len(), 10);
        // branch_if_false at 4 jumps over 3 instructions to `end` at 7.
        assert!(matches!(lowered.code[4].operand, Operand::Branch(7)));
        // Forward switch case to `end`, backward case to `start` at 9.
        match &lowered.code[8].operand {
            Operand::Switch(cases) => {
                assert_eq!(cases[0].target, 7);
                assert_eq!(cases[1].target, 9);
            }
            other => panic!("expected switch, got {other:?}"),
        }
        // $param0 shares int slot 0 with the binary arg; $temp0 follows it.
        assert!(matches!(lowered.code[1].operand, Operand::Local(0)));
        assert!(matches!(lowered.code[3].operand, Operand::Local(1)));
        // Opcodes are canonical book ids.
        assert_eq!(
            lowered.code[0].opcode,
            book.opcode_for("push_constant_int").unwrap()
        );
    }

    #[test]
    fn lower_rejects_bad_models_loudly() {
        use crate::symbols::SymbolRegistry;
        let book = book();
        let symbols = SymbolRegistry::empty();
        let mut bad = example_source();
        bad.body
            .push(instr("branch", SourceOperand::Label("nowhere".to_string())));
        assert!(lower(&bad, &book, &symbols, &configs()).is_err());

        let mut bad = example_source();
        bad.body.push(SourceStmt::Label("end".to_string()));
        assert!(lower(&bad, &book, &symbols, &configs()).is_err());

        let mut bad = example_source();
        bad.body.push(instr(
            "push_int_local",
            SourceOperand::Local("ghost".to_string()),
        ));
        assert!(lower(&bad, &book, &symbols, &configs()).is_err());

        // String local through an int command.
        let mut bad = example_source();
        bad.body.push(instr(
            "push_int_local",
            SourceOperand::Local("temp1".to_string()),
        ));
        assert!(lower(&bad, &book, &symbols, &configs()).is_err());

        // Unknown command.
        let mut bad = example_source();
        bad.body
            .push(instr("no_such_command", SourceOperand::Raw(0)));
        assert!(lower(&bad, &book, &symbols, &configs()).is_err());

        // Call to an unknown name.
        let mut bad = example_source();
        bad.body.push(instr(
            "gosub_with_params",
            SourceOperand::Call("ghost".to_string(), Vec::new()),
        ));
        assert!(lower(&bad, &book, &symbols, &configs()).is_err());
    }

    #[test]
    fn format_is_canonical_text() {
        use crate::symbols::SymbolRegistry;
        let text = format_source(&example_source(), &SymbolRegistry::empty(), &inames()).unwrap();
        let expected = "\
[clientscript,example](int $param0)
int $temp0;
string $temp1;

    push_constant_int(41);
    push_int_local($param0);
    add(0);
    pop_int_local($temp0);
    branch_if_false(end);
    push_constant_string(\"done\");
    pop_string_local($temp1);
end:
    push_int_local($temp0);
    switch {
        case 0 -> end;
        case -1 -> start;
    }
start:
    return(0);
";
        assert_eq!(text, expected);
    }

    fn binstr(command: &str, operand: Operand) -> Instruction {
        Instruction {
            opcode: 0,
            command: command.to_string(),
            operand,
        }
    }

    fn fold_registry() -> crate::symbols::SymbolRegistry {
        use crate::symbols::SymbolRegistry;
        use std::collections::BTreeMap;
        let known = BTreeMap::from([(
            100,
            Counts {
                int: 2,
                obj: 1,
                long: 0,
            },
        )]);
        SymbolRegistry::build(vec![(100, "takes_three".to_string())], &known).unwrap()
    }

    /// Interleaved push order folds, preserving that order.
    fn interleaved_call_script() -> CompiledScript {
        CompiledScript {
            name: None,
            locals: Counts {
                int: 1,
                ..Counts::default()
            },
            args: Counts::default(),
            code: vec![
                binstr("push_constant_string", Operand::Str("x".into())),
                binstr("push_constant_int", Operand::Int(41)),
                binstr("push_int_local", Operand::Local(0)),
                binstr("gosub_with_params", Operand::Script(100)),
                binstr("return", Operand::Byte(0)),
            ],
        }
    }

    #[test]
    fn lift_folds_exact_call_patterns() {
        let symbols = fold_registry();
        let lifted = lift(&interleaved_call_script(), &symbols, &configs(), &inames()).unwrap();
        assert_eq!(lifted.body.len(), 2);
        let SourceStmt::Instr(call) = &lifted.body[0] else {
            panic!("expected a folded call first");
        };
        assert_eq!(call.command, "gosub_with_params");
        let SourceOperand::Call(name, args) = &call.operand else {
            panic!("expected Call, got {:?}", call.operand);
        };
        assert_eq!(name, "takes_three");
        // Original (interleaved) order kept, not canonicalized.
        assert!(matches!(
            args.as_slice(),
            [CallArg::Str(_), CallArg::Int(41), CallArg::Local(_)]
        ));
        assert!(matches!(lifted.body[1], SourceStmt::Instr(_)));
    }

    #[test]
    fn lift_leaves_inexact_calls_numeric() {
        let symbols = fold_registry();
        // Unnamed callee.
        let script = CompiledScript {
            name: None,
            locals: Counts::default(),
            args: Counts::default(),
            code: vec![
                binstr("push_constant_int", Operand::Int(1)),
                binstr("gosub_with_params", Operand::Script(101)),
                binstr("return", Operand::Byte(0)),
            ],
        };
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(matches!(
            &lifted.body[1],
            SourceStmt::Instr(instr) if matches!(&instr.operand, SourceOperand::Script(101))
        ));

        // Computed argument (add result, not a pure push).
        let script = CompiledScript {
            name: None,
            locals: Counts::default(),
            args: Counts::default(),
            code: vec![
                binstr("push_constant_int", Operand::Int(1)),
                binstr("push_constant_int", Operand::Int(2)),
                binstr("add", Operand::Byte(0)),
                binstr("gosub_with_params", Operand::Script(100)),
                binstr("return", Operand::Byte(0)),
            ],
        };
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(lifted.body.iter().all(|stmt| !matches!(
            stmt,
            SourceStmt::Instr(instr) if matches!(&instr.operand, SourceOperand::Call(..))
        )));

        // Jump target strictly inside the argument run blocks folding, even
        // though every push matches: folding would orphan the label.
        let script = CompiledScript {
            name: None,
            locals: Counts::default(),
            args: Counts::default(),
            code: vec![
                binstr("push_constant_int", Operand::Int(1)),
                binstr("push_constant_int", Operand::Int(2)),
                binstr("push_constant_string", Operand::Str("x".into())),
                binstr("gosub_with_params", Operand::Script(100)),
                binstr("branch", Operand::Branch(2)),
                binstr("return", Operand::Byte(0)),
            ],
        };
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(lifted.body.iter().all(|stmt| !matches!(
            stmt,
            SourceStmt::Instr(instr) if matches!(&instr.operand, SourceOperand::Call(..))
        )));

        // A boundary label (on the first push) folds fine and stays put on
        // the call itself.
        let script = CompiledScript {
            name: None,
            locals: Counts::default(),
            args: Counts::default(),
            code: vec![
                binstr("push_constant_int", Operand::Int(1)),
                binstr("push_constant_int", Operand::Int(2)),
                binstr("push_constant_string", Operand::Str("x".into())),
                binstr("gosub_with_params", Operand::Script(100)),
                binstr("branch", Operand::Branch(0)),
                binstr("return", Operand::Byte(0)),
            ],
        };
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(matches!(
            lifted.body.as_slice(),
            [
                SourceStmt::Label(_),
                SourceStmt::Instr(_),
                SourceStmt::Instr(_),
                SourceStmt::Instr(_)
            ]
        ));
        assert!(matches!(
            &lifted.body[1],
            SourceStmt::Instr(instr) if matches!(&instr.operand, SourceOperand::Call(..))
        ));
    }

    #[test]
    fn lower_calls_use_measured_arg_spellings() {
        let book = book();
        let symbols = fold_registry();
        let lifted = lift(&interleaved_call_script(), &symbols, &configs(), &inames()).unwrap();
        let lowered = lower(&lifted, &book, &symbols, &configs()).unwrap();
        // Five binary instructions back: three pushes, the gosub, the return.
        assert_eq!(lowered.code.len(), 5);
        // The int literal lowers to the corpus-measured const-string spelling,
        // never bare push_constant_int.
        assert_eq!(lowered.code[1].command, "push_constant_string");
        assert!(matches!(lowered.code[1].operand, Operand::Int(41)));
        assert_eq!(lowered.code[0].command, "push_constant_string");
        assert!(matches!(lowered.code[3].operand, Operand::Script(100)));
    }

    #[test]
    fn varbit_reads_fold_as_int_arguments() {
        use crate::symbols::SymbolRegistry;
        use std::collections::BTreeMap;
        let book = book();
        // Callee takes a single int.
        let known = BTreeMap::from([(
            200,
            Counts {
                int: 1,
                obj: 0,
                long: 0,
            },
        )]);
        let symbols =
            SymbolRegistry::build(vec![(200, "takes_varbit".to_string())], &known).unwrap();

        let script = CompiledScript {
            name: None,
            locals: Counts::default(),
            args: Counts::default(),
            code: vec![
                binstr(
                    "push_varbit",
                    Operand::VarBitRef(crate::script::VarBitRef {
                        id: 42,
                        transmog: true,
                    }),
                ),
                binstr("gosub_with_params", Operand::Script(200)),
                binstr("return", Operand::Byte(0)),
            ],
        };
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(matches!(
            &lifted.body[0],
            SourceStmt::Instr(instr) if matches!(&instr.operand, SourceOperand::Call(name, args)
                if name == "takes_varbit" && args.as_slice() == [CallArg::VarBit(42, true)])
        ));
        // Rendering names the varbit; lowering restores the exact operand.
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(text.contains("    ~takes_varbit(varbit:42:transmog);\n"));
        let lowered = lower(&lifted, &book, &symbols, &configs()).unwrap();
        assert!(matches!(
            &lowered.code[0].operand,
            Operand::VarBitRef(varbit) if varbit.id == 42 && varbit.transmog
        ));
        // Byte-identical to lowering the unfolded original.
        let direct = lower(
            &lift(&script, &SymbolRegistry::empty(), &configs(), &inames()).unwrap(),
            &book,
            &SymbolRegistry::empty(),
            &configs(),
        )
        .unwrap();
        assert_eq!(
            encode_script(&lowered, &book).unwrap(),
            encode_script(&direct, &book).unwrap()
        );
    }

    #[test]
    fn lower_rejects_bad_calls() {
        let book = book();
        let symbols = fold_registry();
        let call = |args: Vec<CallArg>| SourceScript {
            name: None,
            args: Vec::new(),
            locals: Vec::new(),
            body: vec![
                SourceStmt::Instr(SourceInstr {
                    command: "gosub_with_params".to_string(),
                    operand: SourceOperand::Call("takes_three".to_string(), args),
                }),
                SourceStmt::Instr(SourceInstr {
                    command: "return".to_string(),
                    operand: SourceOperand::Raw(0),
                }),
            ],
        };
        // Arity mismatch (expects 2i,1o).
        assert!(lower(&call(vec![CallArg::Int(1)]), &book, &symbols, &configs()).is_err());
        // Unknown name.
        assert!(
            lower(
                &SourceScript {
                    body: vec![SourceStmt::Instr(SourceInstr {
                        command: "gosub_with_params".to_string(),
                        operand: SourceOperand::Call("ghost".to_string(), Vec::new()),
                    })],
                    ..call(Vec::new())
                },
                &book,
                &symbols,
                &configs()
            )
            .is_err()
        );
    }

    /// Build a decoded script from `(command, operand)` pairs with canonical
    /// opcodes, the shape [`lift`] consumes.
    fn decoded(args: Counts, locals: Counts, code: Vec<(&str, Operand)>) -> CompiledScript {
        let book = book();
        CompiledScript {
            name: None,
            args,
            locals: Counts {
                int: args.int + locals.int,
                obj: args.obj + locals.obj,
                long: args.long + locals.long,
            },
            code: code
                .into_iter()
                .map(|(command, operand)| Instruction {
                    opcode: book.opcode_for(command).unwrap(),
                    command: command.to_string(),
                    operand,
                })
                .collect(),
        }
    }

    fn has_assign(body: &[SourceStmt]) -> bool {
        body.iter()
            .any(|stmt| matches!(stmt, SourceStmt::Assign { .. }))
    }

    fn int_counts(value: u16) -> Counts {
        Counts {
            int: value,
            obj: 0,
            long: 0,
        }
    }

    #[test]
    fn lift_folds_add_window_into_assignment() {
        let symbols = SymbolRegistry::empty();
        let script = decoded(
            int_counts(1),
            int_counts(1),
            vec![
                ("push_int_local", Operand::Local(0)),
                ("push_constant_string", Operand::Int(41)),
                ("add", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(1)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert_eq!(lifted.body.len(), 2);
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert_eq!(
            *expr,
            Expr::Binary(
                BinaryOp::Add,
                Box::new(Expr::Local("param0".to_string())),
                Box::new(Expr::LitInt(41)),
            )
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    $temp0 = $param0 + 41;\n"),
            "unexpected text:\n{text}"
        );
    }

    #[test]
    fn lift_builds_nested_trees_in_evaluation_order() {
        let symbols = SymbolRegistry::empty();
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_string", Operand::Int(1)),
                ("push_constant_string", Operand::Int(2)),
                ("push_constant_string", Operand::Int(3)),
                ("multiply", Operand::Byte(0)),
                ("add", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert_eq!(
            *expr,
            Expr::Binary(
                BinaryOp::Add,
                Box::new(Expr::LitInt(1)),
                Box::new(Expr::Binary(
                    BinaryOp::Multiply,
                    Box::new(Expr::LitInt(2)),
                    Box::new(Expr::LitInt(3)),
                )),
            )
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    $temp0 = 1 + 2 * 3;\n"),
            "unexpected text:\n{text}"
        );
    }

    #[test]
    fn lift_folds_string_operator_windows() {
        let symbols = SymbolRegistry::empty();
        let script = decoded(
            Counts::default(),
            Counts {
                int: 1,
                obj: 1,
                long: 0,
            },
            vec![
                ("push_string_local", Operand::Local(0)),
                ("string_length", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        // Int and string slots live in separate tables (`temp0` is the int,
        // `temp1` the string).
        assert_eq!(target, "temp0");
        assert_eq!(
            *expr,
            Expr::Unary(
                UnaryOp::StringLength,
                Box::new(Expr::Local("temp1".to_string())),
            )
        );
    }

    fn config_tables() -> ConfigTypes {
        use crate::config::{EnumConfig, ParamConfig};
        use std::collections::BTreeMap;
        ConfigTypes::from_parts(
            BTreeMap::from([
                (
                    0,
                    ParamConfig {
                        kind: Some(0),
                        ..Default::default()
                    },
                ),
                (
                    65,
                    ParamConfig {
                        kind: Some(36),
                        ..Default::default()
                    },
                ),
            ]),
            BTreeMap::from([
                (
                    688,
                    EnumConfig {
                        output_type: Some(57),
                        ..Default::default()
                    },
                ),
                (
                    3907,
                    EnumConfig {
                        output_type: Some(36),
                        ..Default::default()
                    },
                ),
            ]),
        )
    }

    #[test]
    fn lift_folds_config_reads_into_assignments() {
        let symbols = SymbolRegistry::empty();
        let configs = config_tables();
        // Int-lane param read: holder push, literal id push, command, pop.
        let script = decoded(
            int_counts(1),
            int_counts(1),
            vec![
                ("push_int_local", Operand::Local(0)),
                ("push_constant_string", Operand::Int(0)),
                ("struct_param", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(1)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs, &inames()).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert_eq!(
            *expr,
            Expr::Param {
                op: ParamOp::StructParam,
                obj: Box::new(Expr::Local("param0".to_string())),
                param: 0,
            }
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    $temp0 = struct_param($param0, 0);\n"),
            "unexpected text:\n{text}"
        );
        // String-lane param read pops into a string slot.
        let script = decoded(
            int_counts(1),
            Counts {
                int: 0,
                obj: 1,
                long: 0,
            },
            vec![
                ("push_int_local", Operand::Local(0)),
                ("push_constant_string", Operand::Int(65)),
                ("oc_param", Operand::Byte(0)),
                ("pop_string_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs, &inames()).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert!(matches!(
            expr,
            Expr::Param {
                op: ParamOp::OcParam,
                param: 65,
                ..
            }
        ));
        // Enum reads fold the same way (four pushes, command, pop).
        let script = decoded(
            int_counts(1),
            int_counts(1),
            vec![
                ("push_constant_string", Operand::Int(0)),
                ("push_constant_string", Operand::Int(57)),
                ("push_constant_string", Operand::Int(688)),
                ("push_int_local", Operand::Local(0)),
                ("_enum", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(1)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs, &inames()).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert!(matches!(
            expr,
            Expr::Enum {
                enumeration: 688,
                ..
            }
        ));
    }

    #[test]
    fn lift_leaves_unresolvable_config_windows_flat() {
        let symbols = SymbolRegistry::empty();
        let tables = config_tables();
        // Unknown param id: no lane, stays flat.
        let script = decoded(
            int_counts(1),
            int_counts(1),
            vec![
                ("push_int_local", Operand::Local(0)),
                ("push_constant_string", Operand::Int(999_999)),
                ("struct_param", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(1)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &tables, &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "an unknown param id must stay flat"
        );
        // Empty tables: every real id is unknown, stays flat.
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(!has_assign(&lifted.body), "missing config must stay flat");
        // Non-literal param id (a `$local` where the literal belongs): the
        // statically-known subset only, stays flat.
        let script = decoded(
            int_counts(2),
            int_counts(1),
            vec![
                ("push_int_local", Operand::Local(0)),
                ("push_int_local", Operand::Local(1)),
                ("struct_param", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(2)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &tables, &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a non-literal param id must stay flat"
        );
        // Lane mismatch (string read into an int pop): the forward type check
        // fails, stays flat rather than misshaping a tree.
        let script = decoded(
            int_counts(1),
            int_counts(1),
            vec![
                ("push_int_local", Operand::Local(0)),
                ("push_constant_string", Operand::Int(65)),
                ("struct_param", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(1)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &tables, &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a lane-mismatched window must stay flat"
        );
    }

    #[test]
    fn lift_leaves_jump_into_window_flat() {
        let symbols = SymbolRegistry::empty();
        let script = decoded(
            int_counts(1),
            int_counts(1),
            vec![
                ("branch", Operand::Branch(3)),
                ("push_int_local", Operand::Local(0)),
                ("push_constant_string", Operand::Int(1)),
                ("add", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(1)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a jump into the run must stay flat"
        );
    }

    #[test]
    fn lift_leaves_nonzero_op_operand_flat() {
        let symbols = SymbolRegistry::empty();
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_string", Operand::Int(1)),
                ("push_constant_string", Operand::Int(2)),
                ("add", Operand::Byte(5)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a non-canonical operator operand must stay flat"
        );
    }

    #[test]
    fn lift_leaves_mistyped_window_flat() {
        // Not a valid script (`add` over strings), but lift must degrade to
        // flat text rather than misshape a tree: decode does not check stack
        // types, so lift cannot assume them.
        let symbols = SymbolRegistry::empty();
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_string", Operand::Str("a".to_string())),
                ("push_constant_string", Operand::Str("b".to_string())),
                ("add", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(!has_assign(&lifted.body), "a mistyped run must stay flat");
    }

    #[test]
    fn lift_folds_scale_window_into_assignment() {
        // The n-ary family (`scale` arity 3, `interpolate` arity 5) folds a
        // window of literal pushes into one assignment that re-lowers
        // byte-identically.
        use crate::expr::NaryOp;
        let symbols = SymbolRegistry::empty();
        let book = book();
        for (op, command, arity, want_line) in [
            (NaryOp::Scale, "scale", 3, "    $temp0 = scale(1, 2, 3);\n"),
            (
                NaryOp::Interpolate,
                "interpolate",
                5,
                "    $temp0 = interpolate(1, 2, 3, 4, 5);\n",
            ),
        ] {
            let values: Vec<i32> = (1..=arity).collect();
            let mut code: Vec<(&str, Operand)> = values
                .iter()
                .map(|value| ("push_constant_string", Operand::Int(*value)))
                .collect();
            code.push((command, Operand::Byte(0)));
            code.push(("pop_int_local", Operand::Local(0)));
            code.push(("return", Operand::Byte(0)));
            let script = decoded(Counts::default(), int_counts(1), code);
            let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
            let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
                panic!("{op:?}: expected an assignment, got {:?}", lifted.body[0]);
            };
            assert_eq!(target, "temp0");
            assert_eq!(
                *expr,
                Expr::Nary(
                    op,
                    values.iter().map(|value| Expr::LitInt(*value)).collect()
                ),
            );
            let text = format_source(&lifted, &symbols, &inames()).unwrap();
            assert!(text.contains(want_line), "{op:?}: unexpected text:\n{text}");
            // The folded window re-lowers byte-identically.
            let lowered = lower(&lifted, &book, &symbols, &configs()).unwrap();
            let want: Vec<(String, String)> = script
                .code
                .iter()
                .map(|instr| (instr.command.clone(), format!("{:?}", instr.operand)))
                .collect();
            let got: Vec<(String, String)> = lowered
                .code
                .iter()
                .map(|instr| (instr.command.clone(), format!("{:?}", instr.operand)))
                .collect();
            assert_eq!(got, want, "{op:?} lift must re-lower byte-identically");
        }
    }

    #[test]
    fn lift_folds_join_string_windows_into_assignments() {
        let symbols = SymbolRegistry::empty();
        let book = book();
        let strings = Counts {
            int: 0,
            obj: 2,
            long: 0,
        };
        // Two parts: the smallest join window.
        let script = decoded(
            Counts::default(),
            strings,
            vec![
                ("push_constant_string", Operand::Str("a".to_string())),
                ("push_string_local", Operand::Local(1)),
                ("join_string", Operand::Count(2)),
                ("pop_string_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert_eq!(
            *expr,
            Expr::Join(vec![
                Expr::LitStr("a".to_string()),
                Expr::Local("temp1".to_string()),
            ])
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    $temp0 = join_string(\"a\", $temp1);\n"),
            "unexpected text:\n{text}"
        );
        // The folded window re-lowers byte-identically (explicit count and
        // all — the flat `join_string(2)` spelling and the expression form
        // are the same bytes).
        let lowered = lower(&lifted, &book, &symbols, &configs()).unwrap();
        let want: Vec<(String, String)> = script
            .code
            .iter()
            .map(|instr| (instr.command.clone(), format!("{:?}", instr.operand)))
            .collect();
        let got: Vec<(String, String)> = lowered
            .code
            .iter()
            .map(|instr| (instr.command.clone(), format!("{:?}", instr.operand)))
            .collect();
        assert_eq!(got, want, "join lift must re-lower byte-identically");
        // Three parts with nesting: `append` feeds `join` left-first.
        let script = decoded(
            Counts::default(),
            strings,
            vec![
                ("push_string_local", Operand::Local(0)),
                ("push_string_local", Operand::Local(1)),
                ("append", Operand::Byte(0)),
                ("push_constant_string", Operand::Str("!".to_string())),
                ("join_string", Operand::Count(2)),
                ("pop_string_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert!(
            matches!(expr, Expr::Join(parts) if parts.len() == 2 && matches!(&parts[0], Expr::Binary(BinaryOp::Append, _, _))),
            "unexpected expression: {expr:?}"
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    $temp0 = join_string(append($temp0, $temp1), \"!\");\n"),
            "unexpected text:\n{text}"
        );
        // A degenerate count has no expression form: stays flat.
        let script = decoded(
            Counts::default(),
            strings,
            vec![
                ("push_string_local", Operand::Local(1)),
                ("join_string", Operand::Count(1)),
                ("pop_string_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a degenerate join count must stay flat"
        );
        // A mistyped part (int among strings) stays flat, never misshaped.
        let script = decoded(
            Counts::default(),
            strings,
            vec![
                ("push_constant_string", Operand::Str("a".to_string())),
                ("push_constant_string", Operand::Int(1)),
                ("join_string", Operand::Count(2)),
                ("pop_string_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a mistyped join window must stay flat"
        );
        // A join whose value never reaches a pop stays flat too.
        let script = decoded(
            Counts::default(),
            strings,
            vec![
                ("push_constant_string", Operand::Str("a".to_string())),
                ("push_string_local", Operand::Local(1)),
                ("join_string", Operand::Count(2)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a join with no consuming pop must stay flat"
        );
    }

    #[test]
    fn lift_folds_tostring_window_into_assignment() {
        let symbols = SymbolRegistry::empty();
        let script = decoded(
            Counts::default(),
            Counts {
                int: 1,
                obj: 1,
                long: 0,
            },
            vec![
                ("push_int_local", Operand::Local(0)),
                ("tostring", Operand::Byte(0)),
                ("pop_string_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        // Int and string slots live in separate tables (`temp0` is the int,
        // `temp1` the string).
        assert_eq!(target, "temp1");
        assert_eq!(
            *expr,
            Expr::Unary(
                UnaryOp::ToString,
                Box::new(Expr::Local("temp0".to_string())),
            )
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    $temp1 = tostring($temp0);\n"),
            "unexpected text:\n{text}"
        );
    }

    #[test]
    fn lift_nests_new_operators_in_evaluation_order() {
        let symbols = SymbolRegistry::empty();
        // `push a; push b; push c; scale; push d; add` becomes
        // `$temp0 = scale($param0, $param1, $param2) + $param3;`.
        let script = decoded(
            Counts {
                int: 4,
                obj: 0,
                long: 0,
            },
            int_counts(1),
            vec![
                ("push_int_local", Operand::Local(0)),
                ("push_int_local", Operand::Local(1)),
                ("push_int_local", Operand::Local(2)),
                ("scale", Operand::Byte(0)),
                ("push_int_local", Operand::Local(3)),
                ("add", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(4)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert!(
            matches!(expr, Expr::Binary(BinaryOp::Add, _, _)),
            "unexpected expression: {expr:?}"
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    $temp0 = scale($param0, $param1, $param2) + $param3;\n"),
            "unexpected text:\n{text}"
        );
        // `push $x; tostring; string_length` chains int->obj->int.
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_string", Operand::Int(41)),
                ("tostring", Operand::Byte(0)),
                ("string_length", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert_eq!(
            *expr,
            Expr::Unary(
                UnaryOp::StringLength,
                Box::new(Expr::Unary(UnaryOp::ToString, Box::new(Expr::LitInt(41)),)),
            )
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    $temp0 = string_length(tostring(41));\n"),
            "unexpected text:\n{text}"
        );
    }

    #[test]
    fn lift_leaves_new_operator_lane_slips_flat() {
        let symbols = SymbolRegistry::empty();
        // `scale` over strings is not a valid run: lift degrades to flat
        // rather than misshaping a tree.
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_string", Operand::Str("a".to_string())),
                ("push_constant_string", Operand::Str("b".to_string())),
                ("push_constant_string", Operand::Str("c".to_string())),
                ("scale", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a mistyped scale run must stay flat"
        );
        // `tostring` over a string likewise stays flat.
        let script = decoded(
            Counts::default(),
            Counts {
                int: 1,
                obj: 1,
                long: 0,
            },
            vec![
                ("push_constant_string", Operand::Str("a".to_string())),
                ("tostring", Operand::Byte(0)),
                ("pop_string_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a mistyped tostring run must stay flat"
        );
    }

    fn value_registry() -> SymbolRegistry {
        let known = BTreeMap::from([(7, int_counts(1))]);
        SymbolRegistry::build(vec![(7, "seven".to_string())], &known).unwrap()
    }

    #[test]
    fn lift_fuses_single_value_call_with_pop() {
        let symbols = value_registry().with_returns(&BTreeMap::from([(7, int_counts(1))]));
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_string", Operand::Int(41)),
                ("gosub_with_params", Operand::Script(7)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert_eq!(lifted.body.len(), 2);
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert!(
            matches!(expr, Expr::Call { name, args } if name == "seven" && args.len() == 1),
            "unexpected expression: {expr:?}"
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    $temp0 = ~seven(41);\n"),
            "unexpected text:\n{text}"
        );
    }

    #[test]
    fn lift_leaves_unknown_return_call_unfused() {
        // No inference attached: the call stays a statement, the pop flat.
        let symbols = value_registry();
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_string", Operand::Int(41)),
                ("gosub_with_params", Operand::Script(7)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "an unknowable call must stay a statement"
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    ~seven(41);\n"),
            "unexpected text:\n{text}"
        );
        // Nested under an operator the same unknown return keeps the whole
        // window flat (the `lift_call_leaf` guard, not the pop fusion).
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_string", Operand::Int(41)),
                ("gosub_with_params", Operand::Script(7)),
                ("push_constant_string", Operand::Int(1)),
                ("add", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "an unknowable call must keep its operator window flat"
        );
    }

    #[test]
    fn lift_leaves_varbit_arg_value_call_unfused() {
        // Varbit reads fold into calls (always int-valued) but have no
        // expression syntax, so the value fusion bails while the statement
        // fold stands.
        let symbols = value_registry().with_returns(&BTreeMap::from([(7, int_counts(1))]));
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                (
                    "push_varbit",
                    Operand::VarBitRef(VarBitRef {
                        id: 5,
                        transmog: false,
                    }),
                ),
                ("gosub_with_params", Operand::Script(7)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a varbit argument must stay a statement call"
        );
        let text = format_source(&lifted, &symbols, &inames()).unwrap();
        assert!(
            text.contains("    ~seven(varbit:5);\n"),
            "unexpected text:\n{text}"
        );
    }

    fn nested_registry() -> SymbolRegistry {
        // `seven` takes one int and returns one int; `zero` takes nothing and
        // returns one int; `two` takes two ints and returns one int; `multi`
        // takes one int and returns two ints.
        let known = BTreeMap::from([
            (7, int_counts(1)),
            (
                8,
                Counts {
                    int: 0,
                    obj: 0,
                    long: 0,
                },
            ),
            (
                9,
                Counts {
                    int: 2,
                    obj: 0,
                    long: 0,
                },
            ),
            (
                10,
                Counts {
                    int: 1,
                    obj: 0,
                    long: 0,
                },
            ),
        ]);
        SymbolRegistry::build(
            vec![
                (7, "seven".to_string()),
                (8, "zero".to_string()),
                (9, "two".to_string()),
                (10, "multi".to_string()),
            ],
            &known,
        )
        .unwrap()
        .with_returns(&BTreeMap::from([
            (7, int_counts(1)),
            (8, int_counts(1)),
            (
                9,
                Counts {
                    int: 1,
                    obj: 0,
                    long: 0,
                },
            ),
            (
                10,
                Counts {
                    int: 2,
                    obj: 0,
                    long: 0,
                },
            ),
        ]))
    }

    #[test]
    fn lift_nests_call_under_operator() {
        // Value calls of every arity nest as `Expr::Call` leaves under an
        // operator, and each folded window re-lowers byte-identically.
        let call = |name: &str, args: Vec<Expr>| Expr::Call {
            name: name.to_string(),
            args,
        };
        let add =
            |left: Expr, right: Expr| Expr::Binary(BinaryOp::Add, Box::new(left), Box::new(right));
        let local = |name: &str| Expr::Local(name.to_string());
        let cases = vec![
            (
                // `push $param0; gosub seven; push 1; add; pop $temp0`.
                "one-arg call",
                1,
                vec![
                    ("push_int_local", Operand::Local(0)),
                    ("gosub_with_params", Operand::Script(7)),
                    ("push_constant_string", Operand::Int(1)),
                    ("add", Operand::Byte(0)),
                    ("pop_int_local", Operand::Local(1)),
                ],
                add(call("seven", vec![local("param0")]), Expr::LitInt(1)),
                "    $temp0 = ~seven($param0) + 1;\n",
            ),
            (
                // The goal shape verbatim: `push a; push b; gosub two; push c; add`.
                "two-arg call",
                3,
                vec![
                    ("push_int_local", Operand::Local(0)),
                    ("push_int_local", Operand::Local(1)),
                    ("gosub_with_params", Operand::Script(9)),
                    ("push_int_local", Operand::Local(2)),
                    ("add", Operand::Byte(0)),
                    ("pop_int_local", Operand::Local(3)),
                ],
                add(
                    call("two", vec![local("param0"), local("param1")]),
                    local("param2"),
                ),
                "    $temp0 = ~two($param0, $param1) + $param2;\n",
            ),
            (
                // Zero-arg calls carry no arguments, so no drift is possible.
                "zero-arg call",
                0,
                vec![
                    ("gosub_with_params", Operand::Script(8)),
                    ("push_constant_string", Operand::Int(2)),
                    ("add", Operand::Byte(0)),
                    ("pop_int_local", Operand::Local(0)),
                ],
                add(call("zero", Vec::new()), Expr::LitInt(2)),
                "    $temp0 = ~zero() + 2;\n",
            ),
            (
                // Deeper nesting: both leaves are calls.
                "two call leaves",
                0,
                vec![
                    ("push_constant_string", Operand::Int(41)),
                    ("gosub_with_params", Operand::Script(7)),
                    ("push_constant_string", Operand::Int(2)),
                    ("gosub_with_params", Operand::Script(7)),
                    ("add", Operand::Byte(0)),
                    ("pop_int_local", Operand::Local(0)),
                ],
                add(
                    call("seven", vec![Expr::LitInt(41)]),
                    call("seven", vec![Expr::LitInt(2)]),
                ),
                "    $temp0 = ~seven(41) + ~seven(2);\n",
            ),
        ];
        let symbols = nested_registry();
        let book = book();
        for (case, arg_ints, mut code, want_expr, want_line) in cases {
            code.push(("return", Operand::Byte(0)));
            let script = decoded(int_counts(arg_ints), int_counts(1), code);
            let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
            assert_eq!(lifted.body.len(), 2, "{case}: {:?}", lifted.body);
            let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
                panic!("{case}: expected an assignment, got {:?}", lifted.body[0]);
            };
            assert_eq!(target, "temp0", "{case}");
            assert_eq!(*expr, want_expr, "{case}");
            let text = format_source(&lifted, &symbols, &inames()).unwrap();
            assert!(text.contains(want_line), "{case}: unexpected text:\n{text}");
            let lowered = lower(&lifted, &book, &symbols, &configs()).unwrap();
            let want: Vec<(String, String)> = script
                .code
                .iter()
                .map(|instr| (instr.command.clone(), format!("{:?}", instr.operand)))
                .collect();
            let got: Vec<(String, String)> = lowered
                .code
                .iter()
                .map(|instr| (instr.command.clone(), format!("{:?}", instr.operand)))
                .collect();
            assert_eq!(
                got, want,
                "{case}: nested lift must re-lower byte-identically"
            );
        }
    }

    #[test]
    fn lift_leaves_bare_push_arg_nested_window_flat() {
        // A bare `push_constant_int` argument would drift bytes through
        // `lower_expr` (const-string family), so the window stays flat while
        // the statement fold stands.
        let symbols = nested_registry();
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_int", Operand::Int(41)),
                ("gosub_with_params", Operand::Script(7)),
                ("push_constant_string", Operand::Int(1)),
                ("add", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a bare-push call must keep its window flat, got {:?}",
            lifted.body
        );
    }

    #[test]
    fn lift_leaves_multi_return_nested_window_flat() {
        // A two-value callee can never sit in expression position.
        let symbols = nested_registry();
        let script = decoded(
            Counts::default(),
            int_counts(2),
            vec![
                ("push_constant_string", Operand::Int(41)),
                ("gosub_with_params", Operand::Script(10)),
                ("push_constant_string", Operand::Int(1)),
                ("add", Operand::Byte(0)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a multi-value call must keep its window flat"
        );
    }

    #[test]
    fn lift_leaves_jump_onto_gosub_unfolded() {
        // A jump landing exactly on the gosub must not fold: the argument
        // pushes may never have run, so `~seven(41)` would invent bytes.
        let symbols = value_registry();
        let script = decoded(
            Counts::default(),
            Counts::default(),
            vec![
                ("push_constant_string", Operand::Int(41)),
                ("branch", Operand::Branch(2)),
                ("gosub_with_params", Operand::Script(7)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            lifted.body.iter().all(|stmt| !matches!(
                stmt,
                SourceStmt::Instr(instr) if matches!(instr.operand, SourceOperand::Call(..))
            )),
            "a gosub targeted by a jump must stay numeric: {:?}",
            lifted.body
        );
        // Same shape without the jump folds: the guard is the target, not
        // the shape.
        let script = decoded(
            Counts::default(),
            Counts::default(),
            vec![
                ("push_constant_string", Operand::Int(41)),
                ("gosub_with_params", Operand::Script(7)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            lifted.body.iter().any(|stmt| matches!(
                stmt,
                SourceStmt::Instr(instr) if matches!(instr.operand, SourceOperand::Call(..))
            )),
            "the untargeted call must still fold"
        );
    }

    /// Registry naming interface 1253 `bank` (child 4 `main`) over a roster
    /// holding children 4 and 7, plus unnamed interface 9 with child 0.
    fn comp_names() -> InterfaceRegistry {
        let (ifaces, children) =
            crate::inames::parse_inames_txt("interface 1253 bank\ncomponent 1253 4 main\n")
                .unwrap();
        let roster = BTreeMap::from([(1253, vec![4, 7]), (9, vec![0])]);
        crate::inames::InterfaceRegistry::build(&ifaces, &children, &roster).unwrap()
    }

    #[test]
    fn lift_symbolizes_roster_known_pushes() {
        let symbols = SymbolRegistry::empty();
        let names = comp_names();
        // A pushed packed int the roster holds lifts to a Component operand.
        let script = decoded(
            Counts::default(),
            Counts::default(),
            vec![
                ("push_constant_string", Operand::Int(82_116_612)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &names).unwrap();
        assert!(matches!(
            &lifted.body[0],
            SourceStmt::Instr(instr) if matches!(&instr.operand, SourceOperand::Component { iface: 1253, child: 4 })
        ));
        // Named pairs render symbolically; the text reassembles byte-identical.
        let text = format_source(&lifted, &symbols, &names).unwrap();
        assert!(
            text.contains("    push_constant_string(bank/main);\n"),
            "unexpected text:\n{text}"
        );
        let book = book();
        let lowered = lower(&lifted, &book, &symbols, &configs()).unwrap();
        let plain = lower(
            &lift(&script, &SymbolRegistry::empty(), &configs(), &inames()).unwrap(),
            &book,
            &symbols,
            &configs(),
        )
        .unwrap();
        assert_eq!(
            encode_script(&lowered, &book).unwrap(),
            encode_script(&plain, &book).unwrap(),
            "symbolic lift must re-lower byte-identically"
        );
    }

    #[test]
    fn lift_symbolizes_assignment_leaves() {
        let symbols = SymbolRegistry::empty();
        let names = comp_names();
        // `push const; pop` over a packed int folds to `$y = bank/7;`.
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_string", Operand::Int(82_116_615)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &names).unwrap();
        let SourceStmt::Assign { target, expr } = &lifted.body[0] else {
            panic!("expected an assignment, got {:?}", lifted.body[0]);
        };
        assert_eq!(target, "temp0");
        assert_eq!(
            *expr,
            Expr::Component {
                iface: 1253,
                child: 7
            }
        );
        let text = format_source(&lifted, &symbols, &names).unwrap();
        assert!(
            text.contains("    $temp0 = bank/7;\n"),
            "unexpected text:\n{text}"
        );
        // And the folded assignment re-lowers byte-identically.
        let book = book();
        let lowered = lower(&lifted, &book, &symbols, &configs()).unwrap();
        let want: Vec<(String, String)> = script
            .code
            .iter()
            .map(|instr| (instr.command.clone(), format!("{:?}", instr.operand)))
            .collect();
        let got: Vec<(String, String)> = lowered
            .code
            .iter()
            .map(|instr| (instr.command.clone(), format!("{:?}", instr.operand)))
            .collect();
        assert_eq!(got, want, "symbolic fold must re-lower byte-identically");
    }

    #[test]
    fn lift_renders_unnamed_pairs_numeric() {
        let symbols = SymbolRegistry::empty();
        let names = comp_names();
        // Interface 9 is in the roster but unnamed: the pair symbolizes in
        // the model yet renders as the packed decimal (status-quo text).
        let script = decoded(
            Counts::default(),
            Counts::default(),
            vec![
                ("push_constant_string", Operand::Int(589_824)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &names).unwrap();
        assert!(matches!(
            &lifted.body[0],
            SourceStmt::Instr(instr) if matches!(&instr.operand, SourceOperand::Component { iface: 9, child: 0 })
        ));
        let text = format_source(&lifted, &symbols, &names).unwrap();
        assert!(
            text.contains("    push_constant_string(589824);\n"),
            "unexpected text:\n{text}"
        );
        // Plain ints outside the roster never symbolize at all.
        let script = decoded(
            Counts::default(),
            Counts::default(),
            vec![
                ("push_constant_string", Operand::Int(41)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &names).unwrap();
        assert!(matches!(
            &lifted.body[0],
            SourceStmt::Instr(instr) if matches!(&instr.operand, SourceOperand::Int(41))
        ));
        // An empty registry lifts exactly as before (nothing symbolizes).
        let script = decoded(
            Counts::default(),
            Counts::default(),
            vec![
                ("push_constant_string", Operand::Int(82_116_612)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(matches!(
            &lifted.body[0],
            SourceStmt::Instr(instr) if matches!(&instr.operand, SourceOperand::Int(82_116_612))
        ));
    }

    #[test]
    fn bare_push_spelling_symbolizes_but_stays_flat() {
        let symbols = SymbolRegistry::empty();
        let names = comp_names();
        // `push_constant_int` has no expression form, so its window stays flat
        // (as before) while rendering symbolically and reassembling exactly.
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_int", Operand::Int(82_116_612)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &names).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a bare-push window must stay flat"
        );
        let text = format_source(&lifted, &symbols, &names).unwrap();
        assert!(
            text.contains("    push_constant_int(bank/main);\n"),
            "unexpected text:\n{text}"
        );
        let book = book();
        let lowered = lower(&lifted, &book, &symbols, &configs()).unwrap();
        assert!(matches!(lowered.code[0].operand, Operand::Int(82_116_612)));
        assert_eq!(lowered.code[0].command, "push_constant_int");
        // A plain (non-component) `push_constant_int` copy stays flat too:
        // the assignment lowerer would re-emit it as `push_constant_string`,
        // so folding it would drift bytes.
        let script = decoded(
            Counts::default(),
            int_counts(1),
            vec![
                ("push_constant_int", Operand::Int(7)),
                ("pop_int_local", Operand::Local(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let lifted = lift(&script, &symbols, &configs(), &inames()).unwrap();
        assert!(
            !has_assign(&lifted.body),
            "a non-canonical push must stay flat"
        );
    }
}
