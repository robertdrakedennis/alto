//! Milestone-1 corpus gate: every script in the real 910 runtime pack must
//! decode with the embedded book and re-encode byte-identical.
//!
//! Reads `server/data/pack/client.scripts.js5` (tracked out of git as a local
//! runtime artifact). Fails loudly when the pack is absent; `--features no-pack`
//! (CI) reports it ignored.

mod common;

use native910::validate::validate_scripts_file;
use std::path::PathBuf;

fn pack_path() -> PathBuf {
    common::pack_root().join("client.scripts.js5")
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn corpus_910_scripts_are_byte_exact() {
    let path = pack_path();
    common::require_present(&path);
    let report = validate_scripts_file(&path).expect("validate corpus pack");
    for failure in report.failures.iter().take(20) {
        eprintln!("FAIL {failure}");
    }
    assert!(
        report.is_clean(),
        "{} corpus failure(s) over {} scripts",
        report.failures.len(),
        report.scripts
    );
    assert!(
        report.scripts > 1000,
        "suspiciously small 910 corpus: {} scripts",
        report.scripts
    );
    eprintln!(
        "corpus: {} scripts, {} instructions, byte-exact",
        report.scripts, report.instructions
    );
}
