//! Milestone-2a corpus gate: every component in the real 910 interfaces pack
//! must decode with its group's parentlayer and re-encode byte-identical.
//!
//! Byte-identity alone cannot catch semantic misreads (a shifted walk over
//! zero-filled regions still round-trips), so the meaning gate below pins
//! decoded CONTENT on representative components alongside the byte gate.
//!
//! Pack-dependent: fails loudly without server/data/pack; `--features no-pack` reports it ignored.

mod common;

use native910::interface::{ComponentBody, decode_component, encode_component};
use native910::pack::PackArchive;

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_interfaces_are_byte_exact() {
    let path = common::pack_root().join("client.interfaces.js5");
    common::require_present(&path);
    let archive = PackArchive::open(&path).expect("open interfaces pack");

    let mut groups = 0_usize;
    let mut components = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for group in archive.group_ids() {
        groups += 1;
        let parentlayer = (group << 16) as i32;
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        for (file, bytes) in &files {
            components += 1;
            let component = match decode_component(bytes, parentlayer) {
                Ok(component) => component,
                Err(error) => {
                    failures.push(format!("{group}/{file}: decode: {error}"));
                    continue;
                }
            };
            match encode_component(&component, parentlayer) {
                Ok(out) if out == *bytes => {}
                Ok(out) => failures.push(format!(
                    "{group}/{file}: re-encoded {} bytes, original {}",
                    out.len(),
                    bytes.len()
                )),
                Err(error) => {
                    failures.push(format!("{group}/{file}: re-encode: {error}"));
                }
            }
        }
    }

    for failure in failures.iter().take(200) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} interface failure(s) over {components} components in {groups} groups",
        failures.len()
    );
    eprintln!("interface corpus: {components} components in {groups} groups, byte-exact");
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_meaning_spot_checks() {
    let path = common::pack_root().join("client.interfaces.js5");
    common::require_present(&path);
    let archive = PackArchive::open(&path).expect("open interfaces pack");

    // 94/6: keybind entry, one "Skip" op, mouseover none. Pins the single-head
    // keybind loop: a gate-read-then-head implementation misreads this file
    // into garbage that still round-trips, so only content asserts catch it.
    let files = archive
        .group_files(94)
        .expect("unpack group 94")
        .expect("present");
    let component = decode_component(&files[&6], 94 << 16).expect("decode 94/6");
    assert_eq!(component.keybinds.len(), 1);
    assert_eq!(component.keybinds[0].head, 0x10);
    assert_eq!(component.ops, vec!["Skip".to_string()]);
    assert_eq!(component.mouseovercursor, -1);
    assert!(matches!(component.body, ComponentBody::Layer { .. }));

    // 64/6: one keybind entry, empty opbase, no ops — and still exact.
    let files = archive
        .group_files(64)
        .expect("unpack group 64")
        .expect("present");
    let component = decode_component(&files[&6], 64 << 16).expect("decode 64/6");
    assert_eq!(component.keybinds.len(), 1);
    assert_eq!(component.keybinds[0].head, 0x10);
    assert_eq!(component.opbase, "");
    assert!(component.ops.is_empty());
    // Raw layer 1 resolved against parentlayer 64 << 16.
    assert_eq!(component.layer, (64 << 16) + 1);
}
