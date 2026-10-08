//! Text-level corpus gate: for every script in the real 910 runtime pack,
//! `dump` (binary → source) followed by `assemble` (source → verified binary)
//! must reproduce the input bytes exactly.
//!
//! The registry names EVERY gosub target (`callee_<id>`) and carries inferred
//! returns, exactly like the dump flow (`registry_for_scripts_pack`) — so
//! this exercises the full fold path at corpus scale: every foldable call
//! renders as `~name(args)`, computed-argument calls stay numeric, pure
//! push/operate runs lift to `$x = <expr>;`, single-value calls fuse with
//! their consuming pop into `$x = ~name(args);`, single-value calls nest
//! inside larger operator trees (`$y = ~f(a) + $c;`, `$x = max(0, ~g());`),
//! and everything reassembles byte-identical. It also pins the group-id call
//! rule (any target outside the group roster fails the build on purpose).
//!
//! Pack-dependent: fails loudly without server/data/pack; `--features no-pack` reports it ignored.

mod common;

use native910::config::ConfigTypes;
use native910::expr::Expr;
use native910::inames::InterfaceRegistry;
use native910::opcode::OpcodeBook;
use native910::pack::PackArchive;
use native910::returns::ReturnArity;
use native910::script::{Operand, decode_script};
use native910::source::{assemble_source, dump_script, lift};
use native910::symbols::SymbolRegistry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn pack_path() -> PathBuf {
    common::pack_root().join("client.scripts.js5")
}

fn pack_root() -> PathBuf {
    common::pack_root()
}

fn load_configs() -> ConfigTypes {
    ConfigTypes::load(&pack_root()).expect("load config tables")
}

fn inames() -> InterfaceRegistry {
    InterfaceRegistry::empty()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_source_roundtrip_is_byte_exact() {
    let path = pack_path();
    common::require_present(&path);
    let archive = PackArchive::open(&path).expect("open scripts pack");
    let book = OpcodeBook::embedded().expect("load opcode book");

    // Registry over the whole corpus: every group known with its arg counts,
    // every gosub target named. Named-but-unfoldable calls (computed
    // arguments) must survive as numeric gosubs.
    let mut known = BTreeMap::new();
    let mut targets: BTreeSet<i32> = BTreeSet::new();
    for group in archive.group_ids() {
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        for (file, bytes) in &files {
            assert_eq!(
                *file, 0,
                "group {group} holds file {file}: the one-file-per-group rule broke"
            );
            let script = decode_script(bytes, &book).expect("decode corpus script");
            known.insert(group as i32, script.args);
            for instr in &script.code {
                if let Operand::Script(id) = &instr.operand {
                    targets.insert(*id);
                }
            }
        }
    }
    let groups: BTreeSet<i32> = known.keys().copied().collect();
    let unknown: Vec<i32> = targets.difference(&groups).copied().collect();
    assert!(
        unknown.is_empty(),
        "gosub targets outside the group roster: {unknown:?}"
    );
    let curated: Vec<(i32, String)> = targets
        .iter()
        .map(|id| (*id, format!("callee_{id}")))
        .collect();
    // Returns ride along exactly as in the dump flow: inference over the
    // decoded corpus, Known results attached (Unknown stays absent, so
    // unknowable callees keep their statement+pop form).
    let flat: BTreeMap<i32, native910::script::CompiledScript> = archive
        .group_ids()
        .filter_map(|group| {
            archive
                .group_files(group)
                .expect("unpack group")
                .unwrap_or_default()
                .into_iter()
                .next()
                .map(|(_, bytes)| {
                    (
                        group as i32,
                        decode_script(&bytes, &book).expect("decode corpus script"),
                    )
                })
        })
        .collect();
    let (inferred, converged) = native910::dataflow::infer_returns(&flat, &load_configs());
    assert!(converged, "inference must converge over the corpus");
    let returns: BTreeMap<i32, native910::script::Counts> = inferred
        .iter()
        .filter_map(|(id, arity)| match arity {
            ReturnArity::Known { int, obj, long } => Some((
                *id,
                native910::script::Counts {
                    int: *int,
                    obj: *obj,
                    long: *long,
                },
            )),
            ReturnArity::Unknown | ReturnArity::Never => None,
        })
        .collect();
    let symbols = SymbolRegistry::build(curated, &known)
        .expect("build registry")
        .with_returns(&returns);
    let configs = load_configs();
    eprintln!(
        "calls: {} distinct targets named, {} known scripts, {} known returns",
        targets.len(),
        known.len(),
        returns.len()
    );

    let mut scripts = 0_usize;
    let mut folded = 0_usize;
    let mut numeric = 0_usize;
    let mut assigned = 0_usize;
    let mut value_calls = 0_usize;
    let mut nested_calls = 0_usize;
    let mut value_samples: Vec<String> = Vec::new();
    let mut nested_samples: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for group in archive.group_ids() {
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        for (file, bytes) in &files {
            scripts += 1;
            let text = match dump_script(bytes, &book, &symbols, &configs, &inames()) {
                Ok(text) => text,
                Err(error) => {
                    failures.push(format!("{group}/{file}: dump: {error}"));
                    continue;
                }
            };
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('~') {
                    folded += 1;
                } else if line.starts_with("gosub_with_params(") {
                    numeric += 1;
                } else if line.starts_with('$') {
                    assigned += 1;
                    if line.contains("= ~") {
                        value_calls += 1;
                        if value_samples.len() < 3 {
                            value_samples.push(format!("{group}/{file}: {line}"));
                        }
                    }
                }
            }
            // Nested-call lift count, model-side (precise where text
            // heuristics blur): an `Assign` whose expression contains a
            // `~call` but is not itself a bare call. Text `= ~` misses deep
            // nests (`$x = max(0, ~g());`) and conflates leading nests
            // (`$y = ~f(a) + $c;`) with direct fusion, so the model decides.
            match decode_script(bytes, &book) {
                Ok(script) => match lift(&script, &symbols, &configs, &inames()) {
                    Ok(lifted) => {
                        for stmt in &lifted.body {
                            if let native910::source::SourceStmt::Assign { target, expr } = stmt {
                                if matches!(expr, Expr::Call { .. }) {
                                    continue;
                                }
                                if expr_has_call(expr) {
                                    nested_calls += 1;
                                    if nested_samples.len() < 3 {
                                        nested_samples.push(format!(
                                            "{group}/{file}: ${target} = {};",
                                            native910::expr::format_expr(expr, &inames())
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => {
                        failures.push(format!("{group}/{file}: lift for nested count: {error}"));
                    }
                },
                Err(error) => {
                    failures.push(format!("{group}/{file}: decode for nested count: {error}"));
                }
            }
            match assemble_source(&text, &book, &symbols, &configs, &inames()) {
                Ok(out) if out == *bytes => {}
                Ok(out) => failures.push(format!(
                    "{group}/{file}: reassembled {} bytes, original {}",
                    out.len(),
                    bytes.len()
                )),
                Err(error) => failures.push(format!("{group}/{file}: assemble: {error}")),
            }
        }
    }

    for failure in failures.iter().take(20) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        failures.is_empty(),
        "{} source round-trip failure(s) over {scripts} scripts",
        failures.len()
    );
    for sample in &value_samples {
        eprintln!("VALUE {sample}");
    }
    for sample in &nested_samples {
        eprintln!("NESTED {sample}");
    }
    eprintln!(
        "source corpus: {scripts} scripts byte-exact ({folded} folded calls, {numeric} numeric gosubs, {assigned} lifted assignments, {value_calls} value calls, {nested_calls} nested calls)"
    );
    // Every gosub target is named, so calls fold to `~callee_N(args)` and
    // still reassemble byte-exact (checked above per script).
    assert!(
        folded > 0,
        "no named call folded to a `~name(args)` spelling"
    );
    assert!(
        assigned >= 40_000,
        "lifting engaged far below the measured 44k windows: {assigned}"
    );
    // Measured 41 over this corpus: pure-arg call sites against Known
    // single-value callees with a consuming pop are rare (most gosub results
    // feed computed expressions or come from unknowable callees). The pin
    // guards the fusion against silent death, not the exact count.
    assert!(
        value_calls >= 30,
        "value-call fusion engaged far below the measured 41: {value_calls}"
    );
    // Measured 4 nested lifts over this corpus (single-value calls with
    // canonical args inside larger operator trees): rarer still, but the pin
    // guards the nesting against silent death, not the exact count.
    assert!(
        nested_calls >= 3,
        "nested-call lift engaged far below the measured 4: {nested_calls}"
    );
}

fn expr_has_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call { .. } => true,
        Expr::Unary(_, inner) => expr_has_call(inner),
        Expr::Binary(_, left, right) => expr_has_call(left) || expr_has_call(right),
        Expr::Nary(_, args) => args.iter().any(expr_has_call),
        Expr::Join(parts) => parts.iter().any(expr_has_call),
        Expr::Param { obj, .. } => expr_has_call(obj),
        Expr::Enum {
            input, output, key, ..
        } => expr_has_call(input) || expr_has_call(output) || expr_has_call(key),
        Expr::LitInt(_) | Expr::LitLong(_) | Expr::LitStr(_) | Expr::Local(_) => false,
        // Component refs are int-literal spellings: they can never contain a
        // call, like every other leaf.
        Expr::Component { .. } => false,
    }
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_call_value_pin_proves_join_end_to_end() {
    let path = pack_path();
    common::require_present(&path);
    let archive = PackArchive::open(&path).expect("open scripts pack");
    let book = OpcodeBook::embedded().expect("load opcode book");

    // Decode everything once, then infer over the whole corpus: value calls
    // need callee returns, which only the fixed-point sees.
    let mut scripts = BTreeMap::new();
    let mut callers_of: BTreeMap<i32, usize> = BTreeMap::new();
    for group in archive.group_ids() {
        let files = archive
            .group_files(group)
            .expect("unpack group")
            .unwrap_or_default();
        for (file, bytes) in &files {
            let script = decode_script(bytes, &book).expect("decode corpus script");
            for instr in &script.code {
                if let Operand::Script(id) = &instr.operand
                    && instr.command == "gosub_with_params"
                {
                    *callers_of.entry(*id).or_default() += 1;
                }
            }
            scripts.insert((group, *file), script);
        }
    }
    let by_group: BTreeMap<i32, &native910::script::CompiledScript> = scripts
        .iter()
        .map(|((group, _), script)| (*group as i32, script))
        .collect();
    let flat: BTreeMap<i32, native910::script::CompiledScript> = by_group
        .iter()
        .map(|(id, script)| (*id, (*script).clone()))
        .collect();
    let (inferred, converged) = native910::dataflow::infer_returns(&flat, &load_configs());
    assert!(converged, "inference must converge over the corpus");

    // Find a real callee with exactly one inferred int return that real code
    // actually calls, preferring one that also takes arguments so the pin
    // exercises argument passing as well as the value use.
    let mut candidate: Option<i32> = None;
    let mut single_int_callees = 0_usize;
    let mut called_single_int = 0_usize;
    for (id, arity) in &inferred {
        if !matches!(
            arity,
            ReturnArity::Known {
                int: 1,
                obj: 0,
                long: 0
            }
        ) {
            continue;
        }
        single_int_callees += 1;
        if !callers_of.contains_key(id) {
            continue;
        }
        called_single_int += 1;
        if candidate.is_none() {
            candidate = Some(*id);
        }
    }
    eprintln!("single-int callees: {single_int_callees}, called: {called_single_int}");
    let callee = candidate.expect("no called single-int-return callee in corpus");
    let args = by_group[&callee].args;
    assert!(
        args.int + args.obj + args.long > 0,
        "pin wants an argument-taking callee, got void {callee}"
    );

    // Hand-write `$x = ~probe(args)` against the real signature and prove it
    // assembles byte-identical to the hand-expanded flat form. Argument kinds
    // follow the signature in (int, obj, long) order; any order would do
    // (multiset rule), but fixed order keeps both spellings in lockstep.
    let mut known = BTreeMap::new();
    known.insert(callee, args);
    let symbols = SymbolRegistry::build(vec![(callee, "probe_ret".to_string())], &known)
        .expect("build registry")
        .with_returns(
            &inferred
                .iter()
                .filter_map(|(id, arity)| match arity {
                    ReturnArity::Known { int, obj, long } => Some((
                        *id,
                        native910::script::Counts {
                            int: *int,
                            obj: *obj,
                            long: *long,
                        },
                    )),
                    ReturnArity::Unknown | ReturnArity::Never => None,
                })
                .collect::<BTreeMap<_, _>>(),
        );
    let mut arg_src = Vec::new();
    let mut flat_src = String::new();
    // Expression literals lower through the const-string family (never bare
    // `push_constant_int`), so the flat form must spell them the same way.
    for _ in 0..args.int {
        arg_src.push("1".to_string());
        flat_src.push_str("    push_constant_string(1);\n");
    }
    for _ in 0..args.obj {
        arg_src.push("\"s\"".to_string());
        flat_src.push_str("    push_constant_string(\"s\");\n");
    }
    for _ in 0..args.long {
        arg_src.push("1L".to_string());
        flat_src.push_str("    push_constant_string(1L);\n");
    }
    let value_text = format!(
        "[](int $a)\nint $x;\n\n    $x = ~probe_ret({});\n    return(0);\n",
        arg_src.join(", ")
    );
    let flat_text = format!(
        "[](int $a)\nint $x;\n\n{flat_src}    gosub_with_params({callee});\n    pop_int_local($x);\n    return(0);\n"
    );
    let from_call = assemble_source(
        &value_text,
        &book,
        &symbols,
        &ConfigTypes::empty(),
        &inames(),
    )
    .expect("assemble value call");
    let from_flat = assemble_source(
        &flat_text,
        &book,
        &symbols,
        &ConfigTypes::empty(),
        &inames(),
    )
    .expect("assemble flat form");
    assert_eq!(from_call, from_flat, "value call must equal its expansion");
    // And the produced bytes decode to the expected shape.
    let decoded = decode_script(&from_call, &book).expect("decode assembled");
    assert!(matches!(
        decoded.code.iter().find_map(|instr| match &instr.operand {
            Operand::Script(id) if *id == callee => Some(()),
            _ => None,
        }),
        Some(())
    ));
}
