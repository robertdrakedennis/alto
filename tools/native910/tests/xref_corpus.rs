//! Corpus engagement and integrity gate for config-aware script/component
//! analysis. Unit tests establish operand, hook and scope semantics; this test
//! checks rostered endpoints, dynamic creator identities, explicit cached-entry
//! conditions, hook clears and the unresolved ledger over the provisioned packs.
//! Engagement floors detect regressions without pretending a missing static edge
//! proves absent runtime behavior.
//!
//! Pack-dependent: fails loudly without server/data/pack; `--features no-pack` reports it ignored.

mod common;

use native910::symbols::SymbolRegistry;
use native910::xref::{build_xref_with_summaries, format_summary, load_corpus};
use std::path::PathBuf;

fn pack_root() -> PathBuf {
    common::pack_root()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_xref_links_scripts_and_components() {
    let root = pack_root();
    common::require_present(&root.join("client.scripts.js5"));
    common::require_present(&root.join("client.interfaces.js5"));
    let (scripts, components) = load_corpus(&root).expect("load corpus packs");
    assert_eq!(scripts.len(), 14_313);
    assert_eq!(components.len(), 99_894);
    let configs = native910::config::ConfigTypes::load(&root).unwrap();
    let summaries = native910::dataflow::infer_summaries(&scripts, &configs);
    let xref = build_xref_with_summaries(&scripts, &components, &configs, &summaries);
    let stats = &xref.stats;
    let creations: usize = xref.creations.values().map(Vec::len).sum();
    eprintln!(
        "xref corpus: {} sites, {} linked, {} bailed, {} dangling; hooks {} seen / {} resolved / {} unresolved",
        stats.sites,
        stats.linked,
        stats.bails,
        stats.dangling,
        stats.hook_heads,
        stats.hook_resolved,
        stats.hook_unresolved
    );
    eprintln!(
        "xref scope: {} cc sites, {} linked, {} dynamic, {} bails; {} creations; {} arg scripts; {} finds ({} pack)",
        stats.cc_sites,
        stats.cc_linked,
        stats.cc_dynamic,
        stats.cc_bails,
        creations,
        stats.arg_scripts,
        stats.finds,
        stats.finds_pack
    );
    eprintln!(
        "xref setters: {} sites, {} installed, {} dangling, {} bailed",
        stats.set_sites, stats.set_linked, stats.set_dangling, stats.set_bails
    );
    // Engagement pins below the config-aware measured shape: 34.6k packed
    // links, 46.8k cached hooks, 609 active packed links, 408 packed finds,
    // 900 child creations and 1,930 conditional cached-entry scripts.
    assert!(
        stats.linked >= 30_000,
        "script->component linking far below the measured 34.6k: {}",
        stats.linked
    );
    assert!(
        stats.hook_resolved >= 40_000,
        "hook-head resolution far below the measured 46.8k: {}",
        stats.hook_resolved
    );
    assert!(
        stats.cc_linked >= 500,
        "cc_ scope linking far below the measured 609: {}",
        stats.cc_linked
    );
    assert!(
        creations >= 500,
        "creation edges far below the measured 900: {creations}"
    );
    assert!(
        stats.arg_scripts >= 1_500,
        "arg resolution far below the measured 1,930 scripts: {}",
        stats.arg_scripts
    );
    assert!(
        stats.finds_pack >= 300,
        "pack finds far below the measured 408: {}",
        stats.finds_pack
    );
    // The shared descriptor analysis recovers 2,243 explicit and 496 active
    // hook installations, retaining 10,219 dynamic component touches. These
    // floors protect those newly inspectable relationships against regression.
    const MIN_EXPLICIT_INSTALLATIONS: usize = 2_200;
    const MIN_ACTIVE_INSTALLATIONS: usize = 450;
    const MIN_DYNAMIC_TOUCHES: usize = 10_000;
    assert!(stats.active_hook_linked >= MIN_ACTIVE_INSTALLATIONS);
    assert!(stats.cc_dynamic >= MIN_DYNAMIC_TOUCHES);
    assert!(
        stats.set_linked >= MIN_EXPLICIT_INSTALLATIONS,
        "hook-setter installs far below the measured 2,243: {}",
        stats.set_linked
    );
    assert!(
        stats.set_dangling >= 700,
        "dangling installs far below the measured 894: {}",
        stats.set_dangling
    );
    assert!(
        stats.set_sites >= 4_500,
        "setter sites far below the measured 5,032: {}",
        stats.set_sites
    );
    // Every dangling install is a `-1` hook clear: a genuine dead script id
    // (or a depth-miscount bug) fails loudly here, same as component
    // dangling above.
    assert!(
        xref.hook_dangling
            .iter()
            .all(|dangling| dangling.value == -1),
        "non-clear dangling installs: {:?}",
        xref.hook_dangling
            .iter()
            .find(|dangling| dangling.value != -1)
    );
    // Zero dangling over this corpus: every well-formed literal names a real
    // pair. Any count above zero is either a dead reference worth knowing or
    // a classification bug — both deserve a human look, so fail loudly.
    assert!(
        xref.dangling.is_empty(),
        "{} dangling reference(s), first: {:?}",
        xref.dangling.len(),
        xref.dangling.first()
    );
    // Every recorded edge names a rostered pair on both ends (true by
    // construction; asserted over the full index as the invariant).
    for uses in xref.script_to_comps.values() {
        for touch in uses {
            assert!(
                components.contains_key(&(
                    touch.target.iface,
                    u32::try_from(touch.target.child).unwrap_or(u32::MAX)
                )),
                "edge to absent component: script {} {:?}",
                touch.script,
                touch.target
            );
            assert!(
                scripts.contains_key(&touch.script),
                "edge from absent script: {}",
                touch.script
            );
        }
    }
    for uses in xref.comp_to_scripts.values() {
        for hook in uses {
            assert!(
                components.contains_key(&(hook.iface, hook.child)),
                "hook on absent component: {hook:?}"
            );
            assert!(
                scripts.contains_key(&hook.script),
                "hook to absent script: {hook:?}"
            );
        }
    }
    // Every recorded hook installation names a rostered triple on both ends
    // (true by construction; asserted over the full index as the invariant).
    for sets in xref.hook_sets.values() {
        for set in sets {
            assert!(
                components.contains_key(&(
                    set.target.iface,
                    u32::try_from(set.target.child).unwrap_or(u32::MAX)
                )),
                "hook install on absent component: {set:?}"
            );
            assert!(
                scripts.contains_key(&set.caller),
                "hook install from absent script: {set:?}"
            );
            assert!(
                scripts.contains_key(&set.callee),
                "hook install to absent script: {set:?}"
            );
        }
    }
    for touch in xref.dynamic_touches.values().flatten() {
        let creator = &scripts[&touch.script].code[usize::try_from(touch.creator_pc).unwrap()];
        assert_eq!(creator.command, "cc_create");
    }
    for hook in xref.dynamic_hook_sets.values().flatten() {
        let creator = &scripts[&hook.caller].code[usize::try_from(hook.creator_pc).unwrap()];
        assert_eq!(creator.command, "cc_create");
        assert!(scripts.contains_key(&hook.callee));
    }
    assert_eq!(xref.entry_contexts.len(), stats.arg_scripts);
    assert_eq!(
        stats.active_hook_sites,
        stats.active_hook_linked + stats.active_hook_dangling + stats.active_hook_bails
    );
    assert_eq!(
        xref.unresolved.len(),
        stats.bails + stats.cc_bails + stats.set_bails + stats.active_hook_bails
    );
    let output = common::native_dir().join(format!("target/evidence-test-{}", std::process::id()));
    native910::evidence::export_with_summaries(
        &root,
        &output,
        &scripts,
        &components,
        &xref,
        &configs,
        &summaries,
    )
    .unwrap();
    let edges = std::fs::read_to_string(output.join("edges.tsv")).unwrap();
    assert!(edges.contains("dynamic-hook-install"));
    assert!(edges.contains("dynamic-touch"));
    assert!(
        std::fs::read_to_string(output.join("inputs.txt"))
            .unwrap()
            .contains("scripts-sha256")
    );
    assert_eq!(
        std::fs::read_to_string(output.join("unresolved.tsv"))
            .unwrap()
            .lines()
            .count(),
        xref.unresolved.len() + usize::from(true)
    );
    // The summary renders (smoke: non-empty, mentions all three directions).
    let summary = format_summary(
        &xref,
        &SymbolRegistry::empty(),
        &native910::inames::InterfaceRegistry::empty(),
    );
    assert!(summary.contains("scripts -> components"));
    assert!(summary.contains("components -> scripts"));
    assert!(summary.contains("scripts -> scripts"));
}
