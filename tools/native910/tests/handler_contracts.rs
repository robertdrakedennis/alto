//! xref's hand-maintained command tables against the original client's
//! handlers.
//!
//! `tests/fixtures/handler-contracts/*.txt` were recorded once by executing every
//! retail `if_*`/`cc_*` handler on a sentinel stack:
//! * `component-slots.txt` — `name depth|none|derived`: the int-stack depth
//!   (0 = top) of the first packed component id the handler resolves;
//! * `hook-installers.txt` — `name field|none`: the Component hook field the
//!   handler fills with the script id popped by its hook-argument reader.
//!
//! The recordings are not regenerated.
mod common;

use native910::opcode::OpcodeBook;
use native910::xref::{component_slot, hook_setter, hook_slot_word};
use std::collections::BTreeMap;

const FIXTURE: &str = "handler-contracts";

fn probe(file: &str) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(common::fixture(FIXTURE).join(file)).unwrap();
    let rows: BTreeMap<String, String> = text
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let (name, value) = line.split_once(' ').unwrap();
            (name.to_string(), value.to_string())
        })
        .collect();
    assert!(rows.len() > 390, "{file}: {} rows", rows.len());
    rows
}

/// Retail (`<= 1431`) `if_*`/`cc_*` book names — the commands the recordings cover.
fn interface_commands(book: &OpcodeBook) -> Vec<(String, u16)> {
    let mut names: Vec<(String, u16)> = book
        .commands()
        .filter(|name| name.starts_with("if_") || name.starts_with("cc_"))
        .map(|name| (name.to_string(), book.opcode_for(name).unwrap()))
        .filter(|(_, opcode)| *opcode <= 1431)
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn xref_component_slots_match_handlers() {
    let book = OpcodeBook::embedded().unwrap();
    let recorded = probe("component-slots.txt");
    // Component readers xref deliberately leaves out (module docs of
    // `native910::xref`, each verified there): `cc_*` operate on the ambient
    // active component (their second-slot reads are drag targets / parent
    // layers resolved through scope), `if_find`/`cc_find` treat the id as a
    // search key, and `if_debug_*` are debug-only with zero corpus uses.
    const EXCLUDED: &[&str] = &[
        "cc_create",
        "cc_find",
        "cc_setdraggable",
        "if_debug_button1",
        "if_debug_button10",
        "if_debug_button2",
        "if_debug_button3",
        "if_debug_button4",
        "if_debug_button5",
        "if_debug_button6",
        "if_debug_button7",
        "if_debug_button8",
        "if_debug_button9",
        "if_debug_target",
        "if_find",
    ];
    let mut problems = Vec::new();
    for (name, _) in interface_commands(&book) {
        let expected = recorded
            .get(&name)
            .unwrap_or_else(|| panic!("{name} missing from the recorded fixture"));
        let table = component_slot(&name).map(|slot| slot.to_string());
        let recorded_slot = expected.parse::<u8>().ok().map(|slot| slot.to_string());
        match (&table, &recorded_slot) {
            (Some(t), Some(j)) if t == j => {}
            (None, None) => {}
            (None, Some(_)) if EXCLUDED.contains(&name.as_str()) => {}
            _ => problems.push(format!("{name}: table {table:?}, recorded {expected}")),
        }
    }
    // The 910 dispatch names only: `if_sendto` and `if_discardhook` are handler
    // names of the original client, not commands (their dispatch entries are
    // `if_sendtofront` and the gesture-hook discard).
    for dead in ["if_sendto", "if_discardhook"] {
        assert!(book.opcode_for(dead).is_err(), "{dead} is a book name");
        assert_eq!(component_slot(dead), None, "{dead} is not a 910 command");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    // Spot values independent of the fixture file: `if_setposition` pops the
    // component first; `if_triggerop` reads it under the sub-index and op.
    assert_eq!(recorded["if_setposition"], "0");
    assert_eq!(recorded["if_triggerop"], "2");
}

#[test]
fn xref_hook_setters_match_handlers() {
    let book = OpcodeBook::embedded().unwrap();
    let recorded = probe("hook-installers.txt");
    let mut problems = Vec::new();
    for (name, _) in interface_commands(&book) {
        let installs = recorded[&name] != "none";
        if installs != hook_setter(&name) {
            problems.push(format!(
                "{name}: table {}, recorded {}",
                hook_setter(&name),
                recorded[&name]
            ));
        } else if installs && hook_slot_word(&name) != recorded[&name] {
            // The slot the xref edge is reported under must be the Component
            // field the handler fills.
            problems.push(format!(
                "{name}: slot {}, recorded {}",
                hook_slot_word(&name),
                recorded[&name]
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    // The pop-and-discard siblings install nothing.
    for name in ["if_setonverticalswipe", "if_setongamepadbutton"] {
        assert_eq!(recorded[name], "none", "{name}");
    }
    assert_eq!(recorded["if_setonclick"], "onclick");
    // Opcode 1218 stores `ondragcomplete`.
    assert_eq!(recorded["if_setondragcomplete_alias"], "ondragcomplete");
}
