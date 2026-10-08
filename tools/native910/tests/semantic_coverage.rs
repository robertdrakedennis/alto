//! Required revision-910 coverage gate: every previously known signature stays known.
mod common;
use native910::{
    config::ConfigTypes, coverage, opcode::OpcodeBook, pack::PackArchive, script::decode_script,
};
use std::collections::BTreeMap;
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn semantic_coverage_does_not_regress() {
    let root = common::pack_root();
    let configs = ConfigTypes::load(&root).unwrap();
    let book = OpcodeBook::embedded().unwrap();
    let archive = PackArchive::open(&root.join("client.scripts.js5")).unwrap();
    let mut scripts = BTreeMap::new();
    for id in archive.group_ids() {
        let files = archive.group_files(id).unwrap().unwrap();
        assert_eq!(files.len(), 1);
        scripts.insert(id as i32, decode_script(&files[&0], &book).unwrap());
    }
    assert_eq!(scripts.len(), 14313);
    let report = coverage::analyze(&scripts, &configs);
    let missing: Vec<_> = include_str!("fixtures/semantic-known-910.txt")
        .lines()
        .map(|line| line.parse::<i32>().unwrap())
        .filter(|id| !report.known.contains(id))
        .collect();
    assert!(missing.is_empty(), "lost known signatures: {missing:?}");
    assert_eq!(
        report.known.len() + report.never.len() + report.failures.len(),
        scripts.len()
    );
    assert_eq!(report.chains.len(), report.failures.len());
    for root in report.roots.keys() {
        assert!(
            matches!(
                root.reason,
                native910::returns::FailureKind::DivergentExit
                    | native910::returns::FailureKind::IncompatibleMerge
                    | native910::returns::FailureKind::RuntimeDependent
            ),
            "unexpected semantic blocker: {root:?}"
        );
    }
    let out = common::native_dir().join("target/verification");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("semantic-closure.md"), report.markdown()).unwrap();
    std::fs::write(out.join("semantic-blockers.tsv"), report.tsv()).unwrap();
    eprintln!(
        "semantic closure: {} known, {} non-returning, {} unresolved",
        report.known.len(),
        report.never.len(),
        report.failures.len()
    );
}
