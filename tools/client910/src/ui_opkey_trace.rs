//! Real cache scripts that call `if_setopkey`, run through the production
//! hook host chain (`ui_hooks::Pool` → `ui_hook_host::HookHost` →
//! `ui_components::ScriptHost` → `ui_properties` dispatch) and compared
//! instruction by instruction with traces recorded ONCE from the working original
//! client.
//!
//! `if_setopkey` pops FOUR ints: key, modifier and the packed component,
//! then the op slot. A host that pops three leaks one int per call
//! (script 12747 calls it seven times and must return with empty stacks).
//! After the root returns, the harness prints every component a handler gave
//! key bindings (`com` lines: packed id, `opKeys`, `opKeyMods`,
//! `hasKeybinds`), which the Rust component state must reproduce.
//!
//! Fixtures (`fixtures/cs2-opkey-recorded/`, a few hundred bytes):
//! * `cases.txt` — `name<TAB>root script id<TAB>typed args` (`-` = none).
//!   The raw bytes of each root (no gosubs) are read from
//!   `server/data/pack/client.scripts.js5` at test time and never stored
//!   here, so this test needs the local revision-910 cache.
//! * `traces/<name>.dig` — recorded harness output, one FNV-1a 64 digest per
//!   line (the per-instruction trace lines, then the `com` lines), so a
//!   mismatch still names the first divergent line. Scripts are named by
//!   cache id; blank components for interfaces 892 and 1786 stand in for the
//!   cache.
//!

use crate::ui_changes::Changes;
use crate::ui_components::{Arg, Component, Interface, Store};
use crate::ui_hook_host::{Domains, HookHost};
use crate::ui_hooks::{Pool, Request};
use crate::ui_runtime::Engine;
use native910::opcode::OpcodeBook;
use native910::script::{decode_script, CompiledScript};
use native910::vm::{Snapshot, Vm};
use rs910_symbols::interface;
use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;

/// Interfaces the cases touch (`58458151 >> 16`, `117047358 >> 16`); the
/// recording harness installs the same ids as 256 blank components each.
const INTERFACES: [i32; 2] = [
    interface::MINIGAME_SCOREBOARD.id(),
    interface::TREASURE_HUNTER_OVERLAY.id(),
];

fn fixture() -> PathBuf {
    rs910_core::test_support::client_dir().join("fixtures/cs2-opkey-recorded")
}

struct Case {
    name: String,
    root: i32,
    ints: Vec<i32>,
}

fn cases() -> Vec<Case> {
    std::fs::read_to_string(fixture().join("cases.txt"))
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            let ints = if cols[2] == "-" {
                Vec::new()
            } else {
                cols[2]
                    .split(',')
                    .map(|arg| arg.strip_prefix("i:").unwrap().parse().unwrap())
                    .collect()
            };
            Case {
                name: cols[0].to_string(),
                root: cols[1].parse().unwrap(),
                ints,
            }
        })
        .collect()
}

fn script(book: &OpcodeBook, id: i32) -> CompiledScript {
    let root = rs910_js5::test_support::require_pack_file("client.scripts.js5");
    let bytes = native910::pack::PackArchive::open(&root.join("client.scripts.js5"))
        .unwrap()
        .group_files(id as u32)
        .unwrap()
        .unwrap_or_else(|| panic!("script {id} is not in the pack"))
        .remove(&0)
        .unwrap();
    let mut script = decode_script(&bytes, book).unwrap();
    // The recording harness names scripts by cache id.
    script.name = Some(id.to_string());
    script
}

/// Blank components with `parentlayer` = packed id, as the harness installs.
fn store() -> Store {
    let mut store = Store::default();
    for iface in INTERFACES {
        let components = (0..256)
            .map(|child| {
                let mut component = Component::default();
                component.f.parentlayer = (iface << 16) | child;
                Some(Rc::new(RefCell::new(component)))
            })
            .collect();
        store.interfaces.insert(iface, Interface::new(components));
    }
    store
}

/// String rendering of the harness (`s` + UTF-16 units, `n` = null).
fn objects<'a>(values: impl Iterator<Item = Option<&'a str>>) -> String {
    let rendered: Vec<String> = values
        .map(|value| match value {
            None => "n".to_string(),
            Some(value) => {
                native910::jstr::units(value)
                    .iter()
                    .fold("s".to_string(), |mut out, unit| {
                        write!(out, "{unit:04x}").unwrap();
                        out
                    })
            }
        })
        .collect();
    format!("[{}]", rendered.join(","))
}

fn trace_line(s: &Snapshot) -> String {
    format!(
        "{}\t{}\t{:?}\t{}\t{:?}\t{}\t{:?}\t{}\t{:?}",
        s.script_name.as_deref().unwrap_or("null"),
        s.pc,
        s.ints,
        objects(s.strings.iter().map(Option::as_deref)),
        s.longs,
        s.frames.len(),
        s.int_locals,
        objects(s.string_locals.iter().map(Option::as_deref)),
        s.long_locals
    )
}

/// Array rendering per slot, `n` for a null slot (harness `bytes`).
fn key_slots(slots: &[Option<Vec<i8>>]) -> String {
    let rendered: Vec<String> = slots
        .iter()
        .map(|slot| {
            slot.as_ref()
                .map_or_else(|| "n".to_string(), |v| format!("{v:?}"))
        })
        .collect();
    format!("[{}]", rendered.join(","))
}

/// Harness `com` lines for every component whose `opKeys` were allocated.
fn component_lines(store: &Store) -> Vec<String> {
    let mut lines = Vec::new();
    for iface in INTERFACES {
        let interface = store.interfaces[&iface].borrow();
        for component in interface.components.borrow().iter().flatten() {
            let c = component.borrow();
            let Some(keys) = &c.keys else { continue };
            lines.push(format!(
                "com\t{}\t{}\t{}\t{}",
                c.f.parentlayer,
                key_slots(keys),
                key_slots(c.key_mods.as_ref().unwrap()),
                c.f.hasKeybinds
            ));
        }
    }
    lines
}

/// Run `case` through the production hook host, stepping exactly as
/// `Pool::execute` does, and return (per-step trace, `com` lines).
fn client_trace(book: &OpcodeBook, case: &Case) -> (Vec<String>, Vec<String>) {
    let script = script(book, case.root);
    let mut store = store();
    let mut state = crate::ui_properties::State::default();
    let mut engine = Engine::default();
    let mut changes = Changes::default();
    let mut now = || 0_i64;
    let mut pool = Pool::default();
    let request = Request {
        args: Some(
            std::iter::once(case.root)
                .chain(case.ints.iter().copied())
                .map(Arg::Int)
                .collect(),
        ),
        ..Request::default()
    };
    let index = pool.acquire();
    pool.prepare(index, &script, &request).unwrap();
    let mut session = pool
        .session(
            index,
            case.root,
            &script,
            crate::ui_hooks::INTERACTIVE_LIMIT,
        )
        .unwrap();
    let mut lines = Vec::new();
    while !session.finished() {
        lines.push(trace_line(&session.snapshot()));
        let mut host = HookHost {
            engine: &mut engine,
            store: &mut store,
            properties: &mut state,
            context: &mut pool.contexts[index],
            domains: Domains::Plain {
                changes: &mut changes,
                now: &mut now,
            },
        };
        if let Err(error) = Vm::new(&mut host, &()).step(&mut session) {
            panic!(
                "{}: client host failed after {} steps: {error}",
                case.name,
                lines.len()
            );
        }
    }
    (lines, component_lines(&store))
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn if_setopkey_scripts_match_the_recording_through_the_client_host() {
    let book = OpcodeBook::embedded().unwrap();
    let cases = cases();
    assert_eq!(cases.len(), 3);
    for case in &cases {
        let recorded: Vec<String> =
            std::fs::read_to_string(fixture().join("traces").join(format!("{}.dig", case.name)))
                .unwrap()
                .lines()
                .map(String::from)
                .collect();
        let (rust_steps, rust_coms) = client_trace(&book, case);
        let lines: Vec<&String> = rust_steps.iter().chain(&rust_coms).collect();
        for (at, (expected, line)) in recorded.iter().zip(&lines).enumerate() {
            let actual = format!(
                "{:016x}",
                rs910_core::test_support::frozen::fnv64(line.as_bytes())
            );
            assert_eq!(
                &actual, expected,
                "{}: first divergence from the recording at line {at} (step lines come first, then `com` lines): {line}",
                case.name
            );
        }
        assert_eq!(lines.len(), recorded.len(), "{}: trace length", case.name);
        // The recorded last step is the root `return` with every stack empty: each
        // `if_setopkey` consumed exactly its four ints.
        let last = rust_steps.last().unwrap();
        assert!(last.contains("\t[]\t[]\t[]\t0\t"), "{}: {last}", case.name);
        assert!(
            !rust_coms.is_empty(),
            "{}: the recording bound no keys",
            case.name
        );
    }
}
