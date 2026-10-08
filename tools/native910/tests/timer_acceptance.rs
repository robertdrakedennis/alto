//! Real cache scripts 5093/5094, executed against a controlled dynamic component
//! and compared with traces recorded once from the original client
//! (`tests/fixtures/recorded/timer-{baseline,edited}.dig`, one digest per instruction). Not live
//! event-loop coverage.
mod common;

use native910::config::ConfigTypes;
use native910::inames::InterfaceRegistry;
use native910::opcode::OpcodeBook;
use native910::pack::PackArchive;
use native910::runtime::RuntimeHost;
use native910::script::{CompiledScript, decode_script};
use native910::source::{assemble_source, dump_script};
use native910::symbols::SymbolRegistry;
use native910::vars::VarScope;
use native910::vm::{Session, Value, VarLane, Vm};
use std::collections::{BTreeMap, HashMap};

fn native_trace(mut main: CompiledScript, mut callee: CompiledScript) -> (String, String) {
    main.name = Some("5093".into());
    callee.name = Some("5094".into());
    let mut host = RuntimeHost::default();
    host.definitions
        .insert((VarScope::Client, 995), VarLane::Int);
    host.variables
        .insert((VarScope::Client, 995, false), Value::Int(400));
    host.component_text.insert((0, 0), String::new());
    let provider = HashMap::from([(5094, callee)]);
    let mut session = Session::new(&main, &[Value::Int(0), Value::Int(0)]).unwrap();
    let mut lines = Vec::new();
    while !session.finished() {
        let s = session.snapshot();
        let Value::Int(value) = host.variables[&(VarScope::Client, 995, false)] else {
            panic!("integer var");
        };
        lines.push(format!(
            "{}\t{}\t{:?}\t{}\t{:?}\t{}\t{:?}\t{}\t{:?}\t{}\t{}",
            s.script_name.unwrap(),
            s.pc,
            s.ints,
            common::utf16_objects(s.strings.iter().map(Option::as_deref)),
            s.longs,
            s.frames.len(),
            s.int_locals,
            common::utf16_objects(s.string_locals.iter().map(Option::as_deref)),
            s.long_locals,
            value,
            host.component_text[&(0, 0)]
        ));
        Vm::new(&mut host, &provider).step(&mut session).unwrap();
    }
    (
        lines.join("\n") + "\n",
        host.component_text[&(0, 0)].clone(),
    )
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn edited_retail_timer_roundtrips_repacks_and_matches_the_recording() {
    let packs = common::require_pack(&[
        "client.scripts.js5",
        "client.config.js5",
        "client.enum.config.js5",
        "client.dbtableindex.js5",
    ]);
    let base = std::fs::read(packs.join("client.scripts.js5")).expect("required scripts");
    let archive = PackArchive::from_bytes(base.clone()).unwrap();
    let main_bytes = archive
        .group_files(5093)
        .unwrap()
        .unwrap()
        .remove(&0)
        .unwrap();
    let callee_bytes = archive
        .group_files(5094)
        .unwrap()
        .unwrap()
        .remove(&0)
        .unwrap();
    let book = OpcodeBook::embedded().unwrap();
    let main = decode_script(&main_bytes, &book).unwrap();
    let callee = decode_script(&callee_bytes, &book).unwrap();
    let symbols = SymbolRegistry::build(
        vec![(5094, "format_timer".into())],
        &BTreeMap::from([(5093, main.args), (5094, callee.args)]),
    )
    .unwrap();
    let configs = ConfigTypes::load(&packs).unwrap();
    let inames = InterfaceRegistry::empty();
    let source = dump_script(&main_bytes, &book, &symbols, &configs, &inames).unwrap();
    assert_eq!(
        assemble_source(&source, &book, &symbols, &configs, &inames).unwrap(),
        main_bytes
    );
    assert!(source.contains("push_constant_string(1);"), "{source}");
    let edited_source = source.replacen("push_constant_string(1);", "push_constant_string(51);", 1);
    let edited_bytes = assemble_source(&edited_source, &book, &symbols, &configs, &inames).unwrap();
    let rebuilt =
        native910::repack::scripts(&base, &BTreeMap::from([(5093, edited_bytes.clone())])).unwrap();
    let loaded = PackArchive::from_bytes(rebuilt.clone())
        .unwrap()
        .group_files(5093)
        .unwrap()
        .unwrap()
        .remove(&0)
        .unwrap();
    assert_eq!(loaded, edited_bytes);
    // Handed to `verify.sh`, which loads the rebuilt pack with the server's
    // own script verifier.
    let out = common::native_dir().join("target/timer-acceptance");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("client.scripts.js5"), rebuilt).unwrap();
    for (label, bytes, expected) in [
        ("baseline", main_bytes, "0:00:07"),
        ("edited", edited_bytes, "0:00:06"),
    ] {
        let (native, text) = native_trace(decode_script(&bytes, &book).unwrap(), callee.clone());
        // The recording is stored as one line digest per instruction.
        let base = common::fixture("recorded").join(format!("timer-{label}"));
        let native_lines: Vec<&str> = native.lines().collect();
        assert_eq!(
            common::first_trace_divergence(&base, &native_lines),
            None,
            "{label}: first divergence from the recording"
        );
        assert_eq!(text, expected);
    }
    assert_eq!(
        std::fs::read(packs.join("client.scripts.js5")).unwrap(),
        base,
        "base remains available for rollback"
    );
}
