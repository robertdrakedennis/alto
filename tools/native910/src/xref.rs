//! Script and component relationships with explicit evidence boundaries.
//!
//! Packed operands resolve through exact straight-line stack traffic, cached
//! hook arguments, and (with config types) complete typed CFG analyses. CFG
//! constants survive only agreeing joins; a failed analysis supplies no facts.
//! Cached argument refinement is conditional on that hook entry and is retained
//! in `entry_contexts`, since runtime hook installations and triggers may supply
//! other arguments. Unknown operands are retained in an unresolved-site ledger.
//!
//! Cached hooks link their head script ID. Proven installing `if_seton*` and
//! `cc_seton*` commands also link their installed script, using the shared hook
//! descriptor contract when typed CFG state is available. Trailing transmit
//! arrays require a known count. Discarding swipe/pinch/gamepad setters are
//! excluded, and hook clears remain distinct from missing script references.
//!
//! Active component scope starts unknown. Two operand-selected slots are tracked
//! independently through creates and finds; calls and ambiguous joins invalidate
//! scope. Find-success guard claims require a single incoming edge with no
//! fall-through. Boolean local provenance expires when its originating scope is
//! overwritten. Runtime children retain their creator instruction and optional
//! packed parent in `dynamic_touches` and `dynamic_hook_sets`; they are never
//! mistaken for packed components. Runtime-selected children and inherited entry
//! scope need runtime evidence. Roster checks distinguish resolved and dangling
//! packed identities; absence of an edge never proves absence of behavior.

use crate::effects::{Effect, effect};
use crate::error::{NativeError, Result};
use crate::interface::{HookArg, InterfaceComponent, decode_component};
use crate::isource::hook_slots;
use crate::opcode::OpcodeBook;
use crate::pack::PackArchive;
use crate::script::{CompiledScript, Instruction, Operand};
use crate::symbols::SymbolRegistry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A packed component reference: interface id plus child (file) index.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ComponentRef {
    /// Interface (pack group) id.
    pub iface: i32,
    /// Child (pack file) index.
    pub child: i32,
}

const COMPONENT_CHILD_WIDTH: u32 = u16::BITS;
const COMPONENT_CHILD_MASK: i32 = u16::MAX as i32;
const MAX_INTERFACE_ID: i32 = i32::MAX >> COMPONENT_CHILD_WIDTH;

/// Unpack a packed component id (`iface << 16 | child`, as the client's
/// component lookup reads it: interface `arg >> 16`). Negative values
/// are not references — the client would crash resolving them — so they
/// yield `None`, never a guess.
#[must_use]
pub fn unpack_component(value: i32) -> Option<ComponentRef> {
    if value < 0 {
        return None;
    }
    Some(ComponentRef {
        iface: value >> COMPONENT_CHILD_WIDTH,
        child: value & COMPONENT_CHILD_MASK,
    })
}

/// Pack a reference back to its wire int. Fails on out-of-range parts rather
/// than wrapping.
#[must_use]
pub fn pack_component(reference: ComponentRef) -> Option<i32> {
    if reference.iface < 0
        || reference.iface > MAX_INTERFACE_ID
        || reference.child < 0
        || reference.child > COMPONENT_CHILD_MASK
    {
        return None;
    }
    reference
        .iface
        .checked_shl(COMPONENT_CHILD_WIDTH)
        .and_then(|high| high.checked_add(reference.child))
}

/// Commands proven to read a packed component id off the int stack, with the
/// id's depth below the int-stack top (0 = top). 171 rigid slot-0 commands
/// plus `if_triggerop` at depth 2 (`(packed, subindex, op)`); `cc_deleteall`
/// is the sole `cc_` member. Sorted for binary search.
const IF_COMPONENT_SLOT: &[(&str, u8)] = &[
    ("cc_deleteall", 0),
    ("if_callonresize", 0),
    ("if_clearops", 0),
    ("if_clearscripthooks", 0),
    ("if_discardGestureHook", 0),
    ("if_dragpickup", 0),
    ("if_get2dangle", 0),
    ("if_getcharindexatpos", 0),
    ("if_getcharposatindex", 0),
    ("if_getcolour", 0),
    ("if_getfontgraphic", 0),
    ("if_getfontmetrics", 0),
    ("if_getgraphic", 0),
    ("if_getgraphicdimensions", 0),
    ("if_getheight", 0),
    ("if_gethide", 0),
    ("if_getinvcount", 0),
    ("if_getinvobject", 0),
    ("if_getlayer", 0),
    ("if_getmodel", 0),
    ("if_getmodelangle_x", 0),
    ("if_getmodelangle_y", 0),
    ("if_getmodelangle_z", 0),
    ("if_getmodelxof", 0),
    ("if_getmodelyof", 0),
    ("if_getmodelzoom", 0),
    ("if_getnextsubid", 0),
    ("if_getop", 0),
    ("if_getopbase", 0),
    ("if_getparentlayer", 0),
    ("if_getscrollheight", 0),
    ("if_getscrollwidth", 0),
    ("if_getscrollx", 0),
    ("if_getscrolly", 0),
    ("if_gettargetmask", 0),
    ("if_gettext", 0),
    ("if_gettrans", 0),
    ("if_getwidth", 0),
    ("if_getx", 0),
    ("if_gety", 0),
    ("if_npc_setcustombodymodel", 0),
    ("if_npc_setcustombodymodel_transformed", 0),
    ("if_npc_setcustomheadmodel", 0),
    ("if_npc_setcustomrecol", 0),
    ("if_npc_setcustomretex", 0),
    ("if_resetlinkplayer", 0),
    ("if_resetmodellighting", 0),
    ("if_resume_pausebutton", 0),
    ("if_sendtoback", 0),
    ("if_sendtofront", 0),
    ("if_set2dangle", 0),
    ("if_setalpha", 0),
    ("if_setaspect", 0),
    ("if_setclickmask", 0),
    ("if_setcolour", 0),
    ("if_setdragdeadtime", 0),
    ("if_setdragdeadzone", 0),
    ("if_setdraggable", 0),
    ("if_setdragrenderbehaviour", 0),
    ("if_setfill", 0),
    ("if_setfontmono", 0),
    ("if_setgraphic", 0),
    ("if_setgraphicshadow", 0),
    ("if_setheld", 0),
    ("if_sethflip", 0),
    ("if_sethide", 0),
    ("if_setlinedirection", 0),
    ("if_setlinewid", 0),
    ("if_setlinkactiveclanchannel", 0),
    ("if_setlinkfriend", 0),
    ("if_setlinkfriendchat", 0),
    ("if_setlinkplayergroup", 0),
    ("if_setmaxlines", 0),
    ("if_setmodel", 0),
    ("if_setmodelangle", 0),
    ("if_setmodelanim", 0),
    ("if_setmodellighting", 0),
    ("if_setmodelorigin", 0),
    ("if_setmodelorthog", 0),
    ("if_setmodeltint", 0),
    ("if_setmodelzoom", 0),
    ("if_setmouseovercursor", 0),
    ("if_setnoclickthrough", 0),
    ("if_setnpchead", 0),
    ("if_setnpcmodel", 0),
    ("if_setobject", 0),
    ("if_setobject_alwaysnum", 0),
    ("if_setobject_nonum", 0),
    ("if_setobject_wearcol", 0),
    ("if_setobject_wearcol_alwaysnum", 0),
    ("if_setobject_wearcol_nonum", 0),
    ("if_setoncameraupdatetransmit", 0),
    ("if_setoncamfinished", 0),
    ("if_setonchattransmit", 0),
    ("if_setonclanchanneltransmit", 0),
    ("if_setonclansettingstransmit", 0),
    ("if_setonclantransmit", 0),
    ("if_setonclick", 0),
    ("if_setonclickrepeat", 0),
    ("if_setondialogabort", 0),
    ("if_setondrag", 0),
    ("if_setondragcomplete", 0),
    ("if_setondragcomplete_alias", 0),
    ("if_setonfriendtransmit", 0),
    ("if_setongamepadaxis", 0),
    ("if_setongamepadbutton", 0),
    ("if_setongamepadbuttonheld", 0),
    ("if_setongamepadtrigger", 0),
    ("if_setonhold", 0),
    ("if_setonhorizontalpinch", 0),
    ("if_setonhorizontalswipe", 0),
    ("if_setoninvtransmit", 0),
    ("if_setonkey", 0),
    ("if_setonmisctransmit", 0),
    ("if_setonmouseleave", 0),
    ("if_setonmouseover", 0),
    ("if_setonmouserepeat", 0),
    ("if_setonop", 0),
    ("if_setonopt", 0),
    ("if_setonplayergrouptransmit", 0),
    ("if_setonplayergroupvarptransmit", 0),
    ("if_setonrelease", 0),
    ("if_setonresize", 0),
    ("if_setonscrollwheel", 0),
    ("if_setonstattransmit", 0),
    ("if_setonstocktransmit", 0),
    ("if_setonsubchange", 0),
    ("if_setontargetenter", 0),
    ("if_setontargetleave", 0),
    ("if_setontimer", 0),
    ("if_setonvarclantransmit", 0),
    ("if_setonvarcstrtransmit", 0),
    ("if_setonvarctransmit", 0),
    ("if_setonvartransmit", 0),
    ("if_setonverticalpinch", 0),
    ("if_setonverticalswipe", 0),
    ("if_setop", 0),
    ("if_setopbase", 0),
    ("if_setopchar", 0),
    ("if_setopcursor", 0),
    ("if_setopkey", 0),
    ("if_setopkeyignoreheld", 0),
    ("if_setopkeyrate", 0),
    ("if_setoptchar", 0),
    ("if_setoptkey", 0),
    ("if_setoptkeyignoreheld", 0),
    ("if_setoptkeyrate", 0),
    ("if_setoutline", 0),
    ("if_setparam_int", 0),
    ("if_setparam_string", 0),
    ("if_setpausetext", 0),
    ("if_setplayerhead_self", 0),
    ("if_setplayermodel", 0),
    ("if_setplayermodel_self", 0),
    ("if_setposition", 0),
    ("if_setrecol", 0),
    ("if_setretex", 0),
    ("if_setscrollpos", 0),
    ("if_setscrollsize", 0),
    ("if_setsize", 0),
    ("if_settargetcursors", 0),
    ("if_settargetopcursor", 0),
    ("if_settargetverb", 0),
    ("if_settext", 0),
    ("if_settextalign", 0),
    ("if_settextantimacro", 0),
    ("if_settextfont", 0),
    ("if_settextshadow", 0),
    ("if_settiling", 0),
    ("if_settrans", 0),
    ("if_setvflip", 0),
    ("if_triggerop", 2),
];

/// The component-id depth for a command, if it is a proven component reader.
#[must_use]
pub fn component_slot(command: &str) -> Option<u8> {
    IF_COMPONENT_SLOT
        .binary_search_by(|(name, _)| name.cmp(&command))
        .ok()
        .map(|index| IF_COMPONENT_SLOT[index].1)
}

/// The 37 `if_seton*` commands proven to install a hook program (component
/// at slot 0, installed script id deeper on the int stack per
/// [`hook_script_depth`]). The 8 table siblings that pop-but-discard their
/// hook args (swipe/pinch/gamepad — see the module docs) are absent, as is
/// everything without a proven install. Sorted for binary search.
const IF_HOOK_SETTERS: &[&str] = &[
    "if_setoncameraupdatetransmit",
    "if_setoncamfinished",
    "if_setonchattransmit",
    "if_setonclanchanneltransmit",
    "if_setonclansettingstransmit",
    "if_setonclantransmit",
    "if_setonclick",
    "if_setonclickrepeat",
    "if_setondialogabort",
    "if_setondrag",
    "if_setondragcomplete",
    "if_setondragcomplete_alias",
    "if_setonfriendtransmit",
    "if_setonhold",
    "if_setoninvtransmit",
    "if_setonkey",
    "if_setonmisctransmit",
    "if_setonmouseleave",
    "if_setonmouseover",
    "if_setonmouserepeat",
    "if_setonop",
    "if_setonopt",
    "if_setonplayergrouptransmit",
    "if_setonplayergroupvarptransmit",
    "if_setonrelease",
    "if_setonresize",
    "if_setonscrollwheel",
    "if_setonstattransmit",
    "if_setonstocktransmit",
    "if_setonsubchange",
    "if_setontargetenter",
    "if_setontargetleave",
    "if_setontimer",
    "if_setonvarclantransmit",
    "if_setonvarcstrtransmit",
    "if_setonvarctransmit",
    "if_setonvartransmit",
];

/// Whether a command is a proven hook installer (see [`IF_HOOK_SETTERS`]).
#[must_use]
pub fn hook_setter(command: &str) -> bool {
    IF_HOOK_SETTERS.binary_search(&command).is_ok()
}

/// Every jump target in a decoded stream: branch destinations and switch
/// case targets. Shared by the backward operand walk (a target inside the
/// run ends it) and the forward scope simulation (a target merges paths, so
/// scope restarts unknown there).
fn jump_targets(code: &[Instruction]) -> BTreeSet<i32> {
    target_refcounts(code).keys().copied().collect()
}

/// How many branch/switch edges target each index: single-predecessor
/// targets are the only ones a guard can claim (see `check_guard`).
fn target_refcounts(code: &[Instruction]) -> BTreeMap<i32, usize> {
    let mut counts = BTreeMap::new();
    for instr in code {
        match &instr.operand {
            Operand::Branch(target) => {
                *counts.entry(*target).or_insert(0) += 1;
            }
            Operand::Switch(cases) => {
                for case in cases {
                    *counts.entry(case.target).or_insert(0) += 1;
                }
            }
            _ => {}
        }
    }
    counts
}

/// Cap on backward-walk int depth: hostile or pathological runs bail instead
/// of scanning forever. Real operand runs are a handful of pushes.
const MAX_WALK_DEPTH: i32 = 64;

/// Hook tail ints in this range are per-execution placeholders substituted
/// by hook entry, never literal values.
/// Owner-derived members resolve statically for single-installed scripts
/// (see below); the rest (mouse, keys, drop, opindex) are runtime values.
const HOOK_MAGIC_MIN: i32 = -2_147_483_647;
const HOOK_MAGIC_MAX: i32 = -2_147_483_639;
/// `-2147483645`: the installing component's `parentlayer` — which for pack
/// components IS the packed id (`(iface << 16) + file`).
const HOOK_MAGIC_PARENTLAYER: i32 = -2_147_483_645;
/// `-2147483643`: the installing component's `id` — always `-1` for pack
/// components (only `cc_create` assigns `id`; the field defaults to `-1`).
const HOOK_MAGIC_COMPONENT_ID: i32 = -2_147_483_643;

/// One hook installation of a script: the owning component plus the hook
/// program tail (head script id excluded) that becomes the script's entry
/// `$args`. Build-time only — the index records edges, not provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Installer {
    /// Owning `(interface, child)`.
    comp: (i32, u32),
    /// Tail args in listed order (ints and strings mixed, as carried).
    tail: Vec<HookArg>,
}

/// Resolved int entry values per int-arg slot (`None` = provably present but
/// unknowable: a runtime placeholder, assigned arg, or uncovered tail
/// position — never guessed).
pub type IntArgValues = BTreeMap<i32, Option<i32>>;

/// A known int `$local` value by slot, tracked within one straight-line
/// segment (cleared at every jump target). Complements [`IntArgValues`]:
/// args persist across merges only when never assigned (see
/// [`resolve_int_args`]), while locals are re-derived per segment from
/// adjacent copies — which is sound because a non-targeted pop is reachable
/// only from its previous index, so an adjacent push always just ran.
pub type LocalIntValues = BTreeMap<i32, i32>;

/// Look up a known int value for a pushed slot: `$arg` slots consult the
/// entry map (present-but-unknown counts as unknown), everything else the
/// segment-local copy map.
fn known_int(slot: i32, int_args: &IntArgValues, local_vals: &LocalIntValues) -> Option<i32> {
    match int_args.get(&slot) {
        Some(value) => *value,
        None => local_vals.get(&slot).copied(),
    }
}

/// A `$local` slot provably holding a find's pushed boolean (never its
/// value): which scope slot the source find wrote and the pack scope it
/// installed, captured when the tainting pop ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TaintedBool {
    /// Scope slot the source find wrote (`None` = unselectable operand, so
    /// the slot is unknowable — failure-nulling still applies, claiming does
    /// not).
    slot: Option<usize>,
    /// Pack scope the source find installed (`None` = the find resolved
    /// nothing — same asymmetry).
    parent: Option<ComponentRef>,
}

/// Find-boolean taints by `$local` slot, tracked within one straight-line
/// segment like [`LocalIntValues`]: set when an adjacent untargeted
/// `pop_int_local` immediately follows a find, cleared on any reassignment
/// of the slot, at every jump target, and — per scope slot — by any later
/// runtime scope-setter for that slot (see [`check_guard`]).
type BoolTaints = BTreeMap<i32, TaintedBool>;

/// Resolve a script's int `$arg` slots to entry values, when exactly
/// provable. Hook programs invoke `[scriptId, tail...]` against FRESH
/// zeroed locals, filling per type in listed order (`executeHookInner`):
/// so with EXACTLY ONE installer, NO `gosub` callers (inherited args would
/// be arbitrary), and a tail covering its args per type (int tails ≤ int
/// args, string tails ≤ object args — short tails leave fresh-array zeros,
/// overlong tails would crash and genuine hooks never carry them), every int
/// slot is known: ordinary ints literally, the owner placeholders by
/// derivation, runtime placeholders as unknown. Arg slots assigned anywhere
/// in the body go unknown (flow-insensitive, sound). Anything less yields
/// `None` — all args unknown.
fn resolve_int_args(
    script: &CompiledScript,
    installers: &[Installer],
    has_callers: bool,
) -> Option<IntArgValues> {
    let [installer] = installers else {
        return None;
    };
    if has_callers {
        return None;
    }
    let mut int_tails = 0_usize;
    let mut str_tails = 0_usize;
    for arg in &installer.tail {
        match arg {
            HookArg::Int(_) => int_tails += 1,
            HookArg::Str(_) => str_tails += 1,
        }
    }
    if int_tails > usize::from(script.args.int) || str_tails > usize::from(script.args.obj) {
        return None;
    }
    // Assigned int-arg slots go unknown: `$argN = ...` anywhere kills the
    // entry value for that slot.
    let mut assigned = BTreeSet::new();
    for instr in &script.code {
        if let ("pop_int_local", Operand::Local(slot)) = (instr.command.as_str(), &instr.operand)
            && *slot < i32::from(script.args.int)
        {
            assigned.insert(*slot);
        }
    }
    let (iface, file) = installer.comp;
    let owner_packed = pack_component(ComponentRef {
        iface,
        child: i32::try_from(file).ok()?,
    })?;
    let mut values = IntArgValues::new();
    let mut slot = 0_i32;
    for arg in &installer.tail {
        let HookArg::Int(value) = arg else {
            continue;
        };
        let resolved = if *value == HOOK_MAGIC_PARENTLAYER {
            Some(owner_packed)
        } else if *value == HOOK_MAGIC_COMPONENT_ID {
            Some(-1)
        } else if (HOOK_MAGIC_MIN..=HOOK_MAGIC_MAX).contains(value) || assigned.contains(&slot) {
            None
        } else {
            Some(*value)
        };
        values.insert(slot, resolved);
        slot += 1;
    }
    // Uncovered trailing arg slots stay fresh-array zero.
    while slot < i32::from(script.args.int) {
        values.insert(slot, Some(0));
        slot += 1;
    }
    // Reassignment invalidates owner placeholders and uncovered zero slots
    // too, not only ordinary literal arguments.
    for slot in assigned {
        values.insert(slot, None);
    }
    Some(values)
}

/// Recover the packed component id a table command at `code[pc]` reads:
/// walk the straight-line run backward, tracking how many int values sit
/// above the wanted slot, over exact int traffic only. Literal int pushes
/// resolve, as do `$arg` pushes with statically resolved entry values (see
/// [`resolve_int_args`]); `$local`/varbit/var/computed values, calls, jumps
/// into the run, control flow, and anything outside the exact set bail the
/// whole site (`None`). The client resolves the identical value, so an edge
/// built on this is exact, never heuristic.
#[must_use]
pub fn recover_component_operand(
    code: &[Instruction],
    pc: usize,
    slot: u8,
    int_args: &IntArgValues,
    local_vals: &LocalIntValues,
) -> Option<i32> {
    let targets = jump_targets(code);
    // The command itself must not be a merge point: a jump landing on it
    // runs it against an ambient stack the walk never saw.
    if i32::try_from(pc).is_ok_and(|at| targets.contains(&at)) {
        return None;
    }
    // Values still needed above and including the target: slot depth + 1.
    let mut depth = i32::from(slot) + 1;
    let mut index = pc;
    loop {
        index = index.checked_sub(1)?;
        let instr = &code[index];
        // Exact int traffic (pops, pushes) for this instruction; control
        // flow, calls, and anything unverified bail the walk.
        let (pops, pushes) = int_traffic(&instr.command, &instr.operand)?;
        if depth <= pushes {
            // The wanted slot is produced here — and a jump landing exactly
            // on this terminal push still executes it, so its targetedness
            // does not matter. Exact only for a single literal int push or
            // a resolved `$arg`/`$local` value; anything else bails.
            return match (&instr.command[..], &instr.operand) {
                ("push_constant_string" | "push_constant_int", Operand::Int(value))
                    if pushes == 1 =>
                {
                    Some(*value)
                }
                ("push_int_local", Operand::Local(arg)) if pushes == 1 => {
                    known_int(*arg, int_args, local_vals)
                }
                _ => None,
            };
        }
        // A merge strictly inside the run ends it: another path could arrive
        // with a different stack below.
        if i32::try_from(index).is_ok_and(|at| targets.contains(&at)) {
            return None;
        }
        depth += pops - pushes;
        if depth > MAX_WALK_DEPTH {
            return None;
        }
    }
}

/// Exact int-stack traffic `(pops, pushes)` for one instruction, or `None`
/// when the traffic is unknown or unknowable: unlisted pushes (`push_var`),
/// computed values, calls. String/long traffic is `(0, 0)` — separate stacks
/// — but only for commands whose full shape is verified, never assumed.
fn int_traffic(command: &str, operand: &Operand) -> Option<(i32, i32)> {
    match (command, operand) {
        ("push_constant_string" | "push_constant_int", Operand::Int(_)) => Some((0, 1)),
        ("push_constant_string", Operand::Str(_) | Operand::Long(_))
        | ("push_long_constant", _) => Some((0, 0)),
        ("push_int_local", Operand::Local(_)) => Some((0, 1)),
        ("push_string_local" | "push_long_local", Operand::Local(_)) => Some((0, 0)),
        ("push_varbit", Operand::VarBitRef(_)) => Some((0, 1)),
        ("pop_int_local", Operand::Local(_)) => Some((1, 0)),
        ("pop_string_local" | "pop_long_local", Operand::Local(_)) => Some((0, 0)),
        _ => match effect(command, operand) {
            Effect::Fixed { pops, pushes } => Some((i32::from(pops[0]), i32::from(pushes[0]))),
            // Control flow, calls, and anything unverified end the run:
            // stepping over them would misattribute the stack below.
            Effect::Unknown
            | Effect::Trap
            | Effect::Call
            | Effect::ControlFlow { .. }
            | Effect::Conditional { .. } => None,
        },
    }
}

/// Exact object-stack traffic `(pops, pushes)` for one instruction: the
/// object-lane mirror of [`int_traffic`], with the same bail rules. Int/long
/// traffic is `(0, 0)` — separate stacks — only for verified shapes.
fn obj_traffic(command: &str, operand: &Operand) -> Option<(i32, i32)> {
    match (command, operand) {
        ("push_constant_string", Operand::Str(_)) => Some((0, 1)),
        ("push_string_local", Operand::Local(_)) => Some((0, 1)),
        ("pop_string_local", Operand::Local(_)) => Some((1, 0)),
        ("push_constant_string", Operand::Int(_) | Operand::Long(_))
        | ("push_long_constant", _) => Some((0, 0)),
        ("push_int_local" | "push_long_local", Operand::Local(_)) => Some((0, 0)),
        ("pop_int_local" | "pop_long_local", Operand::Local(_)) => Some((0, 0)),
        ("push_varbit", Operand::VarBitRef(_)) => Some((0, 0)),
        _ => match effect(command, operand) {
            Effect::Fixed { pops, pushes } => Some((i32::from(pops[1]), i32::from(pushes[1]))),
            // Control flow, calls, and anything unverified end the run, as
            // in the int lane.
            Effect::Unknown
            | Effect::Trap
            | Effect::Call
            | Effect::ControlFlow { .. }
            | Effect::Conditional { .. } => None,
        },
    }
}

/// Recover the hook-descriptor string an `if_seton*` command at `code[pc]`
/// reads: the object-lane mirror of [`recover_component_operand`] over the
/// same straight-line discipline (no merge inside the run, command itself
/// untargeted, exact traffic only). The terminal must be a literal string
/// push — `$arg`/`$local` descriptors stay out (zero corpus uses outside
/// literals and call-blocked sites, so literals are the exact subset).
/// `None` bails the whole setter site.
#[must_use]
pub fn recover_string_operand(code: &[Instruction], pc: usize, slot: u8) -> Option<String> {
    let targets = jump_targets(code);
    if i32::try_from(pc).is_ok_and(|at| targets.contains(&at)) {
        return None;
    }
    let mut depth = i32::from(slot) + 1;
    let mut index = pc;
    loop {
        index = index.checked_sub(1)?;
        let instr = &code[index];
        let (pops, pushes) = obj_traffic(&instr.command, &instr.operand)?;
        if depth <= pushes {
            return match (&instr.command[..], &instr.operand) {
                ("push_constant_string", Operand::Str(value)) if pushes == 1 => Some(value.clone()),
                _ => None,
            };
        }
        if i32::try_from(index).is_ok_and(|at| targets.contains(&at)) {
            return None;
        }
        depth += pops - pushes;
        if depth > MAX_WALK_DEPTH {
            return None;
        }
    }
}

/// Int-stack depth of the installed script id below the top for an
/// `if_seton*` site carrying this hook-descriptor literal: the component id
/// is on top (slot 0, popped first by every wrapper), then the optional
/// transmit int-array, then one int per non-`s`/non-`l` descriptor char
/// (the hook-argument reader pops an int for every char but `s` and `l`),
/// then the script id it pops last. A trailing `Y` bails (`None`): the int-array count is a
/// runtime value, so the depth is statically unknowable.
#[must_use]
pub fn hook_script_depth(descriptor: &str) -> Option<i32> {
    if descriptor.ends_with('Y') {
        return None;
    }
    let mut tails = 0_i32;
    for char in descriptor.chars() {
        if char != 's' && char != 'l' {
            tails += 1;
        }
    }
    Some(1 + tails)
}

/// One script→component touch: script `script` reads or writes `target` at
/// `pc` via `command` ("touches" covers getters and setters alike — the edge
/// is evidence of a reference, not a direction).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptUse {
    /// Calling script id (pack group).
    pub script: i32,
    /// Instruction index of the component-taking command.
    pub pc: i32,
    /// Command word.
    pub command: String,
    /// Referenced component.
    pub target: ComponentRef,
}

/// One component→script touch: hook slot `slot` of component
/// (`iface`, `child`) installs script `script`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookUse {
    /// Owning interface id.
    pub iface: i32,
    /// Owning child (pack file) index.
    pub child: u32,
    /// Hook slot name (`onload`, `onclick`, …).
    pub slot: &'static str,
    /// Installed script id.
    pub script: i32,
}

/// One script→script hook installation: script `caller` runs an `if_seton*`
/// setter at `pc` that installs script `callee` as a hook program on
/// component `target` (see [`IF_HOOK_SETTERS`]). Both ends are rostered —
/// anything else records as [`DanglingUse`] in [`Xref::hook_dangling`],
/// never here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookSet {
    /// Installing script id (pack group).
    pub caller: i32,
    /// Instruction index of the `if_seton*` command.
    pub pc: i32,
    /// Command word (`if_setontimer`, …).
    pub command: String,
    /// Installed script id (pack group).
    pub callee: i32,
    /// Component the hook is installed on.
    pub target: ComponentRef,
}

/// A well-formed packed id that names no `(interface, child)` pair present
/// in the pack: a dead reference (or a classification bug — the corpus gate
/// pins the count, so drift shows up).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DanglingUse {
    /// Referencing script id.
    pub script: i32,
    /// Instruction index of the reference.
    pub pc: i32,
    /// Command word.
    pub command: String,
    /// The unresolvable packed value.
    pub value: i32,
}

/// A runtime-child touch identified by its creator instruction, never confused
/// with the packed parent component or a persistent cache child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DynamicUse {
    pub script: i32,
    pub pc: i32,
    pub command: String,
    pub creator_pc: i32,
    pub parent: Option<ComponentRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DynamicHookSet {
    pub caller: i32,
    pub pc: i32,
    pub command: String,
    pub callee: i32,
    pub creator_pc: i32,
    pub parent: Option<ComponentRef>,
}

/// A site whose target has not been proven. Keeping sites lets agents inspect
/// the missing evidence instead of treating an absent edge as absent behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnresolvedUse {
    pub script: i32,
    pub pc: i32,
    pub command: String,
    pub reason: &'static str,
}

/// Build counters, for gates and reports.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct XrefStats {
    pub active_hook_sites: usize,
    pub active_hook_linked: usize,
    pub active_hook_dangling: usize,
    pub active_hook_bails: usize,
    /// Scripts scanned.
    pub scripts: usize,
    /// Components scanned.
    pub components: usize,
    /// Table-command sites visited.
    pub sites: usize,
    /// Resolved script→component edges.
    pub linked: usize,
    /// Sites where recovery bailed (non-literal traffic).
    pub bails: usize,
    /// Dangling well-formed references.
    pub dangling: usize,
    /// Hook-head ints seen.
    pub hook_heads: usize,
    /// Hook heads resolving to pack scripts.
    pub hook_resolved: usize,
    /// Hook heads naming no pack script.
    pub hook_unresolved: usize,
    /// Scripts with statically resolved int entry values.
    pub arg_scripts: usize,
    /// `cc_` scope-touching sites visited.
    pub cc_sites: usize,
    /// `cc_` sites resolved against pack scope.
    pub cc_linked: usize,
    /// `cc_` sites under dynamic (created) scope.
    pub cc_dynamic: usize,
    /// `cc_` sites bailed (unknown scope or unselectable slot).
    pub cc_bails: usize,
    /// Find sites (`cc_find`/`if_find`) visited.
    pub finds: usize,
    /// Finds resolved to pack scope.
    pub finds_pack: usize,
    /// `if_seton*` hook-setter sites visited.
    pub set_sites: usize,
    /// Setter sites resolved to script→script hook edges.
    pub set_linked: usize,
    /// Setter sites whose script id named no pack script (dead hooks,
    /// including `-1` clears).
    pub set_dangling: usize,
    /// Setter sites bailed (unrecoverable component, descriptor, or script
    /// id — or an unrostered component, already counted as dangling).
    pub set_bails: usize,
}

/// The whole index: script→component touches, component→script hook
/// installations, runtime creations, dangling references, and counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Xref {
    /// Scripts whose argument refinement assumes their unique cached hook
    /// entry. Other runtime installations or trigger entries can supply
    /// different arguments; consumers must retain this context condition.
    pub entry_contexts: BTreeMap<i32, ComponentRef>,
    pub dynamic_hook_sets: BTreeMap<i32, Vec<DynamicHookSet>>,
    pub dynamic_touches: BTreeMap<i32, Vec<DynamicUse>>,
    pub unresolved: Vec<UnresolvedUse>,
    /// Per script: every resolved component touch, in pc order (`if_` reads
    /// and in-scope `cc_` touches alike — the command word tells them apart).
    pub script_to_comps: BTreeMap<i32, Vec<ScriptUse>>,
    /// Per `(interface, child)`: every hook installation, in slot order.
    pub comp_to_scripts: BTreeMap<(i32, u32), Vec<HookUse>>,
    /// Per script: every `cc_create` under a roster-valid pack parent.
    pub creations: BTreeMap<i32, Vec<CreationUse>>,
    /// Per installing script: every resolved `if_seton*` hook installation,
    /// in pc order.
    pub hook_sets: BTreeMap<i32, Vec<HookSet>>,
    /// Installed script ids that name no pack script (dead hooks and `-1`
    /// clears alike — the client silently no-ops them, so they are evidence,
    /// never drops). Reuses [`DanglingUse`] with the caller as `script`, the
    /// setter as `command`, and the unresolvable id as `value`.
    pub hook_dangling: Vec<DanglingUse>,
    /// Well-formed but unresolvable references, in scan order.
    pub dangling: Vec<DanglingUse>,
    /// Build counters.
    pub stats: XrefStats,
}

/// Decoded scripts keyed by pack group id.
pub type ScriptMap = BTreeMap<i32, CompiledScript>;
/// Decoded components keyed by `(interface, file)`.
pub type ComponentMap = BTreeMap<(i32, u32), InterfaceComponent>;

/// Build the index over decoded scripts (`id → script`) and components
/// (`(interface, file) → component`). Total: anything unprovable is bailed
/// or dangling, never an edge.
#[must_use]
/// One tracked scope slot: unknown at entry, after calls, and at merge
/// points; a pack component after a provable find; or a runtime-created
/// component (which has no pack address) after `cc_create`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scope {
    /// Nothing provable.
    Unknown,
    /// A roster-validated pack component.
    Pack(ComponentRef),
    /// A `cc_create`d runtime component: linkable only through its pack
    /// parent, recorded as a creation edge.
    Dynamic {
        creator_pc: i32,
        /// Pack parent the child was created under, when its id recovered
        /// literally and roster-validated.
        parent: Option<ComponentRef>,
    },
}

/// Whether a `cc_` command reads the ambient active component: every `cc_`
/// word except the scope setters (`cc_create`, `cc_find`), the pack-reading
/// `cc_deleteall` (covered by the component table — it never touches scope),
/// and the eleven swipe/pinch/`subtractinsets` no-op stubs (bare pop
/// bodies that touch nothing at all). The remaining 173 wrappers were each
/// verified active-component-first; the boolean-dispatched rows `cc_sendtofront`/`cc_sendtoback` share
/// `cc_sendto`'s proven body.
#[must_use]
pub fn cc_touches_active(command: &str) -> bool {
    command.starts_with("cc_")
        && !matches!(
            command,
            "cc_create"
                | "cc_find"
                | "cc_deleteall"
                | "cc_setswipeunknown2"
                | "cc_setswipedeadtime"
                | "cc_setswipedeadzone"
                | "cc_setswipeflags"
                | "cc_addswipeflags"
                | "cc_delswipeflags"
                | "cc_setpinchflags"
                | "cc_addpinchflags"
                | "cc_delpinchflags"
                | "cc_setpinchdeadzone"
                | "cc_setsubtractinsets"
        )
}

/// Which scope slot a scope-touching site selects. The interpreter sets
/// `secondary` per instruction from its generic operand (1 selects the
/// secondary slot), so `Byte(1)`/`Int(1)` address the
/// secondary slot and `Byte(0)`/`Int(0)` the primary — uniformly, for every
/// command. Anything else cannot select a slot and the site bails.
fn scope_slot(operand: &Operand) -> Option<usize> {
    match operand {
        Operand::Byte(0) | Operand::Int(0) => Some(0),
        Operand::Byte(1) | Operand::Int(1) => Some(1),
        _ => None,
    }
}

/// One script→parent creation: script `script` runs `cc_create` at `pc`,
/// building a pack-parented runtime child (`index`/`child_type` ride along
/// when literal — informational only, never edges).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreationUse {
    /// Creating script id.
    pub script: i32,
    /// Instruction index of the `cc_create`.
    pub pc: i32,
    /// Pack parent the child is created under.
    pub parent: ComponentRef,
    /// Child index operand, when literal.
    pub index: Option<i32>,
    /// Component type operand, when literal.
    pub child_type: Option<i32>,
}

/// One instruction under scope simulation: everything the tracker needs
/// beyond its mutable accumulator, bundled to stay under the
/// argument-count lint.
#[derive(Clone, Copy)]
struct ScopeSite<'a> {
    /// Owning script id.
    script: i32,
    /// Instruction index.
    position: usize,
    /// The instruction itself.
    instr: &'a Instruction,
    /// Whole decoded stream (for operand recovery).
    code: &'a [Instruction],
    /// Component roster.
    components: &'a ComponentMap,
    /// Resolved int entry values for `$arg` pushes.
    int_args: &'a IntArgValues,
    /// Known int `$local` values for copies in this segment.
    local_vals: &'a LocalIntValues,
    /// Jump targets (for straight-line checks).
    targets: &'a BTreeSet<i32>,
}

/// Advance both scope slots over one instruction, recording `cc_` touches
/// against pack scope and creations against literal parents. Plain `cc_`
/// commands never change scope; setters overwrite exactly one slot (or both
/// go unknown when the slot is unselectable); calls make both unknown.
/// Everything unprovable bails to `Unknown` — fewer edges, never wrong ones.
/// Resolve what a find site installs: the pack scope for a `-1`/self find
/// against a roster-validated parent, else `None`. `get(packed, -1)` returns
/// the component itself, so with
/// a roster-validated parent the find cannot fail and scope is
/// deterministic; any other subindex is runtime state. Shared by the setter
/// itself and guard detection so both agree on what "found" means.
fn resolve_find_pack(
    code: &[Instruction],
    position: usize,
    is_if_find: bool,
    int_args: &IntArgValues,
    local_vals: &LocalIntValues,
    components: &ComponentMap,
) -> Option<ComponentRef> {
    // cc_find requires a dynamic child; -1 short-circuits to false.
    if !is_if_find {
        return None;
    }
    let (packed, sub) = if is_if_find {
        (
            recover_component_operand(code, position, 0, int_args, local_vals),
            Some(-1),
        )
    } else {
        (
            recover_component_operand(code, position, 1, int_args, local_vals),
            recover_component_operand(code, position, 0, int_args, local_vals),
        )
    };
    match (packed.and_then(unpack_component), sub) {
        (Some(parent), Some(-1))
            if components.contains_key(&(
                parent.iface,
                u32::try_from(parent.child).unwrap_or(u32::MAX),
            )) =>
        {
            Some(parent)
        }
        _ => None,
    }
}

/// Success-target claims from find-tested branches: target pc → the pack
/// scope the success path carries, plus which slot it lands in. Filled by
/// [`check_guard`] as branches are processed, consumed when the loop reaches
/// the target label.
type GuardClaims = BTreeMap<i32, (usize, ComponentRef)>;

/// Detect find-guard branches and their fall-through implications.
///
/// Guard shapes, each with the tested boolean provably a find result
/// (adjacent, untargeted branch/push — anything else returns without
/// touching scope):
///
/// * `find; push 1; branch_equals(T)` (either operand order): `T` is the
///   success path, the fall-through is the failure path (scope was nulled).
/// * `find; branch_if_true(T)`: same, without the literal.
/// * `push $l; push 1; branch_equals(T)` (either order) and `push $l;
///   branch_if_true(T)` with `$l` carrying find-boolean taint (see
///   [`BoolTaints`]): the pushed value IS the recorded find's result, so the
///   same failure-nulling applies.
///
/// A tainted push stands in for the ADJACENT find only — never for the
/// literal `push 1`: comparing a live find against an older tainted result
/// proves nothing about either, so `(find, push $t)` pairs stay out.
///
/// On a match the fall-through scope resets to unknown (failure nulled it),
/// and when the find resolved pack scope AND `T` has exactly one incoming
/// branch edge AND no fall-through can reach it (the previous index holds an
/// unconditional `branch` or `return`), the success scope is claimed for `T`.
/// Tainted claims reuse the slot and parent captured at taint time: the
/// segment values may have drifted since the find, so re-resolving at guard
/// time would be unsound, while the capture is exact (nothing runs between
/// an adjacent find and its pop). The claim stays sound because any later
/// runtime scope-setter for the tainted slot clears the taint first (see
/// [`track_scope`]) — and branches themselves are not runtime setters, so
/// the guard reset deliberately leaves taints alone. Every other comparison
/// (`branch_if_false`, `branch_not_equals`, …) needs nothing: failure targets
/// reset by the label rule and success fall-throughs keep scope already.
fn check_guard(
    site: ScopeSite<'_>,
    scope: &mut [Scope; 2],
    guards: &mut GuardClaims,
    refcounts: &BTreeMap<i32, usize>,
    taints: &BoolTaints,
) {
    let code = site.code;
    let position = site.position;
    let at = |back: usize| position.checked_sub(back).and_then(|at| code.get(at));
    let untargeted =
        |at: usize| !i32::try_from(at).is_ok_and(|index| site.targets.contains(&index));
    let is_push_one = |instr: &Instruction| {
        matches!(
            (&instr.command[..], &instr.operand),
            (
                "push_constant_string" | "push_constant_int",
                Operand::Int(1)
            )
        )
    };
    let is_find = |instr: &Instruction| matches!(instr.command.as_str(), "cc_find" | "if_find");
    // A tainted `$local` push, with the find facts captured at taint time.
    let tainted = |instr: &Instruction| match (&instr.command[..], &instr.operand) {
        ("push_int_local", Operand::Local(slot)) => taints.get(slot).copied(),
        _ => None,
    };
    // The tested find facts: scope slot plus resolved pack parent. Adjacent
    // finds re-resolve here (exact — nothing runs between the find and the
    // branch but the literal push); tainted pushes reuse their capture.
    enum Tested {
        /// Adjacent find at this index.
        Find(usize),
        /// Tainted push carrying this capture.
        Tainted(TaintedBool),
    }
    let tested: Option<Tested> = match site.instr.command.as_str() {
        "branch_equals" if position >= 2 && untargeted(position) && untargeted(position - 1) => {
            let (first, second) = (at(2), at(1));
            match (first, second) {
                (Some(find), Some(push)) if is_find(find) && is_push_one(push) => {
                    Some(Tested::Find(position - 2))
                }
                (Some(push), Some(find)) if is_push_one(push) && is_find(find) => {
                    Some(Tested::Find(position - 1))
                }
                (Some(push), Some(one)) if is_push_one(one) => tainted(push).map(Tested::Tainted),
                (Some(one), Some(push)) if is_push_one(one) => tainted(push).map(Tested::Tainted),
                _ => None,
            }
        }
        "branch_if_true" if position >= 1 && untargeted(position) && untargeted(position - 1) => {
            match at(1) {
                Some(find) if is_find(find) => Some(Tested::Find(position - 1)),
                Some(push) => tainted(push).map(Tested::Tainted),
                None => None,
            }
        }
        _ => None,
    };
    let Some(tested) = tested else {
        return;
    };
    // Fall-through means the test failed means the find nulled scope.
    *scope = [Scope::Unknown, Scope::Unknown];
    let Operand::Branch(target) = &site.instr.operand else {
        return;
    };
    let (slot, parent) = match tested {
        Tested::Find(find_at) => {
            let find = &code[find_at];
            let Some(slot) = scope_slot(&find.operand) else {
                return;
            };
            let Some(parent) = resolve_find_pack(
                code,
                find_at,
                find.command == "if_find",
                site.int_args,
                site.local_vals,
                site.components,
            ) else {
                return;
            };
            (slot, parent)
        }
        Tested::Tainted(tainted) => {
            let (Some(slot), Some(parent)) = (tainted.slot, tainted.parent) else {
                return;
            };
            (slot, parent)
        }
    };
    // Single-predecessor, no fall-through: the only arrival is the success
    // edge, so the claim is exact.
    let single = refcounts.get(target).copied().unwrap_or(0) == 1;
    let no_fallthrough = target
        .checked_sub(1)
        .and_then(|at| code.get(usize::try_from(at).unwrap_or(usize::MAX)))
        .is_some_and(|previous| matches!(previous.command.as_str(), "branch" | "return"));
    if single && (*target == 0 || no_fallthrough) {
        guards.insert(*target, (slot, parent));
    }
}

fn track_scope(
    site: ScopeSite<'_>,
    scope: &mut [Scope; 2],
    xref: &mut Xref,
    taints: &mut BoolTaints,
) {
    let id = site.script;
    let position = site.position;
    let instr = site.instr;
    let code = site.code;
    let components = site.components;
    let int_args = site.int_args;
    let local_vals = site.local_vals;
    let pc = i32::try_from(position).unwrap_or(i32::MAX);
    // Calls execute foreign bodies against the same state: whatever scope
    // returns is unknowable.
    if instr.command == "gosub_with_params" {
        *scope = [Scope::Unknown, Scope::Unknown];
        taints.clear();
        return;
    }
    let rostered = |target: ComponentRef| {
        components.contains_key(&(
            target.iface,
            u32::try_from(target.child).unwrap_or(u32::MAX),
        ))
    };
    // A runtime write to scope slot `slot` disturbs every taint derived from
    // an older write to that slot (a later guard claim through the taint
    // would name the older component); a write to no provable slot disturbs
    // everything, including slot-less taints.
    let disturb = |taints: &mut BoolTaints, slot: Option<usize>| match slot {
        Some(slot) => taints.retain(|_, tainted| tainted.slot != Some(slot)),
        None => taints.clear(),
    };
    match instr.command.as_str() {
        // `cc_create(parent, type, index)`: the bottom operand is the pack
        // parent, the top two the child index and type. The client installs
        // the fresh component as active.
        "cc_create" => {
            let Some(slot) = scope_slot(&instr.operand) else {
                *scope = [Scope::Unknown, Scope::Unknown];
                taints.clear();
                return;
            };
            disturb(taints, Some(slot));
            let parent = recover_component_operand(code, position, 2, int_args, local_vals)
                .and_then(unpack_component);
            match parent {
                Some(parent) if rostered(parent) => {
                    scope[slot] = Scope::Dynamic {
                        creator_pc: pc,
                        parent: Some(parent),
                    };
                    xref.creations.entry(id).or_default().push(CreationUse {
                        script: id,
                        pc,
                        parent,
                        index: recover_component_operand(code, position, 0, int_args, local_vals),
                        child_type: recover_component_operand(
                            code, position, 1, int_args, local_vals,
                        ),
                    });
                }
                _ => {
                    scope[slot] = Scope::Dynamic {
                        creator_pc: pc,
                        parent: None,
                    }
                }
            }
        }

        "cc_find" | "if_find" => {
            let Some(slot) = scope_slot(&instr.operand) else {
                *scope = [Scope::Unknown, Scope::Unknown];
                taints.clear();
                return;
            };
            xref.stats.finds += 1;
            if instr.command == "cc_find"
                && recover_component_operand(code, position, 0, int_args, local_vals) == Some(-1)
            {
                // No scope update here: active scope and existing taints are unchanged.
                return;
            }
            disturb(taints, Some(slot));
            match resolve_find_pack(
                code,
                position,
                instr.command == "if_find",
                int_args,
                local_vals,
                components,
            ) {
                Some(parent) => {
                    xref.stats.finds_pack += 1;
                    scope[slot] = Scope::Pack(parent);
                }
                _ => scope[slot] = Scope::Unknown,
            }
        }
        command if cc_touches_active(command) => {
            let Some(slot) = scope_slot(&instr.operand) else {
                xref.stats.cc_bails += 1;
                xref.unresolved.push(UnresolvedUse {
                    script: id,
                    pc,
                    command: instr.command.clone(),
                    reason: "active component slot is not proven",
                });
                return;
            };
            xref.stats.cc_sites += 1;
            match scope[slot] {
                Scope::Pack(target) => {
                    xref.stats.cc_linked += 1;
                    xref.script_to_comps.entry(id).or_default().push(ScriptUse {
                        script: id,
                        pc,
                        command: instr.command.clone(),
                        target,
                    });
                }
                Scope::Dynamic { creator_pc, parent } => {
                    xref.stats.cc_dynamic += 1;
                    xref.dynamic_touches
                        .entry(id)
                        .or_default()
                        .push(DynamicUse {
                            script: id,
                            pc,
                            command: instr.command.clone(),
                            creator_pc,
                            parent,
                        });
                }
                Scope::Unknown => {
                    xref.stats.cc_bails += 1;
                    xref.unresolved.push(UnresolvedUse {
                        script: id,
                        pc,
                        command: instr.command.clone(),
                        reason: "active component identity is not proven",
                    });
                }
            }
        }
        _ => {}
    }
}

pub fn build_xref(scripts: &ScriptMap, components: &ComponentMap) -> Xref {
    build_xref_inner(scripts, components, None, None)
}

/// Add CFG-proven component operands, including agreeing branches and computed IDs.
/// Partial/failed analyses never supply facts to the cross-reference registry.
pub fn build_xref_with_configs(
    scripts: &ScriptMap,
    components: &ComponentMap,
    configs: &crate::config::ConfigTypes,
) -> Xref {
    build_xref_with_summaries(
        scripts,
        components,
        configs,
        &crate::dataflow::infer_summaries(scripts, configs),
    )
}

pub fn build_xref_with_summaries(
    scripts: &ScriptMap,
    components: &ComponentMap,
    configs: &crate::config::ConfigTypes,
    summaries: &crate::dataflow::Summaries,
) -> Xref {
    build_xref_inner(scripts, components, Some(configs), Some(summaries))
}

/// The script ID is the deepest int consumed by a verified hook setter.
/// Using the shared contract also handles transmit arrays, including the
/// zero-count case where the descriptor retains its trailing `Y` argument.
fn flow_hook_installation(
    instruction: &Instruction,
    state: &crate::dataflow::State,
    configs: &crate::config::ConfigTypes,
) -> Option<(ComponentRef, i32)> {
    const TOP_STACK_OFFSET: usize = 0;
    let component = unpack_component(state.int_from_top(TOP_STACK_OFFSET)?)?;
    Some((component, flow_hook_script(instruction, state, configs)?))
}

fn flow_hook_script(
    instruction: &Instruction,
    state: &crate::dataflow::State,
    configs: &crate::config::ConfigTypes,
) -> Option<i32> {
    const SCRIPT_ID_WIDTH: u16 = 1;
    let Effect::Fixed { pops, .. } = crate::dataflow::resolved_effect(instruction, state, configs)
    else {
        return None;
    };
    let script_depth = usize::from(pops.first()?.checked_sub(SCRIPT_ID_WIDTH)?);
    state.int_from_top(script_depth)
}

fn record_active_hook(
    site: ScopeSite<'_>,
    scopes: &[Scope; 2],
    scripts: &ScriptMap,
    flow_script: Option<i32>,
    xref: &mut Xref,
) {
    const EXPLICIT_COMPONENT_WIDTH: i32 = 1;
    let Some(suffix) = site.instr.command.strip_prefix("cc_") else {
        return;
    };
    if !hook_setter(&format!("if_{suffix}")) {
        return;
    }
    xref.stats.active_hook_sites += 1;
    let pc = i32::try_from(site.position).unwrap_or(i32::MAX);
    let descriptor = recover_string_operand(site.code, site.position, u8::default());
    let installed = descriptor
        .as_deref()
        .and_then(hook_script_depth)
        .and_then(|depth| depth.checked_sub(EXPLICIT_COMPONENT_WIDTH))
        .and_then(|depth| depth.try_into().ok())
        .and_then(|depth| {
            recover_component_operand(
                site.code,
                site.position,
                depth,
                site.int_args,
                site.local_vals,
            )
        })
        .or(flow_script);
    let selected = scope_slot(&site.instr.operand).map(|slot| scopes[slot]);
    match (selected, installed) {
        (Some(Scope::Pack(target)), Some(callee)) if scripts.contains_key(&callee) => {
            xref.stats.active_hook_linked += 1;
            xref.hook_sets
                .entry(site.script)
                .or_default()
                .push(HookSet {
                    caller: site.script,
                    pc,
                    command: site.instr.command.clone(),
                    callee,
                    target,
                });
        }
        (Some(Scope::Dynamic { creator_pc, parent }), Some(callee))
            if scripts.contains_key(&callee) =>
        {
            xref.stats.active_hook_linked += 1;
            xref.dynamic_hook_sets
                .entry(site.script)
                .or_default()
                .push(DynamicHookSet {
                    caller: site.script,
                    pc,
                    command: site.instr.command.clone(),
                    callee,
                    creator_pc,
                    parent,
                });
        }
        (Some(Scope::Pack(_) | Scope::Dynamic { .. }), Some(value)) => {
            xref.stats.active_hook_dangling += 1;
            xref.hook_dangling.push(DanglingUse {
                script: site.script,
                pc,
                command: site.instr.command.clone(),
                value,
            });
        }
        _ => {
            xref.stats.active_hook_bails += 1;
            xref.unresolved.push(UnresolvedUse {
                script: site.script,
                pc,
                command: site.instr.command.clone(),
                reason: "active hook component, descriptor or script is not proven",
            });
        }
    }
}

fn build_xref_inner(
    scripts: &ScriptMap,
    components: &ComponentMap,
    configs: Option<&crate::config::ConfigTypes>,
    summaries: Option<&crate::dataflow::Summaries>,
) -> Xref {
    let mut xref = Xref::default();
    xref.stats.scripts = scripts.len();
    xref.stats.components = components.len();
    // Pre-pass 1: hook installations per script (for `$arg` entry values)
    // plus hook-use recording.
    let mut installers: BTreeMap<i32, Vec<Installer>> = BTreeMap::new();
    for ((iface, child), component) in components {
        for (slot, hook) in hook_slots(&component.hooks) {
            let Some(args) = hook else { continue };
            let Some(HookArg::Int(head)) = args.first() else {
                continue;
            };
            xref.stats.hook_heads += 1;
            if scripts.contains_key(head) {
                xref.stats.hook_resolved += 1;
                xref.comp_to_scripts
                    .entry((*iface, *child))
                    .or_default()
                    .push(HookUse {
                        iface: *iface,
                        child: *child,
                        slot,
                        script: *head,
                    });
                installers.entry(*head).or_default().push(Installer {
                    comp: (*iface, *child),
                    tail: args[1..].to_vec(),
                });
            } else {
                xref.stats.hook_unresolved += 1;
            }
        }
    }
    // Pre-pass 2: `gosub` callers per script — a called script's entry args
    // are inherited and unknowable.
    let mut called = BTreeSet::new();
    for script in scripts.values() {
        for instr in &script.code {
            if instr.command == "gosub_with_params"
                && let Operand::Script(callee) = &instr.operand
            {
                called.insert(*callee);
            }
        }
    }
    for (id, script) in scripts {
        let targets = jump_targets(&script.code);
        // Both scope slots start unknown (entry scope is unprovable from
        // available sources: no hook-dispatch assignment exists in the
        // client slice, and gosub callers inherit opaquely). `$arg` entry
        // VALUES resolve separately (see `resolve_int_args`) — dataflow
        // needs no dispatcher proof.
        let mut scope = [Scope::Unknown, Scope::Unknown];
        let int_args = resolve_int_args(
            script,
            installers.get(id).map_or(&[], Vec::as_slice),
            called.contains(id),
        );
        if int_args.is_some() {
            xref.stats.arg_scripts += 1;
            let installer = &installers[id][0];
            xref.entry_contexts.insert(
                *id,
                ComponentRef {
                    iface: installer.comp.0,
                    child: installer.comp.1.try_into().expect("rostered child ID"),
                },
            );
        }
        let int_args = int_args.unwrap_or_default();
        let flow = configs.zip(summaries).and_then(|(types, summaries)| {
            let arguments = [
                (0..script.args.int)
                    .map(|slot| {
                        int_args
                            .get(&i32::from(slot))
                            .copied()
                            .flatten()
                            .map(crate::dataflow::Constant::Int)
                    })
                    .collect(),
                Vec::new(),
                Vec::new(),
            ];
            let analysis = crate::dataflow::analyze_in_context(
                script,
                scripts,
                &summaries.0,
                &summaries.1,
                types,
                &arguments,
            );
            analysis.failure.is_none().then_some(analysis)
        });
        // Known int `$local` copies in the current straight-line segment.
        let mut local_vals: LocalIntValues = BTreeMap::new();
        // Find-boolean `$local` taints in the current segment (see
        // `BoolTaints`).
        let mut taints: BoolTaints = BTreeMap::new();
        let refcounts = target_refcounts(&script.code);
        // Success-target scope claims from find guards (see `check_guard`).
        let mut guards: GuardClaims = BTreeMap::new();
        for (position, instr) in script.code.iter().enumerate() {
            let pc = i32::try_from(position).unwrap_or(i32::MAX);
            let targeted = i32::try_from(position).is_ok_and(|at| targets.contains(&at));
            if targeted {
                // Merge point: paths rejoin with unknowable scope and values —
                // unless a find guard claims this exact target for its
                // success path (single-predecessor, no fall-through).
                scope = [Scope::Unknown, Scope::Unknown];
                local_vals.clear();
                taints.clear();
                if let Some(target) = i32::try_from(position)
                    .ok()
                    .and_then(|at| guards.remove(&at))
                {
                    scope[target.0] = Scope::Pack(target.1);
                }
            }
            if let Some(slot) = component_slot(&instr.command) {
                xref.stats.sites += 1;
                let recovered =
                    recover_component_operand(&script.code, position, slot, &int_args, &local_vals)
                        .or_else(|| {
                            flow.as_ref()?
                                .before
                                .get(position)?
                                .as_ref()?
                                .int_from_top(usize::from(slot))
                        })
                        .and_then(unpack_component);
                match recovered {
                    None => {
                        xref.stats.bails += 1;
                        xref.unresolved.push(UnresolvedUse {
                            script: *id,
                            pc,
                            command: instr.command.clone(),
                            reason: "packed component operand is not proven",
                        });
                    }
                    Some(target) => {
                        let key = (
                            target.iface,
                            u32::try_from(target.child).unwrap_or(u32::MAX),
                        );
                        if components.contains_key(&key) {
                            xref.stats.linked += 1;
                            xref.script_to_comps
                                .entry(*id)
                                .or_default()
                                .push(ScriptUse {
                                    script: *id,
                                    pc,
                                    command: instr.command.clone(),
                                    target,
                                });
                        } else {
                            xref.stats.dangling += 1;
                            xref.dangling.push(DanglingUse {
                                script: *id,
                                pc,
                                command: instr.command.clone(),
                                value: pack_component(target).unwrap_or(-1),
                            });
                        }
                    }
                }
            }
            let site = ScopeSite {
                script: *id,
                position,
                instr,
                code: &script.code,
                components,
                int_args: &int_args,
                local_vals: &local_vals,
                targets: &targets,
            };
            track_scope(site, &mut scope, &mut xref, &mut taints);
            check_guard(site, &mut scope, &mut guards, &refcounts, &taints);
            let flow_script = configs.zip(flow.as_ref()).and_then(|(types, analysis)| {
                crate::semantics::is_hook_setter(&instr.command).then_some(())?;
                flow_hook_script(instr, analysis.before.get(position)?.as_ref()?, types)
            });
            record_active_hook(site, &scope, scripts, flow_script, &mut xref);
            if hook_setter(&instr.command) {
                // Hook installation: the component resolves through the
                // proven slot-0 walk (additionally recorded as a component
                // touch above), the descriptor through the object-lane walk,
                // and the installed script id through the int walk at the
                // descriptor-derived depth. Every side must prove out —
                // anything else bails the site, never an edge.
                xref.stats.set_sites += 1;
                let flow_installation = configs.zip(flow.as_ref()).and_then(|(types, analysis)| {
                    flow_hook_installation(instr, analysis.before.get(position)?.as_ref()?, types)
                });
                let target =
                    recover_component_operand(&script.code, position, 0, &int_args, &local_vals)
                        .and_then(unpack_component)
                        .or_else(|| flow_installation.map(|(target, _)| target))
                        .filter(|target| {
                            components.contains_key(&(
                                target.iface,
                                u32::try_from(target.child).unwrap_or(u32::MAX),
                            ))
                        });
                let descriptor = recover_string_operand(&script.code, position, 0);
                let installed = descriptor
                    .as_deref()
                    .and_then(hook_script_depth)
                    .and_then(|depth| u8::try_from(depth).ok())
                    .and_then(|slot| {
                        recover_component_operand(
                            &script.code,
                            position,
                            slot,
                            &int_args,
                            &local_vals,
                        )
                    })
                    .or_else(|| flow_installation.map(|(_, installed)| installed));
                match (target, installed) {
                    (Some(target), Some(installed)) if scripts.contains_key(&installed) => {
                        xref.stats.set_linked += 1;
                        xref.hook_sets.entry(*id).or_default().push(HookSet {
                            caller: *id,
                            pc,
                            command: instr.command.clone(),
                            callee: installed,
                            target,
                        });
                    }
                    (Some(_), Some(installed)) => {
                        // A well-formed script id naming no pack script: a
                        // dead hook (the client silently no-ops it), recorded,
                        // never dropped. `-1` hook clears dominate here.
                        xref.stats.set_dangling += 1;
                        xref.hook_dangling.push(DanglingUse {
                            script: *id,
                            pc,
                            command: instr.command.clone(),
                            value: installed,
                        });
                    }
                    _ => {
                        xref.stats.set_bails += 1;
                        xref.unresolved.push(UnresolvedUse {
                            script: *id,
                            pc,
                            command: instr.command.clone(),
                            reason: "hook component, descriptor or script is not proven",
                        });
                    }
                }
            }
            // Adjacent-copy tracking: `push <v>; pop_int_local($x)` with an
            // untargeted pop records `$x = v` for the rest of the segment
            // (anything else un-records it). Sound: a non-targeted pop is
            // reachable only from its previous index, so the adjacent push
            // always just ran. `$arg` slots are excluded — entry values own
            // those (see `resolve_int_args`).
            //
            // Boolean-taint tracking rides the same gate: `find;
            // pop_int_local($x)` with an untargeted pop records `$x` as
            // holding that find's pushed boolean (finds push exactly one int,
            // the `? 1 : 0` result, so the pop provably captures it — the
            // value itself stays unknown, only the provenance is tracked).
            // Any other pop to the slot overwrites it and clears the taint,
            // so locals holding ordinary ints never taint. The scope slot and
            // pack parent are captured from the post-find scope (nothing runs
            // between the adjacent find and its pop), never re-resolved at
            // guard time.
            if let ("pop_int_local", Operand::Local(slot)) =
                (instr.command.as_str(), &instr.operand)
                && !targeted
                && *slot >= i32::from(script.args.int)
                && *slot < i32::from(script.args.int) + i32::from(script.locals.int)
                && let Some(previous) = position.checked_sub(1).and_then(|at| script.code.get(at))
            {
                taints.remove(slot);
                if matches!(previous.command.as_str(), "cc_find" | "if_find") {
                    let scope_slot = scope_slot(&previous.operand);
                    let parent = scope_slot.and_then(|selected| match scope[selected] {
                        Scope::Pack(parent) => Some(parent),
                        Scope::Unknown | Scope::Dynamic { .. } => None,
                    });
                    taints.insert(
                        *slot,
                        TaintedBool {
                            slot: scope_slot,
                            parent,
                        },
                    );
                }
                match (&previous.command[..], &previous.operand) {
                    ("push_constant_string" | "push_constant_int", Operand::Int(value)) => {
                        local_vals.insert(*slot, *value);
                    }
                    ("push_int_local", Operand::Local(from)) => {
                        match known_int(*from, &int_args, &local_vals) {
                            Some(value) => {
                                local_vals.insert(*slot, value);
                            }
                            None => {
                                local_vals.remove(slot);
                            }
                        }
                    }
                    _ => {
                        local_vals.remove(slot);
                    }
                }
            }
        }
    }
    xref
}

/// Decode both runtime packs into the maps [`build_xref`] consumes:
/// scripts keyed by group id, components keyed by `(interface, file)`.
/// Shared by the `inspect` verb and the corpus gate so both see the same
/// roster. A script or component that fails to decode is a hard error —
/// reported, never skipped.
pub fn load_scripts(pack_root: &Path) -> Result<ScriptMap> {
    let book = OpcodeBook::embedded()?;
    let scripts_archive = PackArchive::open(&pack_root.join("client.scripts.js5"))?;
    let mut scripts = BTreeMap::new();
    let mut execution = crate::execution::Table::default();
    for group in scripts_archive.group_ids() {
        let files = scripts_archive
            .group_files(group)?
            .ok_or_else(|| NativeError::Invalid(format!("missing script group {group}")))?;
        let id = i32::try_from(group)
            .map_err(|_| NativeError::Invalid("script ID out of range".into()))?;
        let (script, metadata) = crate::execution::decode_group(id, &files, &book)
            .map_err(|error| NativeError::Invalid(format!("scripts {group}: {error}")))?;
        execution.extend(metadata)?;
        scripts.insert(id, script);
    }
    execution.validate_links(&scripts)?;
    Ok(scripts)
}

/// Decode scripts and interface components from the provisioned runtime packs.
pub fn load_corpus(pack_root: &Path) -> Result<(ScriptMap, ComponentMap)> {
    let scripts = load_scripts(pack_root)?;
    let ifaces_archive = PackArchive::open(&pack_root.join("client.interfaces.js5"))?;
    let mut components = BTreeMap::new();
    for group in ifaces_archive.group_ids() {
        let parentlayer = (group << 16) as i32;
        let Some(files) = ifaces_archive.group_files(group)? else {
            continue;
        };
        for (file, bytes) in files {
            let component = decode_component(&bytes, parentlayer).map_err(|error| {
                NativeError::Invalid(format!("interfaces {group}/{file}: {error}"))
            })?;
            components.insert((i32::try_from(group).unwrap_or(i32::MAX), file), component);
        }
    }
    Ok((scripts, components))
}

/// Render a script id through the registry (`~name` when curated, numeric).
fn script_name(symbols: &SymbolRegistry, id: i32) -> String {
    match symbols.name_for_call(id) {
        Some(name) => format!("~{name}"),
        None => id.to_string(),
    }
}

/// One-line xref summary: totals, top linked scripts and components, and the
/// first dangling samples.
#[must_use]
pub fn format_summary(
    xref: &Xref,
    symbols: &SymbolRegistry,
    names: &crate::inames::InterfaceRegistry,
) -> String {
    use std::fmt::Write as _;
    let stats = &xref.stats;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "xref: {} scripts, {} components",
        stats.scripts, stats.components
    );
    let _ = writeln!(
        out,
        "scripts -> components: {} sites, {} linked, {} bailed, {} dangling",
        stats.sites, stats.linked, stats.bails, stats.dangling
    );
    let _ = writeln!(
        out,
        "components -> scripts: {} hook heads, {} resolved, {} unresolved",
        stats.hook_heads, stats.hook_resolved, stats.hook_unresolved
    );
    let _ = writeln!(
        out,
        "scripts -> scripts: {} setter sites, {} installed, {} dangling, {} bailed",
        stats.set_sites, stats.set_linked, stats.set_dangling, stats.set_bails
    );
    let _ = writeln!(
        out,
        "active-component hooks: {} setter sites, {} installed, {} clears/missing, {} bailed",
        stats.active_hook_sites,
        stats.active_hook_linked,
        stats.active_hook_dangling,
        stats.active_hook_bails
    );
    let mut by_script: Vec<(i32, usize)> = xref
        .script_to_comps
        .iter()
        .map(|(id, uses)| (*id, uses.len()))
        .collect();
    by_script.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let _ = writeln!(
        out,
        "cc_ scope: {} sites, {} linked, {} dynamic, {} bailed",
        stats.cc_sites, stats.cc_linked, stats.cc_dynamic, stats.cc_bails
    );
    let creations: usize = xref.creations.values().map(Vec::len).sum();
    let _ = writeln!(out, "creations: {creations} cc_create edges");
    let _ = writeln!(
        out,
        "args: {} scripts with resolved entry values; finds: {} sites, {} pack",
        stats.arg_scripts, stats.finds, stats.finds_pack
    );
    let _ = writeln!(out, "top scripts by component touches:");
    for (id, count) in by_script.iter().take(10) {
        let _ = writeln!(
            out,
            "  script {} ({}): {count}",
            script_name(symbols, *id),
            id
        );
    }
    let mut incoming: BTreeMap<(i32, u32), usize> = BTreeMap::new();
    for use_ in xref.script_to_comps.values().flatten() {
        let key = (
            use_.target.iface,
            u32::try_from(use_.target.child).unwrap_or(u32::MAX),
        );
        *incoming.entry(key).or_insert(0) += 1;
    }
    let mut by_comp: Vec<((i32, u32), usize)> = incoming.into_iter().collect();
    by_comp.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let _ = writeln!(out, "top components by incoming touches:");
    for ((iface, child), count) in by_comp.iter().take(10) {
        let _ = writeln!(out, "  {}: {count}", names.display(*iface, *child));
    }
    let mut by_iface: BTreeMap<i32, usize> = BTreeMap::new();
    for ((iface, _), count) in &by_comp {
        *by_iface.entry(*iface).or_insert(0) += count;
    }
    let mut interfaces: Vec<(i32, usize)> = by_iface.into_iter().collect();
    interfaces.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let _ = writeln!(out, "top interfaces by incoming touches:");
    for (iface, count) in interfaces.iter().take(10) {
        let _ = writeln!(out, "  {}: {count}", names.display_iface(*iface));
    }
    if !xref.dangling.is_empty() {
        let _ = writeln!(out, "dangling samples:");
        for sample in xref.dangling.iter().take(10) {
            let _ = writeln!(
                out,
                "  script {} @{}: {}({})",
                sample.script, sample.pc, sample.command, sample.value
            );
        }
    }
    if !xref.hook_dangling.is_empty() {
        const CLEARED_HOOK: i32 = -1;
        let _ = writeln!(out, "hook clears and missing scripts:");
        for sample in xref.hook_dangling.iter().take(10) {
            let action = if sample.value == CLEARED_HOOK {
                "clears hook".to_string()
            } else {
                format!("installs missing script {}", sample.value)
            };
            let _ = writeln!(
                out,
                "  script {} @{}: {} {action}",
                script_name(symbols, sample.script),
                sample.pc,
                sample.command
            );
        }
    }
    out
}

/// Render one interface as a child tree: each child with its type, geometry,
/// hide flag, populated hook slots (with `~names`), assets, and incoming script
/// touches. `components` holds this interface's `(iface, file)` entries.
/// Names render where `names` knows them (`bank/main`), numbers elsewhere.
#[must_use]
pub fn format_interface(
    iface: i32,
    components: &ComponentMap,
    xref: &Xref,
    symbols: &SymbolRegistry,
    assets: Option<&crate::assets::AssetIndex>,
    names: &crate::inames::InterfaceRegistry,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let children = components
        .keys()
        .filter(|(group, _)| *group == iface)
        .count();
    match names.interface_name(iface) {
        Some(name) => {
            let _ = writeln!(out, "interface {iface} ({name}, {children} children):");
        }
        None => {
            let _ = writeln!(out, "interface {iface} ({children} children):");
        }
    }
    let mut files: Vec<u32> = components
        .keys()
        .filter_map(|(group, file)| (*group == iface).then_some(*file))
        .collect();
    files.sort_unstable();
    if files.is_empty() {
        out.push_str("  (unknown interface)\n");
        return out;
    }
    for file in files {
        let component = &components[&(iface, file)];
        let kind = crate::isource::ComponentType::from_type_id(component.type_id).map_or_else(
            || format!("type{}", component.type_id),
            |kind| kind.word().to_string(),
        );
        let label = match names.child_name(iface, file) {
            Some(name) => format!("child {file} ({name})"),
            None => format!("child {file}"),
        };
        let _ = writeln!(
            out,
            "  {label}: {kind} @({},{}) {}x{}{}",
            component.x,
            component.y,
            component.width,
            component.height,
            if component.hide { " hidden" } else { "" },
        );
        for (slot, hook) in hook_slots(&component.hooks) {
            let Some(args) = hook else { continue };
            let mut rendered = Vec::new();
            for (position, arg) in args.iter().enumerate() {
                match arg {
                    HookArg::Int(value) if position == 0 => {
                        rendered.push(script_name(symbols, *value));
                    }
                    HookArg::Int(value) => rendered.push(value.to_string()),
                    HookArg::Str(text) => rendered.push(format!("{text:?}")),
                }
            }
            let _ = writeln!(out, "    {slot}: {}", rendered.join(", "));
        }
        if let Some(index) = assets {
            for line in crate::assets::format_component_assets(iface, file, index) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        if let Some(uses) = xref.comp_to_scripts.get(&(iface, file)) {
            for hook in uses {
                let _ = writeln!(
                    out,
                    "    <- hook {} installs {}",
                    hook.slot,
                    script_name(symbols, hook.script)
                );
            }
        }
        let incoming: Vec<&ScriptUse> = xref
            .script_to_comps
            .values()
            .flatten()
            .filter(|use_| {
                use_.target.iface == iface && use_.target.child == i32::try_from(file).unwrap_or(-1)
            })
            .collect();
        for touch in incoming {
            let _ = writeln!(
                out,
                "    <- script {} @{} via {}",
                script_name(symbols, touch.script),
                touch.pc,
                touch.command
            );
        }
        let installed_hooks: Vec<&HookSet> = xref
            .hook_sets
            .values()
            .flatten()
            .filter(|set| {
                set.target.iface == iface && set.target.child == i32::try_from(file).unwrap_or(-1)
            })
            .collect();
        for set in installed_hooks {
            let _ = writeln!(
                out,
                "    <- script {} @{} sets {} as {} here",
                script_name(symbols, set.caller),
                set.pc,
                script_name(symbols, set.callee),
                hook_slot_word(&set.command)
            );
        }
        let built_here: Vec<&CreationUse> = xref
            .creations
            .values()
            .flatten()
            .filter(|made| {
                made.parent.iface == iface && made.parent.child == i32::try_from(file).unwrap_or(-1)
            })
            .collect();
        for made in built_here {
            let _ = writeln!(
                out,
                "    <- script {} creates child #{} here",
                script_name(symbols, made.script),
                made.index
                    .map_or_else(|| "?".to_string(), |index| index.to_string()),
            );
        }
    }
    out
}

/// The hook-slot word an `if_seton*` command installs (`if_setontimer` →
/// `ontimer`): the `if_set` prefix is rigid across [`IF_HOOK_SETTERS`]; the
/// `_alias` opcode installs the plain slot (`ondragcomplete`).
#[must_use]
pub fn hook_slot_word(command: &str) -> &str {
    let word = command
        .strip_prefix("if_set")
        .or_else(|| command.strip_prefix("cc_set"))
        .unwrap_or(command);
    word.strip_suffix("_alias").unwrap_or(word)
}

/// Render one script's component touches, hook installations it performs,
/// plus the hook installations that call it.
#[must_use]
pub fn format_script(script: i32, xref: &Xref, symbols: &SymbolRegistry) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "script {} ({}):", script_name(symbols, script), script);
    if let Some(owner) = xref.entry_contexts.get(&script) {
        let _ = writeln!(
            out,
            "  argument refinement assumes cached hook entry from {}/{}; runtime entries may differ",
            owner.iface, owner.child
        );
    }
    match xref.script_to_comps.get(&script) {
        None => out.push_str("  no resolved component touches\n"),
        Some(uses) => {
            for touch in uses {
                let _ = writeln!(
                    out,
                    "  @{:<6} {} -> {}/{}",
                    touch.pc, touch.command, touch.target.iface, touch.target.child
                );
            }
        }
    }
    match xref.hook_sets.get(&script) {
        None => out.push_str("  no resolved hook installations\n"),
        Some(sets) => {
            for set in sets {
                let _ = writeln!(
                    out,
                    "  @{} sets {} as {} of {}/{}",
                    set.pc,
                    script_name(symbols, set.callee),
                    hook_slot_word(&set.command),
                    set.target.iface,
                    set.target.child
                );
            }
        }
    }
    let mut installed: Vec<&HookUse> = xref
        .comp_to_scripts
        .values()
        .flatten()
        .filter(|hook| hook.script == script)
        .collect();
    installed.sort_by_key(|hook| (hook.iface, hook.child));
    if installed.is_empty() {
        out.push_str("  installed by no cached hooks\n");
    } else {
        for hook in installed {
            let _ = writeln!(
                out,
                "  installed by {}/{}:{}",
                hook.iface, hook.child, hook.slot
            );
        }
    }
    for set in xref
        .hook_sets
        .values()
        .flatten()
        .filter(|set| set.callee == script)
    {
        let _ = writeln!(
            out,
            "  installed by script {} @{} as {} of {}/{}",
            script_name(symbols, set.caller),
            set.pc,
            hook_slot_word(&set.command),
            set.target.iface,
            set.target.child
        );
    }
    match xref.creations.get(&script) {
        None => out.push_str("  no resolved component creations\n"),
        Some(created) => {
            for made in created {
                let _ = writeln!(
                    out,
                    "  @{} creates under {}/{} (child #{}, type {})",
                    made.pc,
                    made.parent.iface,
                    made.parent.child,
                    made.index
                        .map_or_else(|| "?".to_string(), |index| index.to_string()),
                    made.child_type
                        .map_or_else(|| "?".to_string(), |type_| type_.to_string()),
                );
            }
        }
    }
    for touch in xref.dynamic_touches.get(&script).into_iter().flatten() {
        let _ = writeln!(
            out,
            "  @{} {} -> runtime child created @{} (parent {})",
            touch.pc,
            touch.command,
            touch.creator_pc,
            touch.parent.map_or_else(
                || "unknown".into(),
                |parent| format!("{}/{}", parent.iface, parent.child)
            )
        );
    }
    for hook in xref.dynamic_hook_sets.get(&script).into_iter().flatten() {
        let _ = writeln!(
            out,
            "  @{} sets {} as {} on runtime child created @{}",
            hook.pc,
            script_name(symbols, hook.callee),
            hook_slot_word(&hook.command),
            hook.creator_pc
        );
    }
    for hook in xref
        .dynamic_hook_sets
        .values()
        .flatten()
        .filter(|hook| hook.callee == script)
    {
        let _ = writeln!(
            out,
            "  installed by script {} @{} as {} on runtime child created @{}",
            script_name(symbols, hook.caller),
            hook.pc,
            hook_slot_word(&hook.command),
            hook.creator_pc
        );
    }
    for site in xref.unresolved.iter().filter(|site| site.script == script) {
        let _ = writeln!(
            out,
            "  unresolved @{} {}: {}",
            site.pc, site.command, site.reason
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::OpcodeBook;

    fn book() -> OpcodeBook {
        OpcodeBook::embedded().unwrap()
    }

    /// Build decoded instructions from `(command, operand)` pairs with
    /// canonical opcodes.
    fn code(pairs: Vec<(&str, Operand)>) -> Vec<Instruction> {
        let book = book();
        pairs
            .into_iter()
            .map(|(command, operand)| Instruction {
                opcode: book.opcode_for(command).unwrap(),
                command: command.to_string(),
                operand,
            })
            .collect()
    }

    #[test]
    fn component_ids_pack_and_unpack() {
        const NEXT_INTERFACE: i32 = 1;
        for iface in [MAX_INTERFACE_ID + NEXT_INTERFACE, i32::MAX] {
            assert!(
                pack_component(ComponentRef {
                    iface,
                    child: i32::default()
                })
                .is_none()
            );
        }
        let largest = ComponentRef {
            iface: MAX_INTERFACE_ID,
            child: COMPONENT_CHILD_MASK,
        };
        assert_eq!(
            unpack_component(pack_component(largest).unwrap()),
            Some(largest)
        );
        let reference = ComponentRef {
            iface: 12,
            child: 34,
        };
        let packed = pack_component(reference).unwrap();
        assert_eq!(packed, 0x000C_0022);
        assert_eq!(unpack_component(packed), Some(reference));
        // Negative values are not references; oversized parts do not pack.
        assert_eq!(unpack_component(-1), None);
        assert_eq!(
            pack_component(ComponentRef {
                iface: -1,
                child: 0
            }),
            None
        );
        assert_eq!(
            pack_component(ComponentRef {
                iface: 0,
                child: 0x1_0000
            }),
            None
        );
        // Zero is interface 0, child 0 — well-formed (roster decides).
        assert_eq!(
            unpack_component(0),
            Some(ComponentRef { iface: 0, child: 0 })
        );
    }

    #[test]
    fn walk_recovers_adjacent_literals() {
        // push 0x30007; push "s"; if_settext  ->  0x30007.
        let instructions = code(vec![
            ("push_constant_int", Operand::Int(0x30007)),
            ("push_constant_string", Operand::Str("s".to_string())),
            ("if_settext", Operand::Byte(0)),
        ]);
        assert_eq!(
            recover_component_operand(
                &instructions,
                2,
                0,
                &IntArgValues::new(),
                &LocalIntValues::new()
            ),
            Some(0x30007)
        );
    }

    #[test]
    fn walk_skips_pops_and_counts_depth() {
        // push 1; pop $x(int slot 0); push 0x30007; if_sethide(0x? )
        // The pop consumes push 1, so the top int is still 0x30007.
        let instructions = code(vec![
            ("push_constant_int", Operand::Int(1)),
            ("pop_int_local", Operand::Local(0)),
            ("push_constant_int", Operand::Int(0x30007)),
            ("if_sethide", Operand::Byte(0)),
        ]);
        assert_eq!(
            recover_component_operand(
                &instructions,
                3,
                0,
                &IntArgValues::new(),
                &LocalIntValues::new()
            ),
            Some(0x30007)
        );
        // Without the pop the top int would be 0x30007 either way here; prove
        // the pop is really counted: push A; push B; pop; if_ -> A.
        let instructions = code(vec![
            ("push_constant_int", Operand::Int(0x10001)),
            ("push_constant_int", Operand::Int(0x20002)),
            ("pop_int_local", Operand::Local(0)),
            ("if_sethide", Operand::Byte(0)),
        ]);
        assert_eq!(
            recover_component_operand(
                &instructions,
                3,
                0,
                &IntArgValues::new(),
                &LocalIntValues::new()
            ),
            Some(0x10001)
        );
    }

    #[test]
    fn walk_bails_on_computed_and_unknown_values() {
        // add result on top: computed, not a literal.
        let computed = code(vec![
            ("push_constant_int", Operand::Int(1)),
            ("push_constant_int", Operand::Int(2)),
            ("add", Operand::Byte(0)),
            ("if_sethide", Operand::Byte(0)),
        ]);
        assert_eq!(
            recover_component_operand(
                &computed,
                3,
                0,
                &IntArgValues::new(),
                &LocalIntValues::new()
            ),
            None
        );
        // $local on top: reaching definitions are a follow-up.
        let local = code(vec![
            ("push_int_local", Operand::Local(0)),
            ("if_sethide", Operand::Byte(0)),
        ]);
        assert_eq!(
            recover_component_operand(&local, 1, 0, &IntArgValues::new(), &LocalIntValues::new()),
            None
        );
        // varbit on top: same story.
        let varbit = code(vec![
            (
                "push_varbit",
                Operand::VarBitRef(crate::script::VarBitRef {
                    id: 5,
                    transmog: false,
                }),
            ),
            ("if_sethide", Operand::Byte(0)),
        ]);
        assert_eq!(
            recover_component_operand(&varbit, 1, 0, &IntArgValues::new(), &LocalIntValues::new()),
            None
        );
    }

    #[test]
    fn walk_handles_merge_points() {
        let empty = LocalIntValues::new();
        // A jump into the middle of a run ends it: another path could arrive
        // with a different stack below.
        let middle = code(vec![
            ("push_constant_int", Operand::Int(0x30007)),
            ("push_constant_int", Operand::Int(0x30008)),
            ("push_constant_int", Operand::Int(0x30009)),
            ("if_sethide", Operand::Byte(0)),
            ("branch", Operand::Branch(2)),
        ]);
        assert_eq!(
            recover_component_operand(&middle, 3, 2, &IntArgValues::new(), &empty),
            None
        );
        // A jump landing exactly on the terminal push is fine: the push runs
        // unconditionally on every path through it.
        let terminal = code(vec![
            ("push_constant_int", Operand::Int(0x30007)),
            ("if_sethide", Operand::Byte(0)),
            ("branch", Operand::Branch(0)),
        ]);
        assert_eq!(
            recover_component_operand(&terminal, 1, 0, &IntArgValues::new(), &empty),
            Some(0x30007)
        );
        // A jump landing on the command itself bails: it runs against an
        // ambient stack the walk never saw.
        let command = code(vec![
            ("push_constant_int", Operand::Int(0x30007)),
            ("if_sethide", Operand::Byte(0)),
            ("branch", Operand::Branch(1)),
        ]);
        assert_eq!(
            recover_component_operand(&command, 1, 0, &IntArgValues::new(), &empty),
            None
        );
    }

    #[test]
    fn walk_resolves_triggerop_depth_two() {
        // push packed; push sub; push op; if_triggerop -> packed.
        let instructions = code(vec![
            ("push_constant_int", Operand::Int(0x40009)),
            ("push_constant_int", Operand::Int(3)),
            ("push_constant_int", Operand::Int(1)),
            ("if_triggerop", Operand::Byte(0)),
        ]);
        assert_eq!(
            recover_component_operand(
                &instructions,
                3,
                2,
                &IntArgValues::new(),
                &LocalIntValues::new()
            ),
            Some(0x40009)
        );
    }

    #[test]
    fn hook_heads_resolve_through_the_roster() {
        use crate::interface::Hooks;
        let hooks = Hooks {
            onload: Some(vec![crate::interface::HookArg::Int(7)]),
            onclick: Some(vec![crate::interface::HookArg::Int(9999)]),
            ..Hooks::default()
        };
        let component = InterfaceComponent {
            version: -1,
            type_id: 4,
            name: None,
            clientcode: 0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            width_mode: 0,
            height_mode: 0,
            x_mode: 0,
            y_mode: 0,
            aspect: None,
            layer: -1,
            hide: false,
            noclickthrough: false,
            body: crate::interface::ComponentBody::Text {
                font: 0,
                // Version -1 carries no fontmono byte: the default.
                mono: true,
                text: String::new(),
                line_height: 0,
                halign: 0,
                valign: 0,
                shadow: false,
                colour: 0,
                trans: 0,
                maxlines: 0,
            },
            keymask: 0,
            keybinds: Vec::new(),
            opbase: String::new(),
            ops: Vec::new(),
            opname_nibble: 0,
            opname_first: None,
            opname_second: None,
            pausetext: None,
            dragdeadzone: 0,
            dragdeadtime: 0,
            dragrenderbehaviour: 0,
            targetverb: String::new(),
            target: None,
            mouseovercursor: -1,
            int_params: Vec::new(),
            str_params: Vec::new(),
            hooks,
            transmits: crate::interface::Transmits::default(),
        };
        let scripts = BTreeMap::from([(
            7,
            CompiledScript {
                name: None,
                args: crate::script::Counts::default(),
                locals: crate::script::Counts::default(),
                code: Vec::new(),
            },
        )]);
        let components = BTreeMap::from([((3, 1), component)]);
        let xref = build_xref(&scripts, &components);
        assert_eq!(xref.stats.hook_heads, 2);
        assert_eq!(xref.stats.hook_resolved, 1);
        assert_eq!(xref.stats.hook_unresolved, 1);
        let uses = &xref.comp_to_scripts[&(3, 1)];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].slot, "onload");
        assert_eq!(uses[0].script, 7);
    }

    fn installer(comp: (i32, u32), tail: Vec<HookArg>) -> Installer {
        Installer { comp, tail }
    }

    fn script_with(args_int: u16, args_obj: u16, pairs: Vec<(&str, Operand)>) -> CompiledScript {
        CompiledScript {
            name: None,
            args: crate::script::Counts {
                int: args_int,
                obj: args_obj,
                long: 0,
            },
            locals: crate::script::Counts::default(),
            code: code(pairs),
        }
    }

    #[test]
    fn args_resolve_literal_and_magic_tails() {
        // Single installer, literal tail: slot 0 is the packed id.
        let script = script_with(1, 0, Vec::new());
        let installers = vec![installer((3, 1), vec![HookArg::Int((3 << 16) | 1)])];
        assert_eq!(
            resolve_int_args(&script, &installers, false),
            Some(BTreeMap::from([(0, Some((3 << 16) | 1))]))
        );
        // Owner placeholders derive; runtime placeholders stay unknown.
        let installers = vec![installer(
            (3, 1),
            vec![
                HookArg::Int(HOOK_MAGIC_PARENTLAYER),
                HookArg::Int(HOOK_MAGIC_COMPONENT_ID),
            ],
        )];
        let script = script_with(2, 0, Vec::new());
        assert_eq!(
            resolve_int_args(&script, &installers, false),
            Some(BTreeMap::from([(0, Some((3 << 16) | 1)), (1, Some(-1))]))
        );
        let installers = vec![installer((3, 1), vec![HookArg::Int(-2_147_483_647)])];
        assert_eq!(
            resolve_int_args(&script_with(1, 0, Vec::new()), &installers, false),
            Some(BTreeMap::from([(0, None)]))
        );
        // Uncovered trailing slots stay fresh-array zero.
        let installers = vec![installer((3, 1), Vec::new())];
        assert_eq!(
            resolve_int_args(&script_with(2, 0, Vec::new()), &installers, false),
            Some(BTreeMap::from([(0, Some(0)), (1, Some(0))]))
        );
    }

    #[test]
    fn reassignment_invalidates_owner_placeholders_and_omitted_arguments() {
        const ARG_COUNT: u16 = 1;
        const FIRST_ARG: i32 = 0;
        const INTERFACE: i32 = 3;
        const FILE: u32 = 1;
        let script = script_with(
            ARG_COUNT,
            u16::default(),
            vec![
                ("push_constant_int", Operand::Int(i32::default())),
                ("pop_int_local", Operand::Local(FIRST_ARG)),
            ],
        );
        for tail in [
            vec![HookArg::Int(HOOK_MAGIC_PARENTLAYER)],
            vec![HookArg::Int(HOOK_MAGIC_COMPONENT_ID)],
            Vec::new(),
        ] {
            let values =
                resolve_int_args(&script, &[installer((INTERFACE, FILE), tail)], false).unwrap();
            assert_eq!(values[&FIRST_ARG], None);
        }
    }

    #[test]
    fn args_bail_on_installers_callers_cover_and_assignment() {
        let script = script_with(1, 0, Vec::new());
        let one = vec![installer((3, 1), vec![HookArg::Int(7)])];
        // Two installers, gosub callers, and overlong tails all veto.
        let two = vec![
            installer((3, 1), vec![HookArg::Int(7)]),
            installer((3, 2), vec![HookArg::Int(8)]),
        ];
        assert_eq!(resolve_int_args(&script, &two, false), None);
        assert_eq!(resolve_int_args(&script, &one, true), None);
        assert!(resolve_int_args(&script, &Vec::new(), false).is_none());
        let long_tail = vec![installer((3, 1), vec![HookArg::Int(1), HookArg::Int(2)])];
        assert_eq!(resolve_int_args(&script, &long_tail, false), None);
        // String tails count toward object args only.
        let str_tail = vec![installer((3, 1), vec![HookArg::Str("s".to_string())])];
        assert_eq!(
            resolve_int_args(&script_with(0, 1, Vec::new()), &str_tail, false),
            Some(BTreeMap::new())
        );
        // An assigned `$arg` slot goes unknown.
        let assigned = script_with(
            1,
            0,
            vec![
                ("push_constant_int", Operand::Int(9)),
                ("pop_int_local", Operand::Local(0)),
            ],
        );
        assert_eq!(
            resolve_int_args(&assigned, &one, false),
            Some(BTreeMap::from([(0, None)]))
        );
    }

    #[test]
    fn args_flow_into_operand_recovery() {
        // push $param0; if_sethide with $param0 bound to a packed id.
        let instructions = code(vec![
            ("push_int_local", Operand::Local(0)),
            ("if_sethide", Operand::Byte(0)),
        ]);
        let bound = BTreeMap::from([(0, Some(0x30007))]);
        assert_eq!(
            recover_component_operand(&instructions, 1, 0, &bound, &LocalIntValues::new()),
            Some(0x30007)
        );
        let unknown = BTreeMap::from([(0, None)]);
        assert_eq!(
            recover_component_operand(&instructions, 1, 0, &unknown, &LocalIntValues::new()),
            None
        );
        // Segment copies resolve the same way through the locals map.
        let copied = BTreeMap::from([(0, 0x30007)]);
        assert_eq!(
            recover_component_operand(&instructions, 1, 0, &IntArgValues::new(), &copied),
            Some(0x30007)
        );
    }

    #[test]
    fn scope_links_finds_creates_and_resets() {
        let packed = (3 << 16) | 1;
        let find = script_with(
            0,
            0,
            vec![
                ("push_constant_int", Operand::Int(packed)),
                ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
                ("if_find", Operand::Byte(0)),
                ("push_constant_string", Operand::Str("s".to_string())),
                ("cc_settext", Operand::Byte(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let create = script_with(
            0,
            0,
            vec![
                ("push_constant_int", Operand::Int(packed)),
                ("push_constant_int", Operand::Int(5)),
                ("push_constant_int", Operand::Int(3)),
                ("cc_create", Operand::Byte(0)),
                ("push_constant_string", Operand::Str("s".to_string())),
                ("cc_settext", Operand::Byte(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        // gosub wipes scope: the trailing touch bails.
        let reset = script_with(
            0,
            0,
            vec![
                ("push_constant_int", Operand::Int(packed)),
                ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
                ("if_find", Operand::Byte(0)),
                ("gosub_with_params", Operand::Script(99)),
                ("push_constant_string", Operand::Str("s".to_string())),
                ("cc_settext", Operand::Byte(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let scripts = BTreeMap::from([(1, find), (2, create), (3, reset)]);
        let mut components = BTreeMap::new();
        components.insert(
            (3, 1),
            InterfaceComponent {
                version: -1,
                type_id: 0,
                name: None,
                clientcode: 0,
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                width_mode: 0,
                height_mode: 0,
                x_mode: 0,
                y_mode: 0,
                aspect: None,
                layer: -1,
                hide: false,
                noclickthrough: false,
                body: crate::interface::ComponentBody::Layer {
                    scroll_width: 0,
                    scroll_height: 0,
                },
                keymask: 0,
                keybinds: Vec::new(),
                opbase: String::new(),
                ops: Vec::new(),
                opname_nibble: 0,
                opname_first: None,
                opname_second: None,
                pausetext: None,
                dragdeadzone: 0,
                dragdeadtime: 0,
                dragrenderbehaviour: 0,
                targetverb: String::new(),
                target: None,
                mouseovercursor: -1,
                int_params: Vec::new(),
                str_params: Vec::new(),
                hooks: crate::interface::Hooks::default(),
                transmits: crate::interface::Transmits::default(),
            },
        );
        let xref = build_xref(&scripts, &components);
        // Find-then-touch links the pack component.
        let uses = &xref.script_to_comps[&1];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].command, "cc_settext");
        assert_eq!(uses[0].target, ComponentRef { iface: 3, child: 1 });
        // Create records a creation edge; the touch under dynamic scope is
        // counted, never linked.
        let made = &xref.creations[&2];
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].parent, ComponentRef { iface: 3, child: 1 });
        assert_eq!(made[0].index, Some(3));
        assert_eq!(made[0].child_type, Some(5));
        assert!(!xref.script_to_comps.contains_key(&2));
        // Post-gosub touch bails; no edge, no creation.
        assert!(!xref.script_to_comps.contains_key(&3));
        assert!(!xref.creations.contains_key(&3));
        assert_eq!(xref.stats.cc_linked, 1);
        assert_eq!(xref.stats.cc_dynamic, 1);
        assert!(xref.stats.cc_bails >= 1);
    }

    #[test]
    fn scope_secondary_slot_selects_by_operand() {
        let packed = (3 << 16) | 1;
        // find on secondary (operand 1), touch on secondary links, touch on
        // primary bails.
        let script = script_with(
            0,
            0,
            vec![
                ("push_constant_int", Operand::Int(packed)),
                ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
                ("if_find", Operand::Byte(1)),
                ("push_constant_string", Operand::Str("s".to_string())),
                ("cc_settext", Operand::Byte(1)),
                ("push_constant_string", Operand::Str("t".to_string())),
                ("cc_settext", Operand::Byte(0)),
                ("return", Operand::Byte(0)),
            ],
        );
        let scripts = BTreeMap::from([(1, script)]);
        let mut components = BTreeMap::new();
        components.insert(
            (3, 1),
            InterfaceComponent {
                version: -1,
                type_id: 0,
                name: None,
                clientcode: 0,
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                width_mode: 0,
                height_mode: 0,
                x_mode: 0,
                y_mode: 0,
                aspect: None,
                layer: -1,
                hide: false,
                noclickthrough: false,
                body: crate::interface::ComponentBody::Layer {
                    scroll_width: 0,
                    scroll_height: 0,
                },
                keymask: 0,
                keybinds: Vec::new(),
                opbase: String::new(),
                ops: Vec::new(),
                opname_nibble: 0,
                opname_first: None,
                opname_second: None,
                pausetext: None,
                dragdeadzone: 0,
                dragdeadtime: 0,
                dragrenderbehaviour: 0,
                targetverb: String::new(),
                target: None,
                mouseovercursor: -1,
                int_params: Vec::new(),
                str_params: Vec::new(),
                hooks: crate::interface::Hooks::default(),
                transmits: crate::interface::Transmits::default(),
            },
        );
        let xref = build_xref(&scripts, &components);
        let uses = &xref.script_to_comps[&1];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].pc, 4);
        assert_eq!(xref.stats.cc_linked, 1);
        assert_eq!(xref.stats.cc_bails, 1);
    }

    fn dummy_component() -> InterfaceComponent {
        InterfaceComponent {
            version: -1,
            type_id: 0,
            name: None,
            clientcode: 0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            width_mode: 0,
            height_mode: 0,
            x_mode: 0,
            y_mode: 0,
            aspect: None,
            layer: -1,
            hide: false,
            noclickthrough: false,
            body: crate::interface::ComponentBody::Layer {
                scroll_width: 0,
                scroll_height: 0,
            },
            keymask: 0,
            keybinds: Vec::new(),
            opbase: String::new(),
            ops: Vec::new(),
            opname_nibble: 0,
            opname_first: None,
            opname_second: None,
            pausetext: None,
            dragdeadzone: 0,
            dragdeadtime: 0,
            dragrenderbehaviour: 0,
            targetverb: String::new(),
            target: None,
            mouseovercursor: -1,
            int_params: Vec::new(),
            str_params: Vec::new(),
            hooks: crate::interface::Hooks::default(),
            transmits: crate::interface::Transmits::default(),
        }
    }

    /// One local int slot (slot 2: two int args declared, so slot 2 is the
    /// first local).
    fn copy_script(pairs: Vec<(&str, Operand)>) -> CompiledScript {
        CompiledScript {
            name: None,
            args: crate::script::Counts {
                int: 2,
                obj: 0,
                long: 0,
            },
            locals: crate::script::Counts {
                int: 1,
                obj: 0,
                long: 0,
            },
            code: code(pairs),
        }
    }

    #[test]
    fn locals_copies_link_within_segments() {
        let packed = (3 << 16) | 1;
        // push C; pop $l; push $l; if_sethide -> edge to C.
        let clean = copy_script(vec![
            ("push_constant_int", Operand::Int(packed)),
            ("pop_int_local", Operand::Local(2)),
            ("push_int_local", Operand::Local(2)),
            ("if_sethide", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
        ]);
        // Same, but a branch targets the pop: the copy is unreachable by
        // fall-through alone, so no edge.
        let targeted = copy_script(vec![
            ("push_constant_int", Operand::Int(packed)),
            ("pop_int_local", Operand::Local(2)),
            ("push_int_local", Operand::Local(2)),
            ("if_sethide", Operand::Byte(0)),
            ("branch", Operand::Branch(1)),
            ("return", Operand::Byte(0)),
        ]);
        // Reassignment wins: the second copy is what the push reads.
        let reassigned = copy_script(vec![
            ("push_constant_int", Operand::Int(0x40002)),
            ("pop_int_local", Operand::Local(2)),
            ("push_constant_int", Operand::Int(packed)),
            ("pop_int_local", Operand::Local(2)),
            ("push_int_local", Operand::Local(2)),
            ("if_sethide", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
        ]);
        let scripts = BTreeMap::from([(1, clean), (2, targeted), (3, reassigned)]);
        let mut real = BTreeMap::new();
        real.insert((3, 1), dummy_component());
        let xref = build_xref(&scripts, &real);
        assert_eq!(xref.script_to_comps[&1].len(), 1);
        assert_eq!(xref.script_to_comps[&1][0].target.child, 1);
        assert!(!xref.script_to_comps.contains_key(&2));
        assert_eq!(xref.script_to_comps[&3].len(), 1);
        assert_eq!(xref.script_to_comps[&3][0].target.child, 1);
    }

    /// Straight-line find-guard triple with a lone success target:
    /// find; push 1; branch_equals(T); branch(F); return; T: cc_ links.
    fn guard_script() -> (CompiledScript, i32) {
        let packed = (3 << 16) | 1;
        let script = CompiledScript {
            name: None,
            args: crate::script::Counts::default(),
            locals: crate::script::Counts::default(),
            code: code(vec![
                ("push_constant_int", Operand::Int(packed)),
                ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
                ("if_find", Operand::Byte(0)),
                ("push_constant_int", Operand::Int(1)),
                ("branch_equals", Operand::Branch(7)),
                ("branch", Operand::Branch(9)),
                ("return", Operand::Byte(0)),
                ("push_constant_string", Operand::Str("s".to_string())),
                ("cc_settext", Operand::Byte(0)),
                ("return", Operand::Byte(0)),
            ]),
        };
        (script, packed)
    }

    #[test]
    fn guard_success_target_links() {
        let (script, _) = guard_script();
        let scripts = BTreeMap::from([(1, script)]);
        let mut components = BTreeMap::new();
        components.insert((3, 1), dummy_component());
        let xref = build_xref(&scripts, &components);
        let uses = &xref.script_to_comps[&1];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].pc, 8);
        assert_eq!(uses[0].target, ComponentRef { iface: 3, child: 1 });
        assert_eq!(xref.stats.cc_linked, 1);
    }

    #[test]
    fn guard_fall_through_bails() {
        // find; push 1; branch_equals(dead); cc_ on the fall-through: the
        // test failed, scope was nulled, no edge.
        let packed = (3 << 16) | 1;
        let script = CompiledScript {
            name: None,
            args: crate::script::Counts::default(),
            locals: crate::script::Counts::default(),
            code: code(vec![
                ("push_constant_int", Operand::Int(packed)),
                ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
                ("if_find", Operand::Byte(0)),
                ("push_constant_int", Operand::Int(1)),
                ("branch_equals", Operand::Branch(7)),
                ("push_constant_string", Operand::Str("s".to_string())),
                ("cc_settext", Operand::Byte(0)),
                ("return", Operand::Byte(0)),
            ]),
        };
        let scripts = BTreeMap::from([(1, script)]);
        let mut components = BTreeMap::new();
        components.insert((3, 1), dummy_component());
        let xref = build_xref(&scripts, &components);
        assert!(!xref.script_to_comps.contains_key(&1));
        assert_eq!(xref.stats.cc_bails, 1);
    }

    #[test]
    fn guard_if_true_links_and_resets_fall_through() {
        // find; branch_if_true(T): T links, the fall-through bails.
        let packed = (3 << 16) | 1;
        let script = CompiledScript {
            name: None,
            args: crate::script::Counts::default(),
            locals: crate::script::Counts::default(),
            code: code(vec![
                ("push_constant_int", Operand::Int(packed)),
                ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
                ("if_find", Operand::Byte(0)),
                ("branch_if_true", Operand::Branch(7)),
                ("push_constant_string", Operand::Str("s".to_string())),
                ("cc_settext", Operand::Byte(0)),
                ("branch", Operand::Branch(9)),
                ("push_constant_string", Operand::Str("t".to_string())),
                ("cc_settext", Operand::Byte(0)),
                ("return", Operand::Byte(0)),
            ]),
        };
        let scripts = BTreeMap::from([(1, script)]);
        let mut components = BTreeMap::new();
        components.insert((3, 1), dummy_component());
        let xref = build_xref(&scripts, &components);
        let uses = &xref.script_to_comps[&1];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].pc, 8);
        assert_eq!(xref.stats.cc_linked, 1);
        assert_eq!(xref.stats.cc_bails, 1);
    }

    #[test]
    fn guard_multi_predecessor_stays_unknown() {
        // Two branches share target 7: the merge is unknowable, no claim.
        let packed = (3 << 16) | 1;
        let script = CompiledScript {
            name: None,
            args: crate::script::Counts::default(),
            locals: crate::script::Counts::default(),
            code: code(vec![
                ("push_constant_int", Operand::Int(packed)),
                ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
                ("if_find", Operand::Byte(0)),
                ("push_constant_int", Operand::Int(1)),
                ("branch_equals", Operand::Branch(7)),
                ("branch", Operand::Branch(7)),
                ("return", Operand::Byte(0)),
                ("push_constant_string", Operand::Str("s".to_string())),
                ("cc_settext", Operand::Byte(0)),
                ("return", Operand::Byte(0)),
            ]),
        };
        let scripts = BTreeMap::from([(1, script)]);
        let mut components = BTreeMap::new();
        components.insert((3, 1), dummy_component());
        let xref = build_xref(&scripts, &components);
        assert!(!xref.script_to_comps.contains_key(&1));
    }

    #[test]
    fn walk_accepts_targeted_terminal_push() {
        // Regression pin for script 937: the parent push is itself a jump
        // target (branch into the creation run), which must not end the
        // walk — the push runs on every path through it.
        let packed = (335 << 16) | 16;
        let script = CompiledScript {
            name: None,
            args: crate::script::Counts::default(),
            locals: crate::script::Counts {
                int: 5,
                obj: 0,
                long: 0,
            },
            code: code(vec![
                ("push_int_local", Operand::Local(4)),
                ("push_constant_int", Operand::Int(90)),
                ("inv_size", Operand::Byte(0)),
                ("branch_less_than", Operand::Branch(5)),
                ("branch", Operand::Branch(9)),
                ("push_constant_int", Operand::Int(packed)),
                ("push_constant_int", Operand::Int(3)),
                ("push_int_local", Operand::Local(4)),
                ("cc_create", Operand::Byte(0)),
                ("return", Operand::Byte(0)),
            ]),
        };
        let scripts = BTreeMap::from([(1, script)]);
        let mut components = BTreeMap::new();
        components.insert((335, 16), dummy_component());
        let xref = build_xref(&scripts, &components);
        assert_eq!(xref.creations.get(&1).map(Vec::len), Some(1));
        assert_eq!(
            xref.creations[&1][0].parent,
            ComponentRef {
                iface: 335,
                child: 16
            }
        );
    }

    /// One int `$local` (slot 0: no int args declared, one int local).
    fn taint_script(pairs: Vec<(&str, Operand)>) -> CompiledScript {
        CompiledScript {
            name: None,
            args: crate::script::Counts::default(),
            locals: crate::script::Counts {
                int: 1,
                obj: 0,
                long: 0,
            },
            code: code(pairs),
        }
    }

    fn taint_components() -> ComponentMap {
        BTreeMap::from([((3, 1), dummy_component())])
    }

    #[test]
    fn taint_guard_via_local_links() {
        // find; pop $l; push $l; push 1; branch_equals(T): T links, the
        // fall-through bails — the tainted push stands in for the find.
        let packed = (3 << 16) | 1;
        let script = taint_script(vec![
            ("push_constant_int", Operand::Int(packed)),
            ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
            ("if_find", Operand::Byte(0)),
            ("pop_int_local", Operand::Local(0)),
            ("push_int_local", Operand::Local(0)),
            ("push_constant_int", Operand::Int(1)),
            ("branch_equals", Operand::Branch(10)),
            ("push_constant_string", Operand::Str("f".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("branch", Operand::Branch(12)),
            ("push_constant_string", Operand::Str("t".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
        ]);
        let scripts = BTreeMap::from([(1, script)]);
        let xref = build_xref(&scripts, &taint_components());
        let uses = &xref.script_to_comps[&1];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].pc, 11);
        assert_eq!(uses[0].target, ComponentRef { iface: 3, child: 1 });
        assert_eq!(xref.stats.cc_linked, 1);
        assert_eq!(xref.stats.cc_bails, 1);
    }

    #[test]
    fn taint_guard_reversed_operand_order_links() {
        // find; pop $l; push 1; push $l; branch_equals(T): same, mirrored.
        let packed = (3 << 16) | 1;
        let script = taint_script(vec![
            ("push_constant_int", Operand::Int(packed)),
            ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
            ("if_find", Operand::Byte(0)),
            ("pop_int_local", Operand::Local(0)),
            ("push_constant_int", Operand::Int(1)),
            ("push_int_local", Operand::Local(0)),
            ("branch_equals", Operand::Branch(10)),
            ("push_constant_string", Operand::Str("f".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("branch", Operand::Branch(12)),
            ("push_constant_string", Operand::Str("t".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
        ]);
        let scripts = BTreeMap::from([(1, script)]);
        let xref = build_xref(&scripts, &taint_components());
        let uses = &xref.script_to_comps[&1];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].pc, 11);
        assert_eq!(xref.stats.cc_linked, 1);
        assert_eq!(xref.stats.cc_bails, 1);
    }

    #[test]
    fn taint_branch_if_true_via_local() {
        // find; pop $l; push $l; branch_if_true(T): T links, fall-through
        // bails.
        let packed = (3 << 16) | 1;
        let script = taint_script(vec![
            ("push_constant_int", Operand::Int(packed)),
            ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
            ("if_find", Operand::Byte(0)),
            ("pop_int_local", Operand::Local(0)),
            ("push_int_local", Operand::Local(0)),
            ("branch_if_true", Operand::Branch(9)),
            ("push_constant_string", Operand::Str("f".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("branch", Operand::Branch(11)),
            ("push_constant_string", Operand::Str("t".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
        ]);
        let scripts = BTreeMap::from([(1, script)]);
        let xref = build_xref(&scripts, &taint_components());
        let uses = &xref.script_to_comps[&1];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].pc, 10);
        assert_eq!(xref.stats.cc_linked, 1);
        assert_eq!(xref.stats.cc_bails, 1);
    }

    #[test]
    fn taint_cleared_on_reassign() {
        // find; pop $l; …; pop $l (ordinary int): the taint dies with the
        // reassignment, so the later guard is not a guard at all — the
        // fall-through keeps pack scope and links.
        let packed = (3 << 16) | 1;
        let script = taint_script(vec![
            ("push_constant_int", Operand::Int(packed)),
            ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
            ("if_find", Operand::Byte(0)),
            ("pop_int_local", Operand::Local(0)),
            ("push_constant_int", Operand::Int(99)),
            ("pop_int_local", Operand::Local(0)),
            ("push_int_local", Operand::Local(0)),
            ("push_constant_int", Operand::Int(1)),
            ("branch_equals", Operand::Branch(12)),
            ("push_constant_string", Operand::Str("f".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
            ("push_constant_string", Operand::Str("t".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
        ]);
        let scripts = BTreeMap::from([(1, script)]);
        let xref = build_xref(&scripts, &taint_components());
        let uses = &xref.script_to_comps[&1];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].pc, 10);
        assert_eq!(xref.stats.cc_linked, 1);
        assert_eq!(xref.stats.cc_bails, 1);
    }

    #[test]
    fn taint_cleared_at_label() {
        // find; pop $l; branch over a label: reaching the guarded push
        // through the label merges unknown provenance, so the taint is gone
        // and no success scope is claimed.
        let packed = (3 << 16) | 1;
        let script = taint_script(vec![
            ("push_constant_int", Operand::Int(packed)),
            ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
            ("if_find", Operand::Byte(0)),
            ("pop_int_local", Operand::Local(0)),
            ("branch", Operand::Branch(6)),
            ("return", Operand::Byte(0)),
            ("push_int_local", Operand::Local(0)),
            ("push_constant_int", Operand::Int(1)),
            ("branch_equals", Operand::Branch(12)),
            ("push_constant_string", Operand::Str("f".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
            ("push_constant_string", Operand::Str("t".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
        ]);
        let scripts = BTreeMap::from([(1, script)]);
        let xref = build_xref(&scripts, &taint_components());
        assert!(!xref.script_to_comps.contains_key(&1));
        assert_eq!(xref.stats.cc_linked, 0);
        assert_eq!(xref.stats.cc_bails, 2);
    }

    #[test]
    fn ordinary_int_never_taints() {
        // A `$local` holding an ordinary int is not a find result: the
        // later compare triggers no failure-nulling, so the fall-through
        // keeps the live find scope and links.
        let packed = (3 << 16) | 1;
        let script = taint_script(vec![
            ("push_constant_int", Operand::Int(packed)),
            ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
            ("if_find", Operand::Byte(0)),
            ("push_constant_int", Operand::Int(5)),
            ("pop_int_local", Operand::Local(0)),
            ("push_int_local", Operand::Local(0)),
            ("push_constant_int", Operand::Int(1)),
            ("branch_equals", Operand::Branch(11)),
            ("push_constant_string", Operand::Str("f".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
            ("push_constant_string", Operand::Str("t".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
        ]);
        let scripts = BTreeMap::from([(1, script)]);
        let xref = build_xref(&scripts, &taint_components());
        let uses = &xref.script_to_comps[&1];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].pc, 9);
        assert_eq!(xref.stats.cc_linked, 1);
        assert_eq!(xref.stats.cc_bails, 1);
    }

    #[test]
    fn taint_dies_with_later_scope_write() {
        // findA packs scope, then findB rewrites the same slot: the taint
        // derived from findA dies with the rewrite, so the later guard
        // claims nothing — the fall-through links findB's component instead.
        let pack_a = (3 << 16) | 1;
        let pack_b = (3 << 16) | 2;
        let script = taint_script(vec![
            ("push_constant_int", Operand::Int(pack_a)),
            ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
            ("if_find", Operand::Byte(0)),
            ("pop_int_local", Operand::Local(0)),
            ("push_constant_int", Operand::Int(pack_b)),
            ("noopDisplayCommand", Operand::Byte(0)), // pad to retain branch PCs
            ("if_find", Operand::Byte(0)),
            ("push_int_local", Operand::Local(0)),
            ("push_constant_int", Operand::Int(1)),
            ("branch_equals", Operand::Branch(13)),
            ("push_constant_string", Operand::Str("f".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("branch", Operand::Branch(15)),
            ("push_constant_string", Operand::Str("t".to_string())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
        ]);
        let scripts = BTreeMap::from([(1, script)]);
        let mut components = BTreeMap::new();
        components.insert((3, 1), dummy_component());
        components.insert((3, 2), dummy_component());
        let xref = build_xref(&scripts, &components);
        let uses = &xref.script_to_comps[&1];
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].pc, 11);
        assert_eq!(uses[0].target, ComponentRef { iface: 3, child: 2 });
    }

    #[test]
    fn string_walk_recovers_adjacent_literal() {
        // push 7; push "ii"; push C; if_setonclick: the descriptor is the
        // object-stack top.
        let instructions = code(vec![
            ("push_constant_int", Operand::Int(7)),
            ("push_constant_string", Operand::Str("ii".to_string())),
            ("push_constant_int", Operand::Int(0x30007)),
            ("if_setonclick", Operand::Byte(0)),
        ]);
        assert_eq!(
            recover_string_operand(&instructions, 3, 0),
            Some("ii".to_string())
        );
    }

    #[test]
    fn string_walk_bails_across_calls_and_locals() {
        // A call between the descriptor push and the setter ends the run:
        // the callee may push or pop objects.
        let called = code(vec![
            ("push_constant_string", Operand::Str(String::new())),
            ("gosub_with_params", Operand::Script(99)),
            ("push_constant_int", Operand::Int(0x30007)),
            ("if_setontimer", Operand::Byte(0)),
        ]);
        assert_eq!(recover_string_operand(&called, 3, 0), None);
        // A `$local` string is not a literal: unknowable, never guessed.
        let local = code(vec![
            ("push_string_local", Operand::Local(0)),
            ("push_constant_int", Operand::Int(0x30007)),
            ("if_setontimer", Operand::Byte(0)),
        ]);
        assert_eq!(recover_string_operand(&local, 2, 0), None);
    }

    #[test]
    fn hook_depth_counts_int_tails() {
        assert_eq!(hook_script_depth(""), Some(1));
        assert_eq!(hook_script_depth("i"), Some(2));
        assert_eq!(hook_script_depth("ii"), Some(3));
        // `s` tails ride the object stack, `l` the long stack: neither moves
        // the int depth.
        assert_eq!(hook_script_depth("sii"), Some(3));
        assert_eq!(hook_script_depth("si"), Some(2));
        assert_eq!(hook_script_depth("li"), Some(2));
        // A trailing `Y` means a runtime-sized int array above the tails.
        assert_eq!(hook_script_depth("Y"), None);
        assert_eq!(hook_script_depth("iY"), None);
        assert_eq!(hook_script_depth("iiiY"), None);
    }

    /// A setter caller: `push <script>; [tails…;] push <"desc">; push <comp>;
    /// if_seton*`, with the callee script present in the roster.
    fn setter_fixture(
        installed: i32,
        descriptor: &str,
        tails: Vec<i32>,
        command: &str,
    ) -> (ScriptMap, ComponentMap) {
        let mut pairs = vec![("push_constant_int", Operand::Int(installed))];
        for tail in tails {
            pairs.push(("push_constant_int", Operand::Int(tail)));
        }
        pairs.push(("push_constant_string", Operand::Str(descriptor.to_string())));
        pairs.push(("push_constant_int", Operand::Int((3 << 16) | 1)));
        pairs.push((command, Operand::Byte(0)));
        let calling = script_with(0, 0, pairs);
        let target_script = script_with(0, 0, Vec::new());
        let scripts = BTreeMap::from([(1, calling), (installed, target_script)]);
        let components = taint_components();
        (scripts, components)
    }

    #[test]
    fn hook_set_empty_descriptor_links() {
        let (scripts, components) = setter_fixture(500, "", Vec::new(), "if_setontimer");
        let xref = build_xref(&scripts, &components);
        assert_eq!(xref.stats.set_sites, 1);
        assert_eq!(xref.stats.set_linked, 1);
        let sets = &xref.hook_sets[&1];
        assert_eq!(sets.len(), 1);
        assert_eq!(
            sets[0],
            HookSet {
                caller: 1,
                pc: 3,
                command: "if_setontimer".to_string(),
                callee: 500,
                target: ComponentRef { iface: 3, child: 1 },
            }
        );
        // The component touch records through the table path as well.
        assert!(xref.script_to_comps[&1].iter().any(|touch| touch.pc == 3));
        // And the installing line renders in the script view.
        let rendered = format_script(1, &xref, &SymbolRegistry::empty());
        assert!(
            rendered.contains("sets 500 as ontimer of 3/1"),
            "{rendered}"
        );
    }

    #[test]
    fn hook_set_tailed_descriptor_reaches_depth() {
        // "ii": the script id sits two tails below the component.
        let (scripts, components) = setter_fixture(500, "ii", vec![11, 22], "if_setonclick");
        let xref = build_xref(&scripts, &components);
        assert_eq!(xref.stats.set_linked, 1);
        let sets = &xref.hook_sets[&1];
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].callee, 500);
        assert_eq!(sets[0].pc, 5);
        assert_eq!(sets[0].target, ComponentRef { iface: 3, child: 1 });
    }

    #[test]
    fn hook_set_y_descriptor_bails() {
        let (scripts, components) = setter_fixture(500, "iY", vec![11], "if_setonclick");
        let xref = build_xref(&scripts, &components);
        assert_eq!(xref.stats.set_sites, 1);
        assert_eq!(xref.stats.set_linked, 0);
        assert_eq!(xref.stats.set_bails, 1);
        assert!(!xref.hook_sets.contains_key(&1));
        assert!(xref.hook_dangling.is_empty());
    }

    #[test]
    fn cfg_hook_recovery_handles_transmits_and_rejects_unknown_counts() {
        const CALLER: i32 = 1;
        const CALLEE: i32 = 500;
        const TAIL: i32 = 11;
        const FIRST_TRANSMIT: i32 = 22;
        const SECOND_TRANSMIT: i32 = 33;
        const EMPTY_COUNT: i32 = 0;
        const PAIR_COUNT: i32 = 2;
        const ARGUMENT_SLOT: i32 = 0;
        const ONE_ARGUMENT: u16 = 1;
        for tails in [
            vec![TAIL, FIRST_TRANSMIT, SECOND_TRANSMIT, PAIR_COUNT],
            // A null transmit array leaves Y in the argument descriptor.
            vec![TAIL, FIRST_TRANSMIT, EMPTY_COUNT],
        ] {
            let (mut scripts, components) = setter_fixture(CALLEE, "iY", tails, "if_setonclick");
            for script in scripts.values_mut() {
                script
                    .code
                    .extend(code(vec![("return", Operand::Byte(u8::default()))]));
            }
            let configs = crate::config::ConfigTypes::empty();
            let xref = build_xref_with_configs(&scripts, &components, &configs);
            assert_eq!(xref.hook_sets[&CALLER].first().unwrap().callee, CALLEE);
            assert!(
                format_script(CALLEE, &xref, &SymbolRegistry::empty())
                    .contains("installed by script")
            );
            let caller = scripts.get_mut(&CALLER).unwrap();
            caller.args.int = ONE_ARGUMENT;
            caller.locals.int = ONE_ARGUMENT;
            // Select the count by its position just before the descriptor.
            let descriptor_pc = caller
                .code
                .iter()
                .position(|instruction| instruction.operand == Operand::Str("iY".into()))
                .unwrap();
            let count_pc = descriptor_pc.checked_sub(ONE_ARGUMENT.into()).unwrap();
            caller.code[count_pc] = code(vec![("push_int_local", Operand::Local(ARGUMENT_SLOT))])
                .remove(usize::default());
            let xref = build_xref_with_configs(&scripts, &components, &configs);
            assert!(!xref.hook_sets.contains_key(&CALLER));
            assert!(xref.hook_dangling.is_empty());
        }
    }

    #[test]
    fn cfg_hook_recovery_requires_agreement_and_complete_analysis() {
        const CALLER: i32 = 1;
        const CALLEE: i32 = 500;
        const OTHER_CALLEE: i32 = 501;
        const ARGUMENT_SLOT: i32 = 0;
        const RIGHT_BRANCH: i32 = 4;
        const MERGE: i32 = 5;
        const ONE_ARGUMENT: u16 = 1;
        let components = taint_components();
        let (&(iface, child), _) = components.first_key_value().unwrap();
        let packed = pack_component(ComponentRef {
            iface,
            child: child.try_into().unwrap(),
        })
        .unwrap();
        for right in [CALLEE, OTHER_CALLEE] {
            let mut caller = script_with(
                ONE_ARGUMENT,
                u16::default(),
                vec![
                    ("push_int_local", Operand::Local(ARGUMENT_SLOT)),
                    ("branch_if_true", Operand::Branch(RIGHT_BRANCH)),
                    ("push_constant_int", Operand::Int(CALLEE)),
                    ("branch", Operand::Branch(MERGE)),
                    ("push_constant_int", Operand::Int(right)),
                    ("push_constant_string", Operand::Str(String::new())),
                    ("push_constant_int", Operand::Int(packed)),
                    ("if_setontimer", Operand::Byte(u8::default())),
                    ("return", Operand::Byte(u8::default())),
                ],
            );
            caller.locals.int = ONE_ARGUMENT;
            let target_body = script_with(
                u16::default(),
                u16::default(),
                vec![("return", Operand::Byte(u8::default()))],
            );
            let mut scripts = BTreeMap::from([
                (CALLER, caller),
                (CALLEE, target_body.clone()),
                (OTHER_CALLEE, target_body),
            ]);
            let configs = crate::config::ConfigTypes::empty();
            let xref = build_xref_with_configs(&scripts, &components, &configs);
            assert_eq!(xref.hook_sets.contains_key(&CALLER), right == CALLEE);
            scripts.get_mut(&CALLER).unwrap().code.pop();
            scripts.get_mut(&CALLER).unwrap().code.push(Instruction {
                opcode: u16::default(),
                command: "unknown_test_command".into(),
                operand: Operand::Byte(u8::default()),
            });
            let xref = build_xref_with_configs(&scripts, &components, &configs);
            assert!(!xref.hook_sets.contains_key(&CALLER));
        }
    }

    #[test]
    fn active_hook_installation_retains_creator_identity_and_excludes_discarded_hooks() {
        const CALLER: i32 = 1;
        const CALLEE: i32 = 500;
        const FIRST_CHILD: i32 = 0;
        let components = taint_components();
        let (&(iface, file), _) = components.first_key_value().unwrap();
        let parent = pack_component(ComponentRef {
            iface,
            child: file.try_into().unwrap(),
        })
        .unwrap();
        let kind = i32::from(crate::isource::ComponentType::Text.type_id());
        for command in ["cc_setontimer", "cc_setonverticalswipe"] {
            let caller = script_with(
                u16::default(),
                u16::default(),
                vec![
                    ("push_constant_int", Operand::Int(parent)),
                    ("push_constant_int", Operand::Int(kind)),
                    ("push_constant_int", Operand::Int(FIRST_CHILD)),
                    ("cc_create", Operand::Byte(u8::default())),
                    ("push_constant_int", Operand::Int(CALLEE)),
                    ("push_constant_string", Operand::Str(String::new())),
                    (command, Operand::Byte(u8::default())),
                    ("return", Operand::Byte(u8::default())),
                ],
            );
            let target = script_with(
                u16::default(),
                u16::default(),
                vec![("return", Operand::Byte(u8::default()))],
            );
            let xref = build_xref_with_configs(
                &BTreeMap::from([(CALLER, caller), (CALLEE, target)]),
                &components,
                &crate::config::ConfigTypes::empty(),
            );
            assert_eq!(
                xref.dynamic_hook_sets.contains_key(&CALLER),
                command == "cc_setontimer"
            );
            if let Some(hooks) = xref.dynamic_hook_sets.get(&CALLER) {
                let hook = hooks.first().unwrap();
                assert_eq!(hook.creator_pc, xref.creations[&CALLER].first().unwrap().pc);
                assert_eq!(hook.callee, CALLEE);
                assert!(
                    format_script(CALLEE, &xref, &SymbolRegistry::empty())
                        .contains("runtime child created")
                );
            }
        }
    }

    #[test]
    fn hook_set_unknown_and_cleared_scripts_dangle() {
        // No callee script in the roster: dead hook, recorded.
        let caller = script_with(
            0,
            0,
            vec![
                ("push_constant_int", Operand::Int(9999)),
                ("push_constant_string", Operand::Str(String::new())),
                ("push_constant_int", Operand::Int((3 << 16) | 1)),
                ("if_setontimer", Operand::Byte(0)),
            ],
        );
        let scripts = BTreeMap::from([(1, caller)]);
        let xref = build_xref(&scripts, &taint_components());
        assert!(!xref.hook_sets.contains_key(&1));
        assert_eq!(xref.stats.set_dangling, 1);
        assert_eq!(
            xref.hook_dangling,
            vec![DanglingUse {
                script: 1,
                pc: 3,
                command: "if_setontimer".to_string(),
                value: 9999,
            }]
        );
        // `-1` hook clears record the same way.
        let cleared = script_with(
            0,
            0,
            vec![
                ("push_constant_int", Operand::Int(-1)),
                ("push_constant_string", Operand::Str(String::new())),
                ("push_constant_int", Operand::Int((3 << 16) | 1)),
                ("if_setontimer", Operand::Byte(0)),
            ],
        );
        let scripts = BTreeMap::from([(1, cleared)]);
        let xref = build_xref(&scripts, &taint_components());
        assert_eq!(xref.stats.set_dangling, 1);
        assert_eq!(xref.hook_dangling[0].value, -1);
    }

    #[test]
    fn hook_set_discard_commands_stay_out() {
        // `if_setonverticalswipe` pops its hook args and installs nothing:
        // literals everywhere, still no edge and no dangling — while the
        // component touch records normally through the table path.
        let (scripts, components) = setter_fixture(500, "", Vec::new(), "if_setonverticalswipe");
        let xref = build_xref(&scripts, &components);
        assert_eq!(xref.stats.set_sites, 0);
        assert!(!xref.hook_sets.contains_key(&1));
        assert!(xref.hook_dangling.is_empty());
        assert!(xref.script_to_comps[&1].iter().any(|touch| touch.pc == 3));
    }

    #[test]
    fn hook_set_unprovable_descriptor_bails() {
        // The descriptor push sits behind a call: the object walk ends
        // there, so the site bails with no edge either way.
        let caller = script_with(
            0,
            0,
            vec![
                ("push_constant_int", Operand::Int(500)),
                ("push_constant_string", Operand::Str(String::new())),
                ("gosub_with_params", Operand::Script(99)),
                ("push_constant_int", Operand::Int((3 << 16) | 1)),
                ("if_setontimer", Operand::Byte(0)),
            ],
        );
        let installed = script_with(0, 0, Vec::new());
        let scripts = BTreeMap::from([(1, caller), (500, installed)]);
        let xref = build_xref(&scripts, &taint_components());
        assert_eq!(xref.stats.set_sites, 1);
        assert_eq!(xref.stats.set_bails, 1);
        assert!(!xref.hook_sets.contains_key(&1));
        assert!(xref.hook_dangling.is_empty());
    }
    #[test]
    fn cc_find_minus_one_does_not_claim_pack_scope() {
        let packed = (3 << 16) | 1;
        let script = taint_script(vec![
            ("push_constant_int", Operand::Int(packed)),
            ("push_constant_int", Operand::Int(-1)),
            ("cc_find", Operand::Byte(0)),
            ("push_constant_string", Operand::Str("text".into())),
            ("cc_settext", Operand::Byte(0)),
            ("return", Operand::Byte(0)),
        ]);
        let result = build_xref(&BTreeMap::from([(1, script)]), &taint_components());
        assert!(!result.script_to_comps.contains_key(&1));
        assert_eq!(result.stats.finds_pack, 0);
    }
}
