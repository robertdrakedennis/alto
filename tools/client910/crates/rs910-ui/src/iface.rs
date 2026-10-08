//! What is left of the original interface stack's pack-only preview (its tree
//! decoder, layout painter and `--login-screen` window were replaced by the
//! retained runtime, `ui_runtime`/`ui_components`/`ui_backend`; Phase 3.1
//! moved `fetch_file` to `rs910_js5::js5_fetch` and `SpriteSheet` to
//! `rs910_config::sprite_sheet`):
//! - [`decode_cp1252`]: loading-screen text decoded by the shared client charset.
//! - `VmState` (tests only): the trap-recording CS2 host the oracle
//!   harnesses run scripts against.

#[cfg(any(test, feature = "test-hooks"))]
use std::collections::HashMap;

/// Decode client Windows-1252 text, skipping NUL and mapping unassigned bytes
/// to `?`, as the shared client charset does.
pub fn decode_cp1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .copied()
        .filter(|byte| *byte != u8::default())
        .map(rs910_core::cp1252::cp1252_decode_byte)
        .collect()
}

// ---------------------------------------------------------------------------
// Test CS2 host
// ---------------------------------------------------------------------------

/// In-memory CS2 variable/array state plus the trapped engine-opcode log
/// behind [`native910::vm::Vm`].
///
/// Unit tests use this directly as the pack-free stub host; the login path
/// runs it inside [`VmAdapter`].
///
/// # Trap policy: record-and-answer-neutral
///
/// Every engine opcode the VM does not implement for real (`if_*` layout and
/// query ops, `cc_*` component ops, `clientclock`, `_enum`, `struct_param`,
/// …) is recorded by command name in [`VmState::trapped`] (in call order)
/// and answered with a neutral value so the script runs to `return` instead
/// of dying at the first engine call. The trap deliberately consumes nothing:
/// the [`native910::vm::Host::trap`] signature carries no operand/arity, so
/// per-op argument consumption is unknowable here — stale call args plus one
/// neutral answer per trapped call is the documented cost (stacks hold 1000
/// slots; typical login hooks settle at dozens, the largest builders at a few
/// hundred — the few that breach it are documented on the tree test below).
///
/// Answer shape follows the command result types, with neutral
/// content (no player, no fonts, no configs are loaded in this path):
/// - String-typed getters push neutral strings: `chat_playername` pushes the
///   player name, `cc_gettext` pushes the component text, `if_gettext`
///   pops the packed id and pushes the component text,
///   `format_datetime_from_minutes` pops minutes and pushes the formatted
///   string. Without the string, object-stack consumers (`parawidth`, which
///   pops 1 obj + 2 ints, …) would
///   underflow — e.g. hook 10058 (`chat_playername` + two `cc_gettext`
///   calls feeding `parawidth`/`paraheight`).
/// - `chat_gethistory_byuid` replays the no-history miss path:
///   at login there is no chat history, so the
///   lookup finds no line and the original client pops the uid, then pushes the full
///   default shape `Int(-1), Str(""), Int(0), Str × 4, Int(0), Str(""),
///   Int(-1)`. The friends/clan/chat builders (3054/4565/3173) pop exactly
///   that shape off the stacks, so anything else underflows them.
/// - `_enum` mirrors the original dispatch: it pops
///   `(inputtype, outputtype, enumid, key)`, pushes a string iff `outputtype
///   == STRING`): the output id rides on the int stack, so the
///   trap reads it back off (the string output type is `36`;
///   every other id in the tree, e.g. `73`, is integer-based). Hook 3376
///   needs both shapes: `(0, 73, …)` results feed `branch_equals`, while
///   `(0, 36, …)` results flow into `pop_string_local`.
/// - Every other engine opcode pushes `Int(-1)`, the CS2 "none" sentinel.
///   `-1` (not `0`) is what lets sentinel-walking scripts terminate: hook
///   12611 walks the layer chain with `push id; if_getlayer; push -1;
///   branch_not` (`if_getlayer` pops the id and pushes the layer), so a `0` answer loops forever — 157k `if_getlayer` traps
///   and an `int stack overflow` per caller — while `-1` exits on the first
///   pass. The same `-1` skips 12610's `enum_getoutputcount`-bounded fill
///   loop instead of iterating it.
///
/// Real login hooks require exactly this to complete:
/// - hook 3057 (31 instrs) pushes four int-ish values into `if_setontimer`,
///   then runs eleven `push Int(-1)` / `pop_var(client:…)` pairs and finishes
///   with `gosub_with_params` — the trap lets it through and the gosub
///   provider (see [`VmAdapter`]) resolves the callee.
/// - hook 13383 (46 instrs) branches on locals around `clientclock`
///   (pushes an int), `if_setontimer`, `_enum`
///   (int result here — enum 12591's output type is int, consumed by
///   `pop_int_local`) and `struct_param`, then `gosub_with_params`.
/// - 9351/10035-style `(int, …)` hooks do `push arg; if_gethide;
///   pop_int_local`: the `Int(-1)` stand-in keeps the pop balanced so the
///   script reaches `return`; the follow-up `if_sethide` is recorded the same
///   way. Layout effects are NOT applied (no component is mutated) — same as
///   the old stub, but scripts now genuinely execute.
///
/// Known limits (failures land in [`OnLoadReport::failed`] with the script
/// id; nothing here panics):
/// - String-typed `_enum` outputs beyond `STRING`, and string-typed
///   `struct_param`/`cc_param` results, cannot be typed from the trap
///   signature (the param/enum tables are not loaded here); where the login
///   tree needs the int shape it is proven by the caller's own
///   `pop_int_local` (real scripts are stack-exact — e.g. every tree
///   `struct_param`/`cc_param(4425/5769)` site pops an int).
/// - Variable types are declared metadata, not inferred from stored values
///   (`pop_var`). This preview still defaults unknown
///   definitions to int; the game host must load actual cache definitions.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug, Default)]
pub struct VmState {
    var_types: HashMap<(native910::vars::VarScope, u16), native910::vm::VarLane>,
    vars: HashMap<(native910::vars::VarScope, u16, bool), native910::vm::Value>,
    varbits: HashMap<(u16, bool), i32>,
    arrays: HashMap<i32, Vec<i32>>,
    trapped: Vec<String>,
}

/// Host-side array cap mirroring the VM's `define_array` rule (the VM rejects
/// `len > 5000` before the host ever sees it; the cap here keeps direct
/// `VmState` users bounded the same way).
#[cfg(any(test, feature = "test-hooks"))]
const MAX_HOST_ARRAY_LEN: usize = 5000;

#[cfg(any(test, feature = "test-hooks"))]
impl VmState {
    /// Empty state: vars/varbits read back 0, no arrays, nothing trapped.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Engine-opcode names trapped so far, in call order.
    #[must_use]
    pub fn trapped(&self) -> &[String] {
        &self.trapped
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl native910::vm::Host for VmState {
    // This host is an approximate login preview, not a fidelity oracle.
    fn trap_context(
        &mut self,
        context: &native910::vm::InstructionContext<'_>,
        ints: &mut Vec<i32>,
        strs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> native910::vm::VmResult<Option<native910::vm::Value>> {
        native910::preview::dispatch(self, context, ints, strs, longs)
    }
    fn var_type(
        &mut self,
        domain: native910::vars::VarScope,
        id: u16,
    ) -> native910::vm::VmResult<native910::vm::VarLane> {
        if let Some(kind) = self.var_types.get(&(domain, id)) {
            return Ok(*kind);
        }
        Ok(
            if domain == native910::vars::VarScope::Client && matches!(id, 2479 | 2480) {
                native910::vm::VarLane::String
            } else {
                native910::vm::VarLane::Int
            },
        )
    }

    fn var_get(
        &mut self,
        domain: native910::vars::VarScope,
        id: u16,
        transmog: bool,
    ) -> native910::vm::VmResult<native910::vm::Value> {
        Ok(self
            .vars
            .get(&(domain, id, transmog))
            .cloned()
            .unwrap_or_else(|| {
                if domain == native910::vars::VarScope::Client && matches!(id, 2479 | 2480) {
                    native910::vm::Value::Str(String::new())
                } else {
                    native910::vm::Value::Int(0)
                }
            }))
    }

    fn var_set(
        &mut self,
        domain: native910::vars::VarScope,
        id: u16,
        transmog: bool,
        value: native910::vm::Value,
    ) -> native910::vm::VmResult<()> {
        self.vars.insert((domain, id, transmog), value);
        Ok(())
    }

    fn varbit_get(&mut self, id: u16, transmog: bool) -> native910::vm::VmResult<i32> {
        Ok(self.varbits.get(&(id, transmog)).copied().unwrap_or(0))
    }

    fn varbit_set(&mut self, id: u16, transmog: bool, value: i32) -> native910::vm::VmResult<()> {
        self.varbits.insert((id, transmog), value);
        Ok(())
    }

    fn array_define(&mut self, array_id: i32, len: usize) -> native910::vm::VmResult<()> {
        if len > MAX_HOST_ARRAY_LEN {
            return Err(native910::vm::VmError::BadArray {
                id: array_id,
                reason: format!("length {len} exceeds host cap {MAX_HOST_ARRAY_LEN}"),
            });
        }
        self.arrays.insert(array_id, vec![0; len]);
        Ok(())
    }

    fn array_len(&mut self, array_id: i32) -> native910::vm::VmResult<usize> {
        self.arrays
            .get(&array_id)
            .map(Vec::len)
            .ok_or(native910::vm::VmError::BadArray {
                id: array_id,
                reason: "unknown array".to_string(),
            })
    }

    fn array_get(&mut self, array_id: i32, index: i32) -> native910::vm::VmResult<i32> {
        let slot = usize::try_from(index).map_err(|_| native910::vm::VmError::BadArray {
            id: array_id,
            reason: format!("negative index {index}"),
        })?;
        self.arrays
            .get(&array_id)
            .and_then(|items| items.get(slot))
            .copied()
            .ok_or(native910::vm::VmError::BadArray {
                id: array_id,
                reason: format!("index {index} out of bounds"),
            })
    }

    fn array_set(&mut self, array_id: i32, index: i32, value: i32) -> native910::vm::VmResult<()> {
        let slot = usize::try_from(index).map_err(|_| native910::vm::VmError::BadArray {
            id: array_id,
            reason: format!("negative index {index}"),
        })?;
        let cell = self
            .arrays
            .get_mut(&array_id)
            .and_then(|items| items.get_mut(slot))
            .ok_or(native910::vm::VmError::BadArray {
                id: array_id,
                reason: format!("index {index} out of bounds"),
            })?;
        *cell = value;
        Ok(())
    }

    fn trap(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        strs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> native910::vm::VmResult<Option<native910::vm::Value>> {
        self.trapped.push(command.to_string());
        // Pop-then-answer effects for the string-typed getters above; the
        // pops take the call's own args (pushed immediately before the call),
        // never live values. Everything else is left untouched — see the
        // policy docs on [`VmState`].
        match command {
            "chat_playername" | "cc_gettext" => Ok(Some(native910::vm::Value::Str(String::new()))),
            "if_gettext" | "format_datetime_from_minutes" => {
                let _ = ints.pop();
                Ok(Some(native910::vm::Value::Str(String::new())))
            }
            "chat_gethistory_byuid" => {
                let _ = ints.pop();
                for value in [
                    native910::vm::Value::Int(-1),
                    native910::vm::Value::Str(String::new()),
                    native910::vm::Value::Int(0),
                    native910::vm::Value::Str(String::new()),
                    native910::vm::Value::Str(String::new()),
                    native910::vm::Value::Str(String::new()),
                    native910::vm::Value::Str(String::new()),
                    native910::vm::Value::Int(0),
                    native910::vm::Value::Str(String::new()),
                    native910::vm::Value::Int(-1),
                ] {
                    push_trap_value(ints, strs, longs, value)?;
                }
                Ok(None)
            }
            "_enum" => {
                // `STRING` id — the only string-based output
                // type.
                const STRING_OUTPUT: i32 = 36;
                let output = ints
                    .len()
                    .checked_sub(3)
                    .and_then(|index| ints.get(index))
                    .copied()
                    .unwrap_or(0);
                for _ in 0..4 {
                    let _ = ints.pop();
                }
                if output == STRING_OUTPUT {
                    push_trap_value(ints, strs, longs, native910::vm::Value::Str(String::new()))?;
                } else {
                    push_trap_value(ints, strs, longs, native910::vm::Value::Int(-1))?;
                }
                Ok(None)
            }
            _ => Ok(Some(native910::vm::Value::Int(-1))),
        }
    }
}

/// Push one trap answer onto its typed stack, mirroring the VM's own push
/// (1000-slot original cap included — a full stack errors instead of panicking).
#[cfg(any(test, feature = "test-hooks"))]
fn push_trap_value(
    ints: &mut Vec<i32>,
    strs: &mut Vec<String>,
    longs: &mut Vec<i64>,
    value: native910::vm::Value,
) -> native910::vm::VmResult<()> {
    const MAX_TRAP_STACK: usize = 1000;
    match value {
        native910::vm::Value::Int(value) => {
            if ints.len() >= MAX_TRAP_STACK {
                return Err(native910::vm::VmError::StackFull { stack: "int" });
            }
            ints.push(value);
        }
        native910::vm::Value::Str(text) => {
            if strs.len() >= MAX_TRAP_STACK {
                return Err(native910::vm::VmError::StackFull { stack: "object" });
            }
            strs.push(text);
        }
        native910::vm::Value::Null => {
            return Err(native910::vm::VmError::NullObject {
                command: "login preview".into(),
            });
        }
        native910::vm::Value::Long(value) => {
            if longs.len() >= MAX_TRAP_STACK {
                return Err(native910::vm::VmError::StackFull { stack: "long" });
            }
            longs.push(value);
        }
    }
    Ok(())
}
