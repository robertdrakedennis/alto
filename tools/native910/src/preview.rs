//! Approximate legacy login host support. Not a source of verified semantics.
use crate::vm::{Host, InstructionContext, Value, VmError, VmResult};

/// Per-engine-opcode stack arity recorded from the original client's CS2
/// command handlers.
///
/// Every engine command pops its arguments off the typed stacks (int, string,
/// long) and pushes its results back. The VM already executes
/// pure ops (arithmetic, branches, locals, strings) for real; everything else
/// — the `if_*`/`cc_*` interface families, chat/clan/friend engine calls,
/// `_enum`, `struct_param`, font metrics, and the rest of the 906-tree trap
/// surface — falls through to [`crate::vm::Host::trap`]. Before the trap runs, the VM
/// pops the arg counts recorded here so giant builder scripts (world list
/// `7794`, friends `3023/3029/3041`, preferences `2595`) cannot breach the
/// 1000-slot stack cap with stale call args.
///
/// `cc_*` ops act on the active component (no component-id arg);
/// `if_*` ops pop one extra int for the packed `(interface << 16) | component`
/// id and then share the `cc_if_*` helper body, so `if_*` pops are the
/// `cc_*` pops plus one. `seton*` hooks (`cc_if_seton*`) take a variable
/// tail (the hook argument list always pops at least the hook id int); the
/// table records that verified minimum
/// and the host must tolerate the variable remainder. Commands with no
/// engine handler (synthetic opcode-book rows past the real
/// dispatch range `0..=1431`) are marked `UNVERIFIED` with zero pops rather
/// than guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arity {
    /// Ints consumed.
    pub int_pop: u8,
    /// Strings consumed.
    pub str_pop: u8,
    /// Longs consumed.
    pub long_pop: u8,
    /// Ints produced (conditional branches record each
    /// possible push, so `_enum`-style ops show both `int_push: 1` and
    /// `str_push: 1` for the either/or result).
    pub int_push: u8,
    /// Strings produced.
    pub str_push: u8,
    /// Longs produced.
    pub long_push: u8,
}

/// Engine-opcode arity for `command`, or `None` for ops the VM executes for
/// real (control flow, data movement, int arithmetic, pure strings) and for
/// unknown names. The trap path pops `int_pop`/`str_pop`/`long_pop` before
/// calling [`crate::vm::Host::trap`]; underflow is a [`VmError`] rather than a panic.
#[must_use]
pub fn arity(command: &str) -> Option<Arity> {
    match command {
        "_enum" => Some(Arity {
            int_pop: 4,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 1,
            long_push: 0,
        }), // conditional push: 1i OR 1s
        "activeclanchannel_find_affined" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "applet_hasfocus" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_addpinchflags" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_addswipeflags" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_button_getcantoggle" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1527)
        "cc_button_getlinkobjoptions" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1529)
        "cc_button_gettextareasizeoffsets" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1531)
        "cc_button_gettoggled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1525)
        "cc_button_setcantoggle" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1519)
        "cc_button_setlinkobjoptions" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1521)
        "cc_button_settextareasizeoffsets" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1523)
        "cc_button_settoggled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1517)
        "cc_callonresize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_carousel_addiconentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1707)
        "cc_carousel_addtextentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1705)
        "cc_carousel_clearentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1709)
        "cc_carousel_getbuttonsize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1717)
        "cc_carousel_getenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1711)
        "cc_carousel_getselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1701)
        "cc_carousel_setbuttonsize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1715)
        "cc_carousel_setenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1713)
        "cc_carousel_seticonentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1721)
        "cc_carousel_setselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1703)
        "cc_carousel_settextentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1719)
        "cc_check_get" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1515)
        "cc_check_getalignment" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1513)
        "cc_check_getbuttonsize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1511)
        "cc_check_set" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1505)
        "cc_check_setalignment" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1507)
        "cc_check_setbuttonsize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1509)
        "cc_clearops" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_clearscripthooks" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_combo_addentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1625)
        "cc_combo_clearentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1621)
        "cc_combo_getentrycount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1605)
        "cc_combo_getentryheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1603)
        "cc_combo_getselectedvalue" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1601)
        "cc_combo_hasentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1607)
        "cc_combo_removeentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1619)
        "cc_combo_reserveentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1627)
        "cc_combo_select" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1617)
        "cc_combo_setdisplayedentrycount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1611)
        "cc_combo_setdropdownbuttonparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1613)
        "cc_combo_setentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1623)
        "cc_combo_setentryheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1609)
        "cc_combo_setvisualparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1615)
        "cc_create" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_createchild" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1432)
        "cc_crmview_contains" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1845)
        "cc_crmview_dismiss" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1847)
        "cc_crmview_getint" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1843)
        "cc_crmview_getstring" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1841)
        "cc_crmview_init" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1849)
        "cc_crmview_init_v2" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1851)
        "cc_crmview_setavailablefonts" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1855)
        "cc_crmview_setonupdated" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1857)
        "cc_crmview_setservertargets" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1853)
        "cc_cutscenelayer_setoncutscenefinished" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1881)
        "cc_delete" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_deleteall" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_deleteallnested" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1439)
        "cc_delpinchflags" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_delswipeflags" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_discardGestureHook" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // alias of cc_discardhook dispatch 1008; hook op: minimum pops
        "cc_dragpickup" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_find" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_findbycategory" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1438)
        "cc_get2dangle" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getcharindexatpos" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getcharposatindex" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 2,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getcolour" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getdynamiclayer" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2017)
        "cc_getfontgraphic" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getfontmetrics" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getgraphic" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getgraphicdimensions" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 2,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_gethide" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getid" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getinvcount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }), // either branch pushes one
        "cc_getinvobject" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getlayer" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getmodel" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getmodelangle_x" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getmodelangle_y" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getmodelangle_z" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getmodelxof" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getmodelyof" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getmodelzoom" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getop" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }), // either branch pushes one
        "cc_getopbase" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }), // either branch pushes one
        "cc_getparentlayer" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getscrollheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getscrollwidth" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getscrollx" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getscrolly" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getstylesheet" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1533)
        "cc_gettargetmask" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_gettext" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }),
        "cc_gettrans" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getwidth" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_getx" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_gety" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "cc_grid_getcellheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1663)
        "cc_grid_getcellwidth" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1661)
        "cc_grid_getlayoutparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1665)
        "cc_grid_getnumcolumns" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1671)
        "cc_grid_getnumrows" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1669)
        "cc_grid_setlayoutparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1667)
        "cc_groupbox_getheaderlayout" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2003)
        "cc_groupbox_setheaderlayout" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2005)
        "cc_input_getcontentvisibilitymode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1561)
        "cc_input_getfiltermode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1559)
        "cc_input_getkeyhandlingmode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1557)
        "cc_input_getmaxlength" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1563)
        "cc_input_setemptytext" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1551)
        "cc_input_setinteractioncolours" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1553)
        "cc_input_setkeyhandlingmode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1555)
        "cc_input_setup" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1549)
        "cc_list_addentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1454)
        "cc_list_clear" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1453)
        "cc_list_clearselection" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1451)
        "cc_list_exists" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1452)
        "cc_list_getdropdownbuttonparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1466)
        "cc_list_getdropdownnumentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1458)
        "cc_list_getenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1443)
        "cc_list_getentryheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1463)
        "cc_list_getentryiconscale" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1468)
        "cc_list_getinteractioncolours" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1460)
        "cc_list_getnumselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1447)
        "cc_list_getselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1448)
        "cc_list_getselectionlimit" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1446)
        "cc_list_getvisible" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1444)
        "cc_list_isdropdown" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1456)
        "cc_list_isselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1450)
        "cc_list_setdropdownbuttonparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1465)
        "cc_list_setdropdownnumentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1457)
        "cc_list_setenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1441)
        "cc_list_setentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1455)
        "cc_list_setentryheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1464)
        "cc_list_setentryicon" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1461)
        "cc_list_setentryicons" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1462)
        "cc_list_setentryiconscale" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1467)
        "cc_list_setinteractioncolours" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1459)
        "cc_list_setisselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1449)
        "cc_list_setselectionlimit" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1445)
        "cc_list_setvisible" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1442)
        "cc_modelgroup_addcamera" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1891)
        "cc_modelgroup_cameraexists" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1895)
        "cc_modelgroup_deletecamera" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1893)
        "cc_modelgroup_getactivecamera" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1899)
        "cc_modelgroup_getcameraclearcolour" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1907)
        "cc_modelgroup_getcameraclipplanes" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1911)
        "cc_modelgroup_getcamerafov" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1923)
        "cc_modelgroup_getcameralookat" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1931)
        "cc_modelgroup_getcameraposition" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1927)
        "cc_modelgroup_getcamerasortbycomponentorder" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1919)
        "cc_modelgroup_getcamerauselookat" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1915)
        "cc_modelgroup_getcamerayawpitchroll" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1935)
        "cc_modelgroup_resetlighting" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1903)
        "cc_modelgroup_setactivecamera" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1897)
        "cc_modelgroup_setcameraclearcolour" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1905)
        "cc_modelgroup_setcameraclipplanes" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1909)
        "cc_modelgroup_setcamerafov" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1921)
        "cc_modelgroup_setcameralookat" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1929)
        "cc_modelgroup_setcameraposition" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1925)
        "cc_modelgroup_setcamerasortbycomponentorder" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1917)
        "cc_modelgroup_setcamerauselookat" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1913)
        "cc_modelgroup_setcamerayawpitchroll" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1933)
        "cc_modelgroup_setlighting" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1901)
        "cc_npc_setcustombodymodel" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_npc_setcustombodymodel_transformed" => Some(Arity {
            int_pop: 10,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_npc_setcustomheadmodel" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_npc_setcustomrecol" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_npc_setcustomretex" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_pagedcarousel_getbuttonparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1741)
        "cc_pagedcarousel_getdynamicpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1747)
        "cc_pagedcarousel_getenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1749)
        "cc_pagedcarousel_getfadesize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1743)
        "cc_pagedcarousel_getselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1763)
        "cc_pagedcarousel_getstaticpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1745)
        "cc_pagedcarousel_setbuttonparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1755)
        "cc_pagedcarousel_setdynamicpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1759)
        "cc_pagedcarousel_setenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1751)
        "cc_pagedcarousel_setfadesize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1753)
        "cc_pagedcarousel_setpagingparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1757)
        "cc_pagedcarousel_setselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1761)
        "cc_pagedlayer_createheader" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1803)
        "cc_pagedlayer_getactivepage" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1801)
        "cc_pagedlayer_getdynamicpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1783)
        "cc_pagedlayer_getpageenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1787)
        "cc_pagedlayer_getstaticpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1781)
        "cc_pagedlayer_setactivepage" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1799)
        "cc_pagedlayer_setdynamicpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1785)
        "cc_pagedlayer_setpageenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1789)
        "cc_pagedlayer_setpageicon" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1791)
        "cc_pagedlayer_setpageicons" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1795)
        "cc_pagedlayer_setpagelabel" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1793)
        "cc_pagedlayer_setpagelabels" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1797)
        "cc_panel_setisvertical" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2013)
        "cc_param" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 1,
            long_push: 0,
        }), // conditional push: 1i OR 1s
        "cc_radialprogressoverlay_getprogress" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2001)
        "cc_radialprogressoverlay_getvalue" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1999)
        "cc_radialprogressoverlay_getvaluerange" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1993)
        "cc_radialprogressoverlay_set" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1995)
        "cc_radialprogressoverlay_setvalue" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1997)
        "cc_radialprogressoverlay_start" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1991)
        "cc_radiogroup_addoption" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1501)
        "cc_radiogroup_clearoptions" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2009)
        "cc_radiogroup_setoptions" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2008)
        "cc_radiogroup_setoptionselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1503)
        "cc_radiogroup_setselectionlimits" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2007)
        "cc_resetlinkplayer" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_resetmodellighting" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_resume_pausebutton" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_scrollbar_setup" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1541)
        "cc_scrollbar_setvisible" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1539)
        "cc_sendtoback" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // alias of cc_sendto(false) dispatch 278
        "cc_sendtofront" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // alias of cc_sendto(true) dispatch 587
        "cc_set2dangle" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setalpha" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setaspect" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setchildspacing" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2011)
        "cc_setclickmask" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setcolour" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setdragdeadtime" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setdragdeadzone" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setdraggable" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setdragrenderbehaviour" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1535)
        "cc_setfeedbackmode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1537)
        "cc_setfill" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setfontmono" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setgraphic" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setgraphicshadow" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setheld" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_sethflip" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_sethide" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setlinedirection" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setlinewid" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setlinkactiveclanchannel" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setlinkfriend" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setlinkfriendchat" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setlinkplayergroup" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setmaxlines" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setmodel" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setmodelangle" => Some(Arity {
            int_pop: 6,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setmodelanim" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setmodellighting" => Some(Arity {
            int_pop: 10,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setmodelorigin" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setmodelorthog" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setmodeltint" => Some(Arity {
            int_pop: 4,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setmodelzoom" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setmouseovercursor" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setnoclickthrough" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setnpchead" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setnpcmodel" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setobject" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setobject_alwaysnum" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setobject_nonum" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setobject_wearcol" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setobject_wearcol_alwaysnum" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setobject_wearcol_nonum" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setonbuttonclick" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1543)
        "cc_setoncameraupdatetransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setoncamfinished" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonchattransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonclanchanneltransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonclansettingstransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonclantransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonclick" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonclickrepeat" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setondialogabort" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setondrag" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setondragcomplete" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setondragcomplete_alias" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops
        "cc_setondropdownselect" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1497)
        "cc_setonfriendtransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setongamepadaxis" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setongamepadbutton" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setongamepadbuttonheld" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setongamepadtrigger" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonhold" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonhorizontalpinch" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonhorizontalswipe" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setoninvtransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonkey" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonmisctransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonmouseleave" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonmouseover" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonmouserepeat" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonop" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonopt" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonplayergrouptransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonplayergroupvarptransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonrelease" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonresize" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonscrollwheel" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonstattransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonstocktransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonsubchange" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setontargetenter" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setontargetleave" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setontimer" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonvarclantransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonvarcstrtransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonvarctransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonvartransmit" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonverticalpinch" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setonverticalswipe" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "cc_setop" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setopbase" => Some(Arity {
            int_pop: 0,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setopchar" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setopcursor" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setopkey" => Some(Arity {
            int_pop: 10,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setopkeyignoreheld" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setopkeyrate" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setoptchar" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setoptkey" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setoptkeyignoreheld" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setoptkeyrate" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setoutline" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setparam" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setparam_int" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setparam_string" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setpausetext" => Some(Arity {
            int_pop: 0,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setpinchdeadzone" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setpinchflags" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setplayerhead_self" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setplayermodel" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setplayermodel_self" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setposition" => Some(Arity {
            int_pop: 4,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setrecol" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setretex" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setscrollpos" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setscrollsize" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setsize" => Some(Arity {
            int_pop: 4,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setstylesheet" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1499)
        "cc_setsubtractinsets" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setswipedeadtime" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setswipedeadzone" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setswipeflags" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_settargetcursors" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_settargetopcursor" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_settargetverb" => Some(Arity {
            int_pop: 0,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_settext" => Some(Arity {
            int_pop: 0,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_settextalign" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_settextantimacro" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_settextfont" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_settextshadow" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_settiling" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_settrans" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_setvflip" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "cc_slider_getminmax" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1587)
        "cc_slider_getscale" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1583)
        "cc_slider_gettick" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1581)
        "cc_slider_getvalue" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1585)
        "cc_slider_getvaluelabel" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1597)
        "cc_slider_setup" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1593)
        "cc_slider_setupenum" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1595)
        "cc_slider_setvalue" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1589)
        "cc_slider_setvisualparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1591)
        "cc_text_settrans" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2014)
        "cc_triggerop" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "chat_getfilter_private" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "chat_gethistory_byuid" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 4,
            str_push: 6,
            long_push: 0,
        }), // both branches push 4i+6s
        "chat_lastuid" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "chat_playername" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }),
        "clan_getchatcount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "clientclock" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "clienttype" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "compare" => Some(Arity {
            int_pop: 0,
            str_pop: 2,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "date_runeday" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "detailget_maxscreensize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "detailget_toolkit" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "enum_getoutputcount" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "enum_hasoutput" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "escape" => Some(Arity {
            int_pop: 0,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }),
        "format_datetime_from_minutes" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }),
        "friend_count" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "fullscreen_getmode" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 2,
            str_push: 0,
            long_push: 0,
        }),
        "fullscreen_modecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "getwindowmode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "has_nxt" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_addpinchflags" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_addswipeflags" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_button_getcantoggle" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1528)
        "if_button_getlinkobjoptions" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1530)
        "if_button_gettextareasizeoffsets" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1532)
        "if_button_gettoggled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1526)
        "if_button_setcantoggle" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1520)
        "if_button_setlinkobjoptions" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1522)
        "if_button_settextareasizeoffsets" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1524)
        "if_button_settoggled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1518)
        "if_callonresize" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_carousel_addiconentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1708)
        "if_carousel_addtextentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1706)
        "if_carousel_clearentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1710)
        "if_carousel_getbuttonsize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1718)
        "if_carousel_getenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1714)
        "if_carousel_getselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1702)
        "if_carousel_setbuttonsize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1716)
        "if_carousel_setenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1714)
        "if_carousel_seticonentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1722)
        "if_carousel_setselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1704)
        "if_carousel_settextentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1720)
        "if_check_get" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1516)
        "if_check_getalignment" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1514)
        "if_check_getbuttonsize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1512)
        "if_check_set" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1506)
        "if_check_setalignment" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1508)
        "if_check_setbuttonsize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1510)
        "if_clearops" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_clearscripthooks" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_close" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_closesubclient" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_combo_addentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1626)
        "if_combo_clearentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1622)
        "if_combo_getentrycount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1606)
        "if_combo_getentryheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1604)
        "if_combo_getselectedvalue" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1602)
        "if_combo_hasentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1608)
        "if_combo_removeentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1620)
        "if_combo_reserveentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1628)
        "if_combo_select" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1618)
        "if_combo_setdisplayedentrycount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1612)
        "if_combo_setdropdownbuttonparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1614)
        "if_combo_setentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1624)
        "if_combo_setentryheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1610)
        "if_combo_setvisualparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1616)
        "if_createchild" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1433)
        "if_createnested" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1434)
        "if_crmview_contains" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1846)
        "if_crmview_dismiss" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1848)
        "if_crmview_getint" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1844)
        "if_crmview_getstring" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1842)
        "if_crmview_init" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1850)
        "if_crmview_init_v2" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1852)
        "if_crmview_setavailablefonts" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1856)
        "if_crmview_setonupdated" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1858)
        "if_crmview_setservertargets" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1854)
        "if_cutscenelayer_setoncutscenefinished" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1882)
        "if_debug_button1" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_button10" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_button2" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_button3" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_button4" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_button5" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_button6" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_button7" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_button8" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_button9" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_getcomcount" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }), // either branch pushes one
        "if_debug_getcomname" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }), // either branch pushes one
        "if_debug_getname" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }), // either branch pushes one
        "if_debug_getopenifcount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_debug_getopenifid" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }), // either branch pushes one
        "if_debug_getservertriggers" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }), // either branch pushes one
        "if_debug_target" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_deleteallnested" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1440)
        "if_delpinchflags" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_delswipeflags" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_discardGestureHook" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // alias of if_discardhook dispatch 768; hook op: minimum pops
        "if_dragpickup" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_find" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_get2dangle" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_get_gamescreen" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getcharindexatpos" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getcharposatindex" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 2,
            str_push: 0,
            long_push: 0,
        }),
        "if_getchildspacing" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2012)
        "if_getcolour" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2010)
        "if_getfontgraphic" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getfontmetrics" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getgraphic" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getgraphicdimensions" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 2,
            str_push: 0,
            long_push: 0,
        }),
        "if_getheight" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_gethide" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getinvcount" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }), // either branch pushes one
        "if_getinvobject" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getlayer" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getmodel" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getmodelangle_x" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getmodelangle_y" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getmodelangle_z" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getmodelxof" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getmodelyof" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getmodelzoom" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getnextcategorysubid" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1437)
        "if_getnextsubid" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getop" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }), // either branch pushes one
        "if_getopbase" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }), // either branch pushes one
        "if_getparentlayer" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getscrollheight" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getscrollwidth" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getscrollx" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getscrolly" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getstylesheet" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1534)
        "if_gettargetmask" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_gettext" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }),
        "if_gettop" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_gettrans" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getwidth" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_getx" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_gety" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_grid_getcellheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1664)
        "if_grid_getcellwidth" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1662)
        "if_grid_getlayoutparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1666)
        "if_grid_getnumcolumns" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1672)
        "if_grid_getnumrows" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1670)
        "if_grid_setlayoutparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1668)
        "if_groupbox_getheaderlayout" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2004)
        "if_groupbox_setheaderlayout" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2006)
        "if_hassub" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }), // either branch pushes one
        "if_hassubmodal" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_hassuboverlay" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "if_input_getcontentvisibilitymode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1562)
        "if_input_getfiltermode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1560)
        "if_input_getkeyhandlingmode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1558)
        "if_input_getmaxlength" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1564)
        "if_input_setemptytext" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1552)
        "if_input_setinteractioncolours" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1554)
        "if_input_setkeyhandlingmode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1556)
        "if_input_setup" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1550)
        "if_list_addentry" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1482)
        "if_list_clear" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1481)
        "if_list_clearselection" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1479)
        "if_list_exists" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1480)
        "if_list_getdropdownbuttonparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1494)
        "if_list_getdropdownnumentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1486)
        "if_list_getenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1471)
        "if_list_getentryheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1491)
        "if_list_getentryiconscale" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1496)
        "if_list_getinteractioncolours" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1488)
        "if_list_getnumselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1475)
        "if_list_getselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1476)
        "if_list_getselectionlimit" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1474)
        "if_list_getvisible" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1472)
        "if_list_isdropdown" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1484)
        "if_list_isselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1478)
        "if_list_setdropdownbuttonparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1493)
        "if_list_setdropdownnumentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1485)
        "if_list_setenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1469)
        "if_list_setentries" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1483)
        "if_list_setentryheight" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1492)
        "if_list_setentryicon" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1489)
        "if_list_setentryicons" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1490)
        "if_list_setentryiconscale" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1495)
        "if_list_setinteractioncolours" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1487)
        "if_list_setisselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1477)
        "if_list_setselectionlimit" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1473)
        "if_list_setvisible" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1470)
        "if_modelgroup_addcamera" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1892)
        "if_modelgroup_cameraexists" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1896)
        "if_modelgroup_deletecamera" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1894)
        "if_modelgroup_getactivecamera" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1900)
        "if_modelgroup_getcameraclearcolour" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1908)
        "if_modelgroup_getcameraclipplanes" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1912)
        "if_modelgroup_getcamerafov" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1924)
        "if_modelgroup_getcameralookat" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1932)
        "if_modelgroup_getcameraposition" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1928)
        "if_modelgroup_getcamerasortbycomponentorder" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1920)
        "if_modelgroup_getcamerauselookat" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1916)
        "if_modelgroup_getcamerayawpitchroll" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1936)
        "if_modelgroup_resetlighting" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1904)
        "if_modelgroup_setactivecamera" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1898)
        "if_modelgroup_setcameraclearcolour" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1906)
        "if_modelgroup_setcameraclipplanes" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1910)
        "if_modelgroup_setcamerafov" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1922)
        "if_modelgroup_setcameralookat" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1930)
        "if_modelgroup_setcameraposition" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1926)
        "if_modelgroup_setcamerasortbycomponentorder" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1918)
        "if_modelgroup_setcamerauselookat" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1914)
        "if_modelgroup_setcamerayawpitchroll" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1934)
        "if_modelgroup_setlighting" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1902)
        "if_npc_setcustombodymodel" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_npc_setcustombodymodel_transformed" => Some(Arity {
            int_pop: 11,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_npc_setcustomheadmodel" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_npc_setcustomrecol" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_npc_setcustomretex" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_opensubclient" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_pagedcarousel_getbuttonparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1742)
        "if_pagedcarousel_getdynamicpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1748)
        "if_pagedcarousel_getenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1750)
        "if_pagedcarousel_getfadesize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1744)
        "if_pagedcarousel_getselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1764)
        "if_pagedcarousel_getstaticpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1746)
        "if_pagedcarousel_setbuttonparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1756)
        "if_pagedcarousel_setdynamicpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1760)
        "if_pagedcarousel_setenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1752)
        "if_pagedcarousel_setfadesize" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1754)
        "if_pagedcarousel_setpagingparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1758)
        "if_pagedcarousel_setselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1762)
        "if_pagedlayer_createheader" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1804)
        "if_pagedlayer_getactivepage" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1802)
        "if_pagedlayer_getdynamicpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1784)
        "if_pagedlayer_getpageenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1788)
        "if_pagedlayer_getstaticpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1782)
        "if_pagedlayer_setactivepage" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1800)
        "if_pagedlayer_setdynamicpagecount" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1786)
        "if_pagedlayer_setpageenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1790)
        "if_pagedlayer_setpageicon" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1792)
        "if_pagedlayer_setpageicons" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1796)
        "if_pagedlayer_setpagelabel" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1794)
        "if_pagedlayer_setpagelabels" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1798)
        "if_radialprogressoverlay_getprogress" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2002)
        "if_radialprogressoverlay_getvalue" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 2000)
        "if_radialprogressoverlay_getvaluerange" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1994)
        "if_radialprogressoverlay_set" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1996)
        "if_radialprogressoverlay_setvalue" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1998)
        "if_radialprogressoverlay_start" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1992)
        "if_radiogroup_addoption" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1502)
        "if_radiogroup_setoptionselected" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1504)
        "if_resetlinkplayer" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_resetmodellighting" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_resume_pausebutton" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_scrollbar_setup" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1542)
        "if_scrollbar_setvisible" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1540)
        "if_sendtoback" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // alias of if_sendto(false) dispatch 490
        "if_sendtofront" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // alias of if_sendto(true) dispatch 12
        "if_set2dangle" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_set_gamescreen_enabled" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setalpha" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setaspect" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setclickmask" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setcolour" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setdragdeadtime" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setdragdeadzone" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setdraggable" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setdragrenderbehaviour" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setenabled" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1536)
        "if_setfeedbackmode" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1538)
        "if_setfill" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setfontmono" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setgraphic" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setgraphicshadow" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setheld" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_sethflip" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_sethide" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setlinedirection" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setlinewid" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setlinkactiveclanchannel" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setlinkfriend" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setlinkfriendchat" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setlinkplayergroup" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setmaxlines" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setmodel" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setmodelangle" => Some(Arity {
            int_pop: 7,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setmodelanim" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setmodellighting" => Some(Arity {
            int_pop: 11,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setmodelorigin" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setmodelorthog" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setmodeltint" => Some(Arity {
            int_pop: 5,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setmodelzoom" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setmouseovercursor" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setnoclickthrough" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setnpchead" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setnpcmodel" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setobject" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setobject_alwaysnum" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setobject_nonum" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setobject_wearcol" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setobject_wearcol_alwaysnum" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setobject_wearcol_nonum" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setonbuttonclick" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1544)
        "if_setoncameraupdatetransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setoncamfinished" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonchattransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonclanchanneltransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonclansettingstransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonclantransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonclick" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonclickrepeat" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setondialogabort" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setondrag" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setondragcomplete" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setondragcomplete_alias" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops
        "if_setondropdownselect" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1498)
        "if_setonfriendtransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setongamepadaxis" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setongamepadbutton" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setongamepadbuttonheld" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setongamepadtrigger" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonhold" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonhorizontalpinch" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonhorizontalswipe" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setoninvtransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonkey" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonmisctransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonmouseleave" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonmouseover" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonmouserepeat" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonop" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonopt" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonplayergrouptransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonplayergroupvarptransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonrelease" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonresize" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonscrollwheel" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonstattransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonstocktransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonsubchange" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setontargetenter" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setontargetleave" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setontimer" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonvarclantransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonvarcstrtransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonvarctransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonvartransmit" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonverticalpinch" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setonverticalswipe" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // hook op: minimum pops (+variable hook args)
        "if_setop" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setopbase" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setopchar" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setopcursor" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setopkey" => Some(Arity {
            int_pop: 4,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // pops include the op slot
        "if_setopkeyignoreheld" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setopkeyrate" => Some(Arity {
            int_pop: 4,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setoptchar" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setoptkey" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setoptkeyignoreheld" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setoptkeyrate" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setoutline" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setparam_int" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setparam_string" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setpausetext" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setpinchdeadzone" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setpinchflags" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setplayerhead_self" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setplayermodel" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setplayermodel_self" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setposition" => Some(Arity {
            int_pop: 5,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setrecol" => Some(Arity {
            int_pop: 4,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setretex" => Some(Arity {
            int_pop: 4,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setscrollpos" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setscrollsize" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setsize" => Some(Arity {
            int_pop: 5,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setstylesheet" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1500)
        "if_setsubtractinsets" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setswipedeadtime" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setswipedeadzone" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setswipeflags" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_settargetcursors" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_settargetopcursor" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_settargetverb" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_settext" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_settextalign" => Some(Arity {
            int_pop: 4,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_settextantimacro" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_settextfont" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_settextshadow" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_settiling" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_settrans" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_setvflip" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "if_slider_getminmax" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1588)
        "if_slider_getscale" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1584)
        "if_slider_gettick" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1582)
        "if_slider_getvalue" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1586)
        "if_slider_getvaluelabel" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1598)
        "if_slider_setup" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1594)
        "if_slider_setupenum" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1596)
        "if_slider_setvalue" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1590)
        "if_slider_setvisualparams" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // UNVERIFIED: no engine handler (synthetic opcode id 1592)
        "if_triggerop" => Some(Arity {
            int_pop: 3,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }),
        "minimenuopen" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "noopDisplayCommand" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 0,
            str_push: 0,
            long_push: 0,
        }), // disabled_command_1253 dispatch 1253; empty
        "paraheight" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }), // same shape as parawidth, invoked by hook 10058
        "paraline" => Some(Arity {
            int_pop: 3,
            str_pop: 1,
            long_pop: 0,
            int_push: 0,
            str_push: 1,
            long_push: 0,
        }),
        "parawidth" => Some(Arity {
            int_pop: 2,
            str_pop: 1,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "playermember" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "quickchat_dynamic_command_add" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }), // quick chat dynamic command add, dispatch 869
        "random" => Some(Arity {
            int_pop: 1,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "stringwidth" => Some(Arity {
            int_pop: 1,
            str_pop: 1,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "struct_param" => Some(Arity {
            int_pop: 2,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 1,
            long_push: 0,
        }), // conditional push: 1i OR 1s
        "userdetail_lobby_emailstatus" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "userdetail_lobby_lastloginday" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),
        "userdetail_lobby_membership" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 3,
            str_push: 0,
            long_push: 0,
        }),
        "userdetail_lobby_unreadmessages" => Some(Arity {
            int_pop: 0,
            str_pop: 0,
            long_pop: 0,
            int_push: 1,
            str_push: 0,
            long_push: 0,
        }),

        _ => None,
    }
}

/// Explicitly approximate dispatch for the legacy login preview.
pub fn dispatch<H: Host + ?Sized>(
    host: &mut H,
    context: &InstructionContext<'_>,
    ints: &mut Vec<i32>,
    strs: &mut Vec<String>,
    longs: &mut Vec<i64>,
) -> VmResult<Option<Value>> {
    let cmd = context.command;
    // Consume the engine op's stack args before the trap runs: without
    // this, every trapped call leaks its args and giant builder
    // scripts breach the 1000-slot stack cap (`int stack
    // overflow` on the 906 login tree). Short stacks are an honest
    // [`VmError::StackUnderflow`], never a panic. Four ops keep their
    // popping inside [`Host::trap`] instead: `tools/client910/src/
    // iface.rs` `VmState::trap` is frozen and already consumes
    // `if_gettext`, `format_datetime_from_minutes`,
    // `chat_gethistory_byuid` and `_enum`, so the VM skips
    // pre-popping those to avoid a double pop.
    if let Some(fx) = arity(cmd)
        && !matches!(
            cmd,
            "if_gettext" | "format_datetime_from_minutes" | "chat_gethistory_byuid" | "_enum"
        )
    {
        pop_trap_args(&mut *ints, fx.int_pop, "int")?;
        pop_trap_args(&mut *strs, fx.str_pop, "object")?;
        pop_trap_args(&mut *longs, fx.long_pop, "long")?;
    }
    let pushed = host.trap(cmd, &mut *ints, &mut *strs, &mut *longs)?;
    if let Some(value) = pushed {
        // Top-up the host's single answer to the table's declared
        // push shape with neutral values. The frozen login host
        // answers every non-string engine op with one `Int(-1)`, but
        // several traps push more (or a string) in the client: hook 10058's
        // `userdetail_lobby_membership` pushes 3 ints
        // while the host supplies 1,
        // so the hook's three `pop_int_local` underflow without the
        // 2 extra; hook 10022's `escape` pops a string and pushes one
        // while the host supplies an
        // int, so the following `if_settext` finds no object without
        // the top-up string. Additive only: hosts that already match
        // (all single-push getters) see no change, and hosts that
        // over-supply (void setters returning `Int(-1)`) keep the
        // documented leak, so giant-builder overflow behavior
        // (hook 10027) is unchanged. Skipped for the host-driven
        // shapes (`None` returns: `chat_gethistory_byuid`, `_enum`)
        // and the conditional-push ops (`struct_param`, `cc_param`), where the
        // host answers the taken branch and topping-up the other
        // type would leak a stray value per call.
        let (have_i, have_o, have_l) = match &value {
            Value::Int(_) => (1_usize, 0_usize, 0_usize),
            Value::Str(_) | Value::Null => (0_usize, 1_usize, 0_usize),
            Value::Long(_) => (0_usize, 0_usize, 1_usize),
        };
        match value {
            Value::Int(v) => ints.push(v),
            Value::Str(v) => strs.push(v),
            Value::Null => {
                return Err(VmError::NullObject {
                    command: cmd.into(),
                });
            }
            Value::Long(v) => longs.push(v),
        }
        if let Some(fx) = arity(cmd)
            && !matches!(cmd, "struct_param" | "cc_param")
        {
            for _ in have_i..usize::from(fx.int_push) {
                ints.push(-1);
            }
            for _ in have_o..usize::from(fx.str_push) {
                strs.push(String::new());
            }
            for _ in have_l..usize::from(fx.long_push) {
                longs.push(0);
            }
        }
    }

    Ok(None)
}

fn pop_trap_args<T>(stack: &mut Vec<T>, count: u8, name: &'static str) -> VmResult<()> {
    let count = usize::from(count);
    if stack.len() < count {
        return Err(VmError::StackUnderflow { stack: name });
    }
    stack.truncate(stack.len().saturating_sub(count));
    Ok(())
}
