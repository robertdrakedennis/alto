//! Verified revision 910 instruction contracts. Fixed stack traffic is not a
//! purity claim. Unknown commands remain unknown, including preview-only rows.
//! Evidence: the original client's handler for each command; control operands
//! consume their lanes before either successor.

use crate::script::{Operand, is_branch_command};

/// A command's stack effect class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    /// Fixed traffic: pops then pushes exactly these `(int, obj, long)`
    /// counts, regardless of path or operand values.
    Fixed {
        /// Values popped per stack.
        pops: [u16; 3],
        /// Values pushed per stack.
        pushes: [u16; 3],
    },
    /// `gosub_with_params`: pops the callee's declared argument counts off
    /// the stacks; pushes whatever the callee returns (return inference).
    Call,
    /// Branches, `switch`, `return`: control flow, not straight-line stack
    /// traffic. (`return` additionally ends every path through it.)
    ControlFlow { pops: [u16; 3] },
    /// Operand- or callee-dependent, or simply unanalyzed: no static claim.
    Unknown,
    /// The retail dispatcher unconditionally raises an error.
    Trap,
    /// Normal stack traffic depends on runtime state, with explicit cases.
    Conditional { cases: &'static [EffectCase] },
}

/// One evidenced normal-completion branch of a runtime-dependent command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectCase {
    pub condition: &'static str,
    pub pops: [u16; 3],
    pub pushes: [u16; 3],
}
const WORLDLIST_CASES: &[EffectCase] = &[
    EffectCase {
        condition: "client state == 13",
        pops: [1, 0, 0],
        pushes: [0; 3],
    },
    EffectCase {
        condition: "client state != 13",
        pops: [0; 3],
        pushes: [0; 3],
    },
];
const VIDEO_CASES: &[EffectCase] = &[
    EffectCase {
        condition: "JavaScript enabled and the argument read is reached",
        pops: [1, 0, 0],
        pushes: [1, 0, 0],
    },
    EffectCase {
        condition: "JavaScript disabled, or a caught exception before the argument read",
        pops: [0; 3],
        pushes: [1, 0, 0],
    },
];

/// The static effect of one instruction. `push_constant_string` dispatches on
/// its operand variant (the tag selects the pushed type); everything else is
/// per command.
#[must_use]
pub fn effect(command: &str, operand: &Operand) -> Effect {
    if matches!(command, "push_constant_string" | "join_string") {
        return hand_checked_effect(command, operand);
    }
    match hand_checked_effect(command, operand) {
        Effect::Unknown => crate::cs2_stack_contracts::effect(command).unwrap_or(Effect::Unknown),
        known => known,
    }
}

fn hand_checked_effect(command: &str, operand: &Operand) -> Effect {
    if matches!(
        command,
        "retail_trap_79"
            | "retail_trap_103"
            | "retail_trap_247"
            | "retail_trap_250"
            | "retail_trap_393"
            | "worldmap_3dview_enable"
            | "worldmap_3dview_disable"
            | "worldmap_3dview_getcoordfine"
            | "worldmap_3dview_setlighting"
            | "worldmap_3dview_setloddistance"
    ) {
        return Effect::Trap;
    }
    // Pops/pushes below mirror the handler bodies 1:1 (each cited).
    match command {
        // Pops nothing, pushes one typed value.
        // `disabled_command_1253`: empty handler.
        "noopDisplayCommand" => fixed([0, 0, 0], [0, 0, 0]),
        "push_constant_int" => fixed([0, 0, 0], [1, 0, 0]),
        "push_long_constant" => fixed([0, 0, 0], [0, 0, 1]),
        "push_constant_string" => match operand {
            Operand::Int(_) => fixed([0, 0, 0], [1, 0, 0]),
            Operand::Long(_) => fixed([0, 0, 0], [0, 0, 1]),
            Operand::Str(_) => fixed([0, 0, 0], [0, 1, 0]),
            _ => Effect::Unknown,
        },
        "push_int_local" => fixed([0, 0, 0], [1, 0, 0]),
        "push_string_local" => fixed([0, 0, 0], [0, 1, 0]),
        "push_long_local" => fixed([0, 0, 0], [0, 0, 1]),
        // Varbits are int-valued by construction (`push_varbit` pushes
        // `intStack`; `pop_varbit` pops it).
        "push_varbit" => fixed([0, 0, 0], [1, 0, 0]),
        // Locals and varbits pop one typed value into their slot.
        "pop_int_local" => fixed([1, 0, 0], [0, 0, 0]),
        "pop_string_local" => fixed([0, 1, 0], [0, 0, 0]),
        "pop_long_local" => fixed([0, 0, 1], [0, 0, 0]),
        "pop_varbit" => fixed([1, 0, 0], [0, 0, 0]),
        // Binary int arithmetic and bit ops (`add`, `multiply`, `divide`,
        // `modulo`, `addpercent`, `setbit`, `clearbit`, `testbit`, `and`,
        // `or`, `min`, `max`, `pow`): pop two ints, push one int, regardless
        // of internal branches (`pow`).
        "add" | "multiply" | "divide" | "modulo" | "addpercent" | "setbit" | "clearbit"
        | "testbit" | "and" | "or" | "min" | "max" | "pow" => fixed([2, 0, 0], [1, 0, 0]),
        // Subtraction spelled as a quickchat command
        // (`quickchat_dynamic_command_add` pops two ints and pushes their
        // difference): still 2 int -> 1 int, whatever the name suggests.
        "quickchat_dynamic_command_add" => fixed([2, 0, 0], [1, 0, 0]),
        // String concatenation (`append` pops two strings and pushes their
        // concatenation): 2 obj -> 1 obj. The mixed-lane siblings have separate rows below.
        "append" => fixed([0, 2, 0], [0, 1, 0]),
        // Unary int producers (`not`, `random`, `randominc`).
        "not" | "random" | "randominc" => fixed([1, 0, 0], [1, 0, 0]),
        // Ternary and quinary int producers (`scale`, `interpolate`).
        "scale" => fixed([3, 0, 0], [1, 0, 0]),
        "interpolate" => fixed([5, 0, 0], [1, 0, 0]),
        // String comparison (`compare`): pops two objects, pushes one int.
        "compare" => fixed([0, 2, 0], [1, 0, 0]),
        // String length (`string_length`): pops one object, pushes one int.
        "string_length" => fixed([0, 1, 0], [1, 0, 0]),
        // String joining (`join_string`): pops its `intOperands[pc]` count off
        // the OBJECT stack — every part is an object (one string write of the
        // in-order concatenation) — pushes one object. The count rides in the operand,
        // so the shape is per-count: any non-negative count that fits the
        // triple is `Fixed` (counts 0/1 are handler-valid too — `""` and the
        // identity copy — the expression layer, not this table, restricts
        // surface form to 2+). Negative counts (the handler would drive `osp`
        // backwards) and counts wider than `u16` (inexpressible in the triple)
        // are `Unknown`: no claim, never a guess.
        "join_string" => match operand {
            Operand::Count(count) | Operand::Int(count) => match u16::try_from(*count) {
                Ok(narrow) => fixed([0, narrow, 0], [0, 1, 0]),
                Err(_) => Effect::Unknown,
            },
            _ => Effect::Unknown,
        },
        // Int to string (`tostring` pops one int and pushes its decimal
        // text): 1 int -> 1 obj.
        "tostring" => fixed([1, 0, 0], [0, 1, 0]),
        // Component access: `if_gettext`/`if_getlayer` and the `cc_` variants.
        "cc_find" => fixed([2, 0, 0], [1, 0, 0]),
        "if_find" => fixed([1, 0, 0], [1, 0, 0]),
        "if_gettext" => fixed([1, 0, 0], [0, 1, 0]),
        "cc_gettext" => fixed([0, 0, 0], [0, 1, 0]),
        "if_getlayer" => fixed([1, 0, 0], [1, 0, 0]),
        "cc_getlayer" => fixed([0, 0, 0], [1, 0, 0]),
        // `cc_settext` and the explicit-component wrapper.
        "if_settext" => fixed([1, 1, 0], [0, 0, 0]),
        "cc_settext" => fixed([0, 1, 0], [0, 0, 0]),
        // Arithmetic, string and array handlers; normal completion traffic.
        "invpow" | "togglebit" | "atan2_deg" => fixed([2, 0, 0], [1, 0, 0]),
        "sqrt"
        | "abs"
        | "bitcount"
        | "sin_deg"
        | "cos_deg"
        | "char_isprintable"
        | "char_isvalid"
        | "char_isalphanumeric"
        | "char_isalpha"
        | "char_isnumeric"
        | "char_tolowercase" => fixed([1, 0, 0], [1, 0, 0]),
        "setbit_range" | "clearbit_range" | "getbit_range" => fixed([3, 0, 0], [1, 0, 0]),
        "setbit_range_toint" => fixed([4, 0, 0], [1, 0, 0]),
        "append_num" | "append_signnum" | "append_char" => fixed([1, 1, 0], [0, 1, 0]),
        "substring" => fixed([2, 1, 0], [0, 1, 0]),
        "string_indexof_char" => fixed([2, 1, 0], [1, 0, 0]),
        "string_indexof_string" => fixed([1, 2, 0], [1, 0, 0]),
        "removetags" => fixed([0, 1, 0], [0, 1, 0]),
        "define_array" | "discard_int" => fixed([1, 0, 0], [0, 0, 0]),
        "discard_string" => fixed([0, 1, 0], [0, 0, 0]),
        "discard_long" => fixed([0, 0, 1], [0, 0, 0]),
        "push_array_int" => fixed([1, 0, 0], [1, 0, 0]),
        "push_array_int_leave_index_on_stack" | "push_array_int_and_index" => {
            fixed([1, 0, 0], [2, 0, 0])
        }
        "pop_array_int" => fixed([2, 0, 0], [0, 0, 0]),
        "pop_array_int_leave_value_on_stack" => fixed([2, 0, 0], [1, 0, 0]),
        // Component wrappers and their shared `cc_`/`if_` handlers.
        "cc_sethide" | "cc_setcolour" | "cc_setgraphic" | "cc_settrans" | "cc_setfill"
        | "cc_setlinewid" | "cc_deleteall" => fixed([1, 0, 0], [0; 3]),
        "if_sethide" | "if_setcolour" | "if_setgraphic" | "if_settrans" | "if_setfill"
        | "if_setlinewid" => fixed([2, 0, 0], [0; 3]),
        "cc_setposition" | "cc_setsize" => fixed([4, 0, 0], [0; 3]),
        "if_setposition" | "if_setsize" => fixed([5, 0, 0], [0; 3]),
        "cc_create" => fixed([3, 0, 0], [0; 3]),
        "cc_delete" => fixed([0; 3], [0; 3]),
        "if_getwidth" | "if_getheight" | "if_gethide" | "if_getnextsubid" => {
            fixed([1, 0, 0], [1, 0, 0])
        }
        "clientclock" | "map_lang" => fixed([0; 3], [1, 0, 0]),
        "enum_getoutputcount"
        | "stat"
        | "stat_base"
        | "stat_visible_xp"
        | "stat_base_actual"
        | "inv_size" => fixed([1, 0, 0], [1, 0, 0]),
        "inv_getobj" | "inv_getnum" | "inv_total" | "inv_totalcat" | "inv_stockbase" => {
            fixed([2, 0, 0], [1, 0, 0])
        }
        "inv_getvar" => fixed([3, 0, 0], [1, 0, 0]),
        "if_gettrans" | "if_getx" | "if_gety" | "if_getgraphic" | "if_getparentlayer" => {
            fixed([1, 0, 0], [1, 0, 0])
        }
        "cc_getx" | "cc_getinvobject" | "cc_getwidth" => fixed([0; 3], [1, 0, 0]),
        "cc_setmodel" | "cc_settextfont" | "cc_setmodelanim" => fixed([1, 0, 0], [0; 3]),
        "if_setmodel" | "if_settextfont" | "if_setmodelanim" | "cc_setscrollsize" => {
            fixed([2, 0, 0], [0; 3])
        }
        "if_setscrollsize" => fixed([3, 0, 0], [0; 3]),
        "cc_setop" => fixed([1, 1, 0], [0; 3]),
        "if_setop" => fixed([2, 1, 0], [0; 3]),
        "sound_vorbis_volume" => fixed([4, 0, 0], [0; 3]),
        "sound_synth" => fixed([3, 0, 0], [0; 3]),
        "mes" => fixed([0, 1, 0], [0; 3]),
        "map_members" | "coord" | "date_runeday" | "has_nxt" => fixed([0; 3], [1, 0, 0]),
        "tostring_localised" => fixed([2, 0, 0], [0, 1, 0]),
        "oc_name" => fixed([1, 0, 0], [0, 1, 0]),
        "paraheight" => fixed([2, 1, 0], [1, 0, 0]),
        "chat_playername_unfiltered" => fixed([0; 3], [0, 1, 0]),
        "cc_setparam" | "cc_setscrollpos" => fixed([2, 0, 0], [0; 3]),
        "cc_settextalign" | "if_setscrollpos" => fixed([3, 0, 0], [0; 3]),
        "if_settextalign" => fixed([4, 0, 0], [0; 3]),
        "cc_gety" | "cc_getheight" | "cc_getgraphic" | "date_minutes" | "gender" => {
            fixed([0; 3], [1, 0, 0])
        }
        "if_getscrolly" | "if_getfontmetrics" | "coordx" => fixed([1, 0, 0], [1, 0, 0]),
        "enum_hasoutput" => fixed([3, 0, 0], [1, 0, 0]),
        "sound_vorbis_rate" => fixed([5, 0, 0], [0; 3]),
        "stringwidth" => fixed([1, 1, 0], [1, 0, 0]),
        "cc_settextshadow" => fixed([1, 0, 0], [0; 3]),
        "if_settextshadow" => fixed([2, 0, 0], [0; 3]),
        // Dependency-ranked closure: named handlers and DB dispatch
        // cases 593 (count) / 847 (no count). db_listall is deliberately absent:
        // its null-result path does not push a count.
        "if_gettop"
        | "clienttype"
        | "fullscreen_modecount"
        | "targetmode_active"
        | "db_findnext" => fixed([0; 3], [1, 0, 0]),
        "cancel_interface_drag" => fixed([0; 3], [0; 3]),
        "enum_getreversecount" => fixed([3, 0, 0], [1, 0, 0]),
        "coordz" | "db_getrowtable" | "db_find_get" => fixed([1, 0, 0], [1, 0, 0]),
        "inv_totalparam" | "db_getfieldcount" | "db_find_with_count" | "db_find_refine" => {
            fixed([2, 0, 0], [1, 0, 0])
        }
        "db_find" => fixed([2, 0, 0], [0; 3]),
        "cc_setobject" | "cc_setobject_nonum" => fixed([2, 0, 0], [0; 3]),
        "if_setobject" | "if_setobject_nonum" => fixed([3, 0, 0], [0; 3]),
        // Opcode 1431 dispatches cc_getparentlayer; 1166 is disabled_command_1166.
        "getparentlayer_alias"
        | "cc_getparentlayer"
        | "cc_getfontmetrics"
        | "cc_gettrans"
        | "cc_getid" => fixed([0; 3], [1, 0, 0]),
        "pushZeroInsets" => fixed([0; 3], [4, 0, 0]),
        "if_getscrollheight" | "if_getscrollwidth" | "quest_finished" => {
            fixed([1, 0, 0], [1, 0, 0])
        }
        "if_setparam_int" => fixed([3, 0, 0], [0; 3]),
        "fullscreen_getmode" => fixed([1, 0, 0], [2, 0, 0]),
        "if_setnoclickthrough" => fixed([2, 0, 0], [0; 3]),
        "cc_setopkey" => fixed([11, 0, 0], [0; 3]),
        "pop_int_discard" => fixed([1, 0, 0], [0; 3]),
        "pop_string_discard" => fixed([0, 1, 0], [0; 3]),
        "pop_long_discard" => fixed([0, 0, 1], [0; 3]),
        "if_getscrollx" | "if_hassub" | "quest_started" => fixed([1, 0, 0], [1, 0, 0]),
        "if_opensubclient" | "cc_setparam_int" | "cc_setopcursor" => fixed([2, 0, 0], [0; 3]),
        "if_clearops" | "cc_settiling" | "cc_setopkeyignoreheld" => fixed([1, 0, 0], [0; 3]),
        "targetmode_cancel" => fixed([0; 3], [0; 3]),
        "movecoord" => fixed([4, 0, 0], [1, 0, 0]),
        "enum_getreverseindex" => fixed([5, 0, 0], [1, 0, 0]),
        "get_mousex" | "get_mousey" | "staffmodlevel" | "lobby_enterlobbyreply" => {
            fixed([0; 3], [1, 0, 0])
        }
        "if_closesubclient" | "if_clearscripthooks" | "cc_sethflip" => fixed([1, 0, 0], [0; 3]),
        "if_close" => fixed([0; 3], [0; 3]),
        "if_setnpcmodel" => fixed([2, 0, 0], [0; 3]),
        "parawidth" => fixed([2, 1, 0], [1, 0, 0]),
        "cc_setdraggable" => fixed([2, 0, 0], [0; 3]),
        "if_setopcursor" => fixed([3, 0, 0], [0; 3]),
        "playermember" => fixed([0; 3], [1, 0, 0]),
        "quest_getname" => fixed([1, 0, 0], [0, 1, 0]),
        "cc_setpausetext" => fixed([0, 1, 0], [0; 3]),
        "cc_setmouseovercursor" => fixed([1, 0, 0], [0; 3]),
        "if_hassuboverlay" => fixed([2, 0, 0], [1, 0, 0]),
        "if_sendtoback" => fixed([1, 0, 0], [0; 3]),
        // cc_if_setlink consumes a member index before its group-kind switch.
        "cc_setlinkfriend" | "cc_setlinkfriendchat" | "cc_setlinkactiveclanchannel" => {
            fixed([1, 0, 0], [0; 3])
        }
        "if_setlinkfriend"
        | "if_setlinkfriendchat"
        | "if_setlinkactiveclanchannel"
        | "cc_setlinkplayergroup" => fixed([2, 0, 0], [0; 3]),
        "if_setlinkplayergroup" => fixed([3, 0, 0], [0; 3]),
        "getclipboard" => fixed([0; 3], [0, 1, 0]),
        "baseidkit" | "movescripted" => fixed([2, 0, 0], [0; 3]),
        "video_advert_has_finished" => fixed([0; 3], [1, 0, 0]),
        "worldlist_pingworlds" => Effect::Conditional {
            cases: WORLDLIST_CASES,
        },
        "video_advert_play" => Effect::Conditional { cases: VIDEO_CASES },
        "gosub_with_params" => Effect::Call,
        "switch" | "return" => Effect::ControlFlow {
            pops: control_pops(command),
        },
        command if is_branch_command(command) => Effect::ControlFlow {
            pops: control_pops(command),
        },
        _ => Effect::Unknown,
    }
}

const fn fixed(pops: [u16; 3], pushes: [u16; 3]) -> Effect {
    Effect::Fixed { pops, pushes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::Operand;

    #[test]
    fn pure_rows_cover_pushes_pops_and_operators() {
        assert_eq!(
            effect("push_constant_int", &Operand::Int(0)),
            Effect::Fixed {
                pops: [0, 0, 0],
                pushes: [1, 0, 0]
            }
        );
        assert_eq!(
            effect("push_constant_string", &Operand::Str(String::new())),
            Effect::Fixed {
                pops: [0, 0, 0],
                pushes: [0, 1, 0]
            }
        );
        assert_eq!(
            effect("add", &Operand::Byte(0)),
            Effect::Fixed {
                pops: [2, 0, 0],
                pushes: [1, 0, 0]
            }
        );
        assert_eq!(
            effect("compare", &Operand::Byte(0)),
            Effect::Fixed {
                pops: [0, 2, 0],
                pushes: [1, 0, 0]
            }
        );
        assert_eq!(
            effect("gosub_with_params", &Operand::Script(0)),
            Effect::Call
        );
        assert_eq!(
            effect("branch_if_false", &Operand::Branch(0)),
            Effect::ControlFlow { pops: [1, 0, 0] }
        );
        // Operand-dependent or unanalyzed: no claim.
        assert_eq!(effect("push_var", &Operand::Byte(0)), Effect::Unknown);
        assert_eq!(
            effect("unverified_synthetic", &Operand::Byte(0)),
            Effect::Unknown
        );
        // `join_string` states one homogeneous row per count (degenerate
        // counts included — the expression layer restricts those, not this
        // table); negative and triple-overflowing counts stay `Unknown`.
        assert_eq!(
            effect("join_string", &Operand::Count(0)),
            Effect::Fixed {
                pops: [0, 0, 0],
                pushes: [0, 1, 0]
            }
        );
        assert_eq!(
            effect("join_string", &Operand::Count(2)),
            Effect::Fixed {
                pops: [0, 2, 0],
                pushes: [0, 1, 0]
            }
        );
        assert_eq!(
            effect("join_string", &Operand::Int(3)),
            Effect::Fixed {
                pops: [0, 3, 0],
                pushes: [0, 1, 0]
            }
        );
        assert_eq!(effect("join_string", &Operand::Count(-1)), Effect::Unknown);
        assert_eq!(effect("join_string", &Operand::Byte(0)), Effect::Unknown);
    }
}

/// Stack operands consumed by control flow (branch and switch handlers).
fn control_pops(command: &str) -> [u16; 3] {
    match command {
        "return" | "branch" => [0, 0, 0],
        "switch" | "branch_if_true" | "branch_if_false" => [1, 0, 0],
        name if name.starts_with("long_branch_") => [0, 0, 2],
        _ => [2, 0, 0],
    }
}

/// Independent transformation property; a known stack shape is insufficient.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateEffect {
    /// Depends only on inputs, but can still fail (e.g. division by zero).
    InputsOnly,
    /// Reads or writes local storage.
    Local,
    /// Reads or writes host state, including variables.
    Host,
    /// Randomness must not be duplicated/reordered by expression recovery.
    Nondeterministic,
    /// Control flow or unresolved behavior.
    Unknown,
}

/// Conservatively classify the established contract subset.
pub fn state_effect(command: &str, operand: &Operand) -> StateEffect {
    if matches!(command, "random" | "randominc") {
        StateEffect::Nondeterministic
    } else if command.ends_with("_local") || command.contains("array") {
        StateEffect::Local
    } else if matches!(
        command,
        "clientclock"
            | "map_lang"
            | "enum_getoutputcount"
            | "stat"
            | "stat_base"
            | "stat_visible_xp"
            | "stat_base_actual"
    ) || command.starts_with("db_")
        || command.starts_with("quest_")
        || command.starts_with("inv_")
        || command.starts_with("if_")
        || command.starts_with("cc_")
        || matches!(
            command,
            "push_var" | "pop_var" | "push_varbit" | "pop_varbit"
        )
    {
        StateEffect::Host
    } else if matches!(effect(command, operand), Effect::Fixed { .. }) {
        if execution_family(command) == ExecutionFamily::HostRequired {
            StateEffect::Host
        } else {
            StateEffect::InputsOnly
        }
    } else {
        StateEffect::Unknown
    }
}

/// An encoded command may still have unknown semantics.
#[derive(Clone, Debug)]
pub struct CommandContract<'a> {
    pub name: &'a str,
    pub opcode: u16,
    pub large_operand: bool,
    pub retail_dispatch: bool,
    pub effect: Effect,
    pub type_rule: TypeRule,
    pub execution: ExecutionFamily,
    pub state: StateEffect,
    pub evidence: &'static str,
}

/// Join encoding and semantic facts. Synthetic encodings do not gain contracts.
pub fn contract<'a>(
    book: &'a crate::opcode::OpcodeBook,
    name: &str,
    operand: &Operand,
) -> crate::error::Result<CommandContract<'a>> {
    let opcode = book.opcode_for(name)?;
    contract_for_opcode(book, opcode, operand)
}

/// Preserve exact numeric identity, including noncanonical synthetic duplicates.
pub fn contract_for_opcode<'a>(
    book: &'a crate::opcode::OpcodeBook,
    opcode: u16,
    operand: &Operand,
) -> crate::error::Result<CommandContract<'a>> {
    let canonical = book.name(opcode)?;
    let retail_dispatch = opcode <= 1431;
    Ok(CommandContract {
        name: canonical,
        opcode,
        large_operand: book.has_large_operand(opcode),
        retail_dispatch,
        effect: if retail_dispatch {
            effect(canonical, operand)
        } else {
            Effect::Unknown
        },
        type_rule: type_rule(canonical),
        execution: if retail_dispatch {
            execution_family(canonical)
        } else {
            ExecutionFamily::HostRequired
        },
        state: if retail_dispatch {
            state_effect(canonical, operand)
        } else {
            StateEffect::Unknown
        },
        evidence: if retail_dispatch && canonical.starts_with("quest_") {
            "quest command handlers of the original client, dispatched by command id"
        } else if retail_dispatch && canonical == "db_getfield" {
            "db_getfield handler of the original client; database field split; recorded tuple/default fixture"
        } else if retail_dispatch
            && hand_checked_effect(canonical, operand) == Effect::Unknown
            && crate::cs2_stack_contracts::effect(canonical).is_some()
        {
            "cs2_stack_contracts.rs: committed stack-contract table; normal completion only"
        } else if retail_dispatch {
            "handler of the original client named by the command"
        } else {
            "unverified synthetic encoding; not retail evidence"
        },
    })
}

/// Dynamic type dependencies remain explicit even when static effects are unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeRule {
    Fixed,
    Operand,
    VariableDefinition,
    ParamDefinition,
    EnumOutput,
    DatabaseSchema,
    RuntimeState,
    CalleeSignature,
    HookDescriptor,
    Unknown,
}

pub fn type_rule(command: &str) -> TypeRule {
    match command {
        "push_var" | "pop_var" | "player_group_member_get_same_world_var" => {
            TypeRule::VariableDefinition
        }
        "worldlist_pingworlds" | "video_advert_play" => TypeRule::RuntimeState,
        "_enum" => TypeRule::EnumOutput,
        "db_getfield" | "db_listall" => TypeRule::DatabaseSchema,
        "struct_param" | "oc_param" | "nc_param" | "lc_param" | "seq_param" | "cc_param"
        | "mec_param" | "quest_param" => TypeRule::ParamDefinition,
        "gosub_with_params" => TypeRule::CalleeSignature,
        "push_constant_string" | "join_string" => TypeRule::Operand,
        name if is_hook_setter(name) => TypeRule::HookDescriptor,
        name if !matches!(effect(name, &Operand::Byte(0)), Effect::Unknown) => TypeRule::Fixed,
        _ => TypeRule::Unknown,
    }
}

/// Registry schema version, independent of the retail build number.
pub const SCHEMA_VERSION: u32 = 3;

/// Execution ownership. HostRequired does not assert that a host implements it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionFamily {
    Trap,
    Control,
    Data,
    Integer,
    String,
    HostRequired,
}

/// The interpreter routes through this registry instead of independent probes.
pub fn execution_family(command: &str) -> ExecutionFamily {
    if matches!(
        command,
        "retail_trap_79"
            | "retail_trap_103"
            | "retail_trap_247"
            | "retail_trap_250"
            | "retail_trap_393"
            | "worldmap_3dview_enable"
            | "worldmap_3dview_disable"
            | "worldmap_3dview_getcoordfine"
            | "worldmap_3dview_setlighting"
            | "worldmap_3dview_setloddistance"
    ) {
        return ExecutionFamily::Trap;
    }
    match command {
        "branch"
        | "branch_not"
        | "branch_equals"
        | "branch_less_than"
        | "branch_greater_than"
        | "branch_less_than_or_equals"
        | "branch_greater_than_or_equals"
        | "long_branch_not"
        | "long_branch_equals"
        | "long_branch_less_than"
        | "long_branch_greater_than"
        | "long_branch_less_than_or_equals"
        | "long_branch_greater_than_or_equals"
        | "branch_if_true"
        | "branch_if_false"
        | "switch"
        | "gosub_with_params"
        | "return" => ExecutionFamily::Control,
        "push_constant_int"
        | "push_long_constant"
        | "push_constant_string"
        | "push_int_local"
        | "pop_int_local"
        | "push_string_local"
        | "pop_string_local"
        | "push_long_local"
        | "pop_long_local"
        | "pop_int_discard"
        | "pop_string_discard"
        | "pop_long_discard"
        | "push_var"
        | "pop_var"
        | "push_varbit"
        | "pop_varbit"
        | "join_string"
        | "define_array"
        | "push_array_int"
        | "push_array_int_leave_index_on_stack"
        | "push_array_int_and_index"
        | "pop_array_int"
        | "pop_array_int_leave_value_on_stack" => ExecutionFamily::Data,
        "quickchat_dynamic_command_add"
        | "noopDisplayCommand"
        | "add"
        | "multiply"
        | "divide"
        | "modulo"
        | "and"
        | "or"
        | "pow"
        | "invpow"
        | "setbit"
        | "clearbit"
        | "testbit"
        | "setbit_range_toint"
        | "addpercent"
        | "interpolate"
        | "scale"
        | "min"
        | "max"
        | "sqrt"
        | "abs"
        | "not"
        | "bitcount"
        | "togglebit"
        | "setbit_range"
        | "clearbit_range"
        | "getbit_range" => ExecutionFamily::Integer,
        "append"
        | "append_num"
        | "append_signnum"
        | "append_char"
        | "tostring"
        | "string_length"
        | "substring"
        | "string_indexof_char"
        | "string_indexof_string" => ExecutionFamily::String,
        _ => ExecutionFamily::HostRequired,
    }
}

/// Commands consuming the hook descriptor protocol, including handlers that
/// discard the decoded hook rather than installing it.
pub fn is_hook_setter(command: &str) -> bool {
    if matches!(
        command,
        "if_discardGestureHook"
            | "cc_discardGestureHook"
            | "if_setongamepadbutton"
            | "cc_setongamepadbutton"
            | "if_setongamepadbuttonheld"
            | "cc_setongamepadbuttonheld"
            | "if_setongamepadaxis"
            | "cc_setongamepadaxis"
            | "if_setongamepadtrigger"
            | "cc_setongamepadtrigger"
    ) {
        return true;
    }
    if matches!(
        command,
        "if_setondragcomplete_alias" | "cc_setondragcomplete_alias"
    ) {
        return true;
    }
    let Some(name) = command
        .strip_prefix("cc_")
        .or_else(|| command.strip_prefix("if_"))
    else {
        return false;
    };
    matches!(
        name,
        "setonclick"
            | "setonhold"
            | "setonrelease"
            | "setonmouseover"
            | "setonmouseleave"
            | "setondrag"
            | "setontargetleave"
            | "setonvartransmit"
            | "setontimer"
            | "setonop"
            | "setondragcomplete"
            | "setonverticalswipe"
            | "setonhorizontalswipe"
            | "setonverticalpinch"
            | "setonhorizontalpinch"
            | "setonclickrepeat"
            | "setonmouserepeat"
            | "setoninvtransmit"
            | "setonstattransmit"
            | "setontargetenter"
            | "setonscrollwheel"
            | "setonchattransmit"
            | "setonkey"
            | "setongamepad"
            | "setonfriendtransmit"
            | "setonclantransmit"
            | "setonmisctransmit"
            | "setondialogabort"
            | "setonsubchange"
            | "setonstocktransmit"
            | "setoncamfinished"
            | "setonresize"
            | "setonvarctransmit"
            | "setonvarcstrtransmit"
            | "setonopt"
            | "setonclansettingstransmit"
            | "setonclanchanneltransmit"
            | "setonvarclantransmit"
            | "setonplayergrouptransmit"
            | "setonplayergroupvarptransmit"
            | "setoncameraupdatetransmit"
    )
}

#[cfg(test)]
mod extracted_tests {
    #[test]
    fn stack_contract_table_agrees_with_hand_checked_contracts() {
        let book = crate::opcode::OpcodeBook::embedded().unwrap();
        let mut compared = 0;
        for (opcode, command) in book.entries() {
            if opcode > 1431 {
                continue;
            }
            let operand = if command == "push_constant_string" {
                crate::script::Operand::Str(String::new())
            } else {
                crate::script::Operand::Byte(0)
            };
            if let (super::Effect::Fixed { pops, pushes }, Some(table)) = (
                super::hand_checked_effect(command, &operand),
                crate::cs2_stack_contracts::effect(command),
            ) {
                assert_eq!(super::Effect::Fixed { pops, pushes }, table, "{command}");
                compared += 1;
            }
        }
        assert!(compared > 150, "only {compared} shared shapes checked");
    }
}
