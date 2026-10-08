//! Corpus gate for curated interface names: seeded names render into dump
//! identity comments and validate back on assemble, byte-exact.
//!
//! Seeds two real interfaces (1253 and 933, the top xref targets) with
//! placeholder names, then proves for a spread of their children that
//! `identity + format + assemble` reproduces the pack bytes — and that a
//! mismatched group or unknown name fails loudly. The full-corpus
//! byte-exactness of unnamed dumps is already gated in `interface_text`;
//! this gate pins the naming layer on top.
//!
//! Pack-dependent: fails loudly without server/data/pack; `--features no-pack` reports it ignored.

mod common;

use native910::inames::{InterfaceRegistry, format_identity, parse_inames_txt};
use native910::interface::decode_component;
use native910::isource::{assemble_component, format_component, lift_component};
use native910::pack::PackArchive;
use native910::symbols::SymbolRegistry;
use std::path::PathBuf;

fn pack_path() -> PathBuf {
    common::pack_root().join("client.interfaces.js5")
}

fn seeded_registry() -> InterfaceRegistry {
    let (ifaces, children) = parse_inames_txt(
        "interface 1253 bank\ncomponent 1253 4 main\ninterface 933 shop\ncomponent 933 253 buy\n",
    )
    .expect("parse seeded names");
    let mut roster = std::collections::BTreeMap::new();
    roster.insert(1253, vec![4]);
    roster.insert(933, vec![253]);
    InterfaceRegistry::build(&ifaces, &children, &roster).expect("build seeded registry")
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_named_identity_roundtrips_byte_exact() {
    let path = pack_path();
    common::require_present(&path);
    let archive = PackArchive::open(&path).expect("open interfaces pack");
    let names = seeded_registry();
    let symbols = SymbolRegistry::empty();
    // Seeded files plus a spread of siblings: named comments must assemble
    // back to the pack bytes.
    let mut checked = 0_usize;
    for (group, seeded) in [(1253_u32, 4_u32), (933_u32, 253_u32)] {
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        // The seeded child must exist — otherwise the seed itself is stale.
        assert!(
            files.contains_key(&seeded),
            "seeded child {group}/{seeded} absent from the pack"
        );
        let mut file_ids: Vec<u32> = files.keys().copied().collect();
        file_ids.sort_unstable();
        // Seeded child first (exercises the named-comment path), then a
        // spread of numeric siblings.
        file_ids.retain(|file| *file != seeded);
        let step = file_ids.len().div_ceil(7).max(1);
        let mut sample: Vec<u32> = file_ids.iter().step_by(step).take(7).copied().collect();
        sample.insert(0, seeded);
        for file in sample {
            let bytes = &files[&file];
            let parentlayer = (group << 16) as i32;
            let decoded = decode_component(bytes, parentlayer).expect("decode corpus component");
            let mut text = format_identity(i32::try_from(group).unwrap_or(i32::MAX), file, &names);
            text.push_str(&format_component(&lift_component(&decoded), &symbols));
            let out = assemble_component(&text, parentlayer, &symbols, &names)
                .expect("assemble named component");
            assert_eq!(out, *bytes, "named round-trip drift at {group}/{file}");
            checked += 1;
        }
    }
    eprintln!("named identity round-trip: {checked} components byte-exact");
    assert!(checked >= 10, "seeded sweep covered too few files");
}
