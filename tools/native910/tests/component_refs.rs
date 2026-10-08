//! Corpus gate for symbolic packed component ids: with two real interfaces
//! named, dumped scripts render `bank/7`-style spellings wherever an int
//! literal carries a roster-known packed id, and the symbolic text
//! reassembles byte-identical.
//!
//! Seeds interfaces 1253 (`bank`) and 933 (`shop`) — the top xref targets,
//! mirroring the `inames_corpus` identity seeds — over the REAL pack roster
//! (so numeric children validate against true pack membership), then proves
//! for scripts pushing those packed ids that `dump | assemble` is the
//! identity on bytes with the symbolic spelling visibly engaged.
//!
//! Pack-dependent: fails loudly without server/data/pack; `--features no-pack` reports it ignored.

mod common;

use native910::config::ConfigTypes;
use native910::inames::{InterfaceRegistry, load_interface_roster, parse_inames_txt};
use native910::opcode::OpcodeBook;
use native910::pack::PackArchive;
use native910::script::{Operand, decode_script};
use native910::source::{assemble_source, dump_script};
use native910::symbols::SymbolRegistry;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn pack_root() -> PathBuf {
    common::pack_root()
}

fn scripts_path() -> PathBuf {
    pack_root().join("client.scripts.js5")
}

/// Real roster plus curated names for 1253 (`bank`) and 933 (`shop`).
/// Children stay numeric on purpose: the gate proves the `Bank/7` shape,
/// whose numeric side validates against roster membership alone.
fn seeded_registry() -> InterfaceRegistry {
    let roster = load_interface_roster(&pack_root()).expect("load interface roster");
    assert!(
        roster.contains_key(&1253) && roster.contains_key(&933),
        "seeded interfaces 1253/933 absent from the pack roster"
    );
    let (ifaces, children) =
        parse_inames_txt("interface 1253 bank\ninterface 933 shop\n").expect("parse seeded names");
    InterfaceRegistry::build(&ifaces, &children, &roster).expect("build seeded registry")
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_symbolic_components_roundtrip_byte_exact() {
    let path = scripts_path();
    common::require_present(&path);
    let archive = PackArchive::open(&path).expect("open scripts pack");
    let book = OpcodeBook::embedded().expect("load opcode book");
    let names = seeded_registry();
    let symbols = SymbolRegistry::empty();
    let configs = ConfigTypes::load(&pack_root()).expect("load config tables");

    // Find scripts pushing packed ids of the seeded interfaces (either const
    // spelling — both symbolize on dump).
    let mut candidates: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    let mut hits: BTreeMap<i32, usize> = BTreeMap::new();
    let mut decoded: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    for group in archive.group_ids() {
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        for (file, bytes) in &files {
            let script = decode_script(bytes, &book).expect("decode corpus script");
            let mut seeded = false;
            for instr in &script.code {
                let is_push = matches!(
                    instr.command.as_str(),
                    "push_constant_string" | "push_constant_int"
                );
                if !is_push {
                    continue;
                }
                if let Operand::Int(value) = &instr.operand
                    && let Some((iface, _)) = names.unpack_known(*value)
                {
                    *hits.entry(iface).or_insert(0) += 1;
                    if iface == 1253 || iface == 933 {
                        seeded = true;
                    }
                }
            }
            decoded.push((group, *file, bytes.clone()));
            if seeded && candidates.len() < 8 {
                candidates.push((group, *file, bytes.clone()));
            }
        }
    }
    let mut top: Vec<(i32, usize)> = hits.into_iter().collect();
    top.sort_by_key(|(_, count)| usize::MAX - *count);
    eprintln!(
        "packed-int pushes per interface (top 5): {:?}",
        &top[..top.len().min(5)]
    );
    assert!(
        !candidates.is_empty(),
        "no corpus script pushes a packed id of 1253/933 — seeds are stale"
    );

    // Every candidate dumps (symbolically) and reassembles byte-identical.
    // Through the EMPTY registry the same bytes dump numerically and still
    // round-trip: naming changes text, never bytes.
    let empty = InterfaceRegistry::empty();
    let mut markers = 0_usize;
    for (group, file, bytes) in &candidates {
        let text = dump_script(bytes, &book, &symbols, &configs, &names)
            .unwrap_or_else(|error| panic!("dump {group}/{file}: {error}"));
        markers += text.matches("bank/").count() + text.matches("shop/").count();
        let out = assemble_source(&text, &book, &symbols, &configs, &names)
            .unwrap_or_else(|error| panic!("assemble {group}/{file}: {error}"));
        assert_eq!(out, *bytes, "symbolic round-trip drift at {group}/{file}");
        let plain = dump_script(bytes, &book, &symbols, &configs, &empty)
            .unwrap_or_else(|error| panic!("dump plain {group}/{file}: {error}"));
        assert!(
            !plain.contains("bank/") && !plain.contains("shop/"),
            "empty-registry dump of {group}/{file} symbolized without names"
        );
        let out = assemble_source(&plain, &book, &symbols, &configs, &empty)
            .unwrap_or_else(|error| panic!("assemble plain {group}/{file}: {error}"));
        assert_eq!(out, *bytes, "numeric round-trip drift at {group}/{file}");
    }
    eprintln!(
        "symbolic components: {} script(s), {markers} bank//shop/ spellings, byte-exact",
        candidates.len()
    );
    assert!(
        markers >= 2,
        "symbolic rendering never engaged over {} candidate script(s)",
        candidates.len()
    );

    // Full-corpus sweep under the seeded registry: symbolization must never
    // move a byte, even where no seeded interface is involved (unnamed pairs
    // render numeric, unknown ints stay plain).
    let mut swept = 0_usize;
    let mut swept_markers = 0_usize;
    for (group, file, bytes) in &decoded {
        let text = dump_script(bytes, &book, &symbols, &configs, &names)
            .unwrap_or_else(|error| panic!("sweep dump {group}/{file}: {error}"));
        swept_markers += text.matches("bank/").count() + text.matches("shop/").count();
        let out = assemble_source(&text, &book, &symbols, &configs, &names)
            .unwrap_or_else(|error| panic!("sweep assemble {group}/{file}: {error}"));
        assert_eq!(out, *bytes, "sweep drift at {group}/{file}");
        swept += 1;
    }
    eprintln!("sweep: {swept} scripts byte-exact with names engaged ({swept_markers} spellings)");
    assert!(
        swept > 1000,
        "suspiciously small 910 corpus: {swept} scripts"
    );
}
