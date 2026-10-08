//! Reproducible corpus diagnostics: cargo run --example semantic_coverage.
#[path = "../tests/common/mod.rs"]
mod common;
use native910::{
    config::ConfigTypes, opcode::OpcodeBook, pack::PackArchive, script::decode_script,
};
use std::collections::BTreeMap;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = common::pack_root();
    let configs = ConfigTypes::load(&root)?;
    let book = OpcodeBook::embedded()?;
    let archive = PackArchive::open(&root.join("client.scripts.js5"))?;
    let mut scripts = BTreeMap::new();
    for id in archive.group_ids() {
        if let Some(files) = archive.group_files(id)? {
            for bytes in files.values() {
                scripts.insert(i32::try_from(id)?, decode_script(bytes, &book)?);
            }
        }
    }
    let report = native910::coverage::analyze(&scripts, &configs);
    if let Some(directory) = std::env::args_os().nth(1) {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(directory.join("closure.md"), report.markdown())?;
        std::fs::write(directory.join("blockers.tsv"), report.tsv())?;
        std::fs::write(
            directory.join("never.txt"),
            report
                .never
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )?;
        let resources = native910::resources::infer(&scripts);
        let mut resource_text = String::from(
            "script\tarray_reads\tarray_writes\tvariable_reads\tvariable_writes\tcomponent_contexts\topaque_host_commands\tterminal_commands\tmissing_callees\tinvalid_flow\n",
        );
        for (id, resource) in resources {
            use std::fmt::Write;
            writeln!(
                resource_text,
                "{id}\t{:?}\t{:?}\t{:?}\t{:?}\t{:?}\t{}\t{:?}\t{:?}\t{}",
                resource.array_reads,
                resource.array_writes,
                resource.variable_reads,
                resource.variable_writes,
                resource.component_contexts,
                resource
                    .host_commands
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(","),
                resource.terminal_commands,
                resource.missing_callees,
                resource.invalid_flow
            )?;
        }
        std::fs::write(directory.join("resources.tsv"), resource_text)?;
        std::fs::write(
            directory.join("known.txt"),
            report
                .known
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )?;
    }
    print!("{}", report.markdown());
    Ok(())
}
