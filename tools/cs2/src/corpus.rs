use crate::profile::{Book, Build, digest};
use crate::wire::{Operand, decode, encode};
use anyhow::{Context, Result, ensure};
use native910::js5::{ArchiveIndex, decompress, unpack_group};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const INDEX_ARCHIVE: u32 = 255;
const MAX_SAMPLES: usize = 5;
const INSTRUCTION_ADVANCE: usize = 1;
const ONE_SCRIPT: usize = 1;

#[derive(Serialize)]
pub struct ScriptRow {
    pub group: u32,
    pub file: u32,
    pub sha256: String,
    pub instructions: usize,
    pub locals: crate::wire::Counts,
    pub args: crate::wire::Counts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalization_error: Option<String>,
}

#[derive(Serialize)]
pub struct Sample {
    pub group: u32,
    pub file: u32,
    pub instruction: usize,
    pub operand: Operand,
}

#[derive(Serialize)]
pub struct Usage {
    pub command: Option<String>,
    pub symbol_hint: Option<String>,
    pub occurrences: usize,
    pub samples: Vec<Sample>,
}

#[derive(Serialize)]
pub struct Report {
    pub build: Build,
    pub client_md5: String,
    pub profile_sha256: String,
    pub script_index_sha256: String,
    pub corpus_sha256: String,
    pub instruction_count: usize,
    pub unresolved_instruction_count: usize,
    pub normalized_scripts: usize,
    pub scripts: Vec<ScriptRow>,
    pub opcodes: BTreeMap<u16, Usage>,
}

/// Validate every indexed script, retaining unresolved semantics and samples.
/// `client_md5` identifies the Ghidra program that supplied the selected book;
/// it is an explicit caller assertion, not a hash of a locally opened binary.
pub fn scan(root: &Path, book: &Book, build: &str, client_md5: &str) -> Result<Report> {
    let archive = book.profile.script_archive;
    let (index_container, index) = load_index(root, book)?;
    book.check_identity(build, client_md5, &index_container)?;
    let mut report = Report {
        build: book.profile.build,
        client_md5: book.profile.client_md5.clone(),
        profile_sha256: book.sha256.clone(),
        script_index_sha256: book.profile.script_index_sha256.clone(),
        corpus_sha256: String::new(),
        instruction_count: 0,
        unresolved_instruction_count: 0,
        normalized_scripts: 0,
        scripts: Vec::new(),
        opcodes: BTreeMap::new(),
    };
    let mut corpus = Sha256::new();
    for group in &index.group_id {
        for (file, bytes) in load_group(root, archive, &index, *group)? {
            let script = decode(&bytes, book)
                .with_context(|| format!("archive {archive} group {group} file {file}"))?;
            ensure!(
                encode(&script, book)? == bytes,
                "byte mismatch in group {group} file {file}"
            );
            corpus.update(group.to_be_bytes());
            corpus.update(file.to_be_bytes());
            corpus.update(Sha256::digest(&bytes));
            for (instruction, operation) in script.code.iter().enumerate() {
                let row = book.opcode(operation.opcode)?;
                if row.command.is_none() {
                    report.unresolved_instruction_count += 1;
                }
                let usage = report.opcodes.entry(row.id).or_insert_with(|| Usage {
                    command: row.command.clone(),
                    symbol_hint: row.symbol_hint.clone(),
                    occurrences: 0,
                    samples: Vec::new(),
                });
                usage.occurrences += 1;
                if usage.samples.len() < MAX_SAMPLES {
                    usage.samples.push(Sample {
                        group: *group,
                        file,
                        instruction,
                        operand: operation.operand.clone(),
                    });
                }
            }
            report.instruction_count += script.code.len();
            let normalization_error = match crate::semantic::normalize(&bytes, book) {
                Ok(normalized) => {
                    ensure!(
                        encode(&normalized.to_wire(), book)? == bytes,
                        "normalization lost wire facts in group {group} file {file}"
                    );
                    report.normalized_scripts += 1;
                    None
                }
                Err(error) => Some(format!("{error:#}")),
            };
            report.scripts.push(ScriptRow {
                group: *group,
                file,
                sha256: digest(&bytes),
                instructions: script.code.len(),
                locals: script.locals,
                args: script.args,
                normalization_error,
            });
        }
    }
    report.corpus_sha256 = format!("{:x}", corpus.finalize());
    Ok(report)
}

/// Every matching use in the verified archive, with original PCs and bounded
/// instruction context. Command selection names registration rows in the book;
/// operand-specific normalization remains visible in each context instruction.
#[derive(Serialize)]
pub struct UseReport {
    pub build: Build,
    pub client_md5: String,
    pub profile_sha256: String,
    pub script_index_sha256: String,
    pub corpus_sha256: String,
    pub selected_opcodes: BTreeSet<u16>,
    pub opcodes: BTreeMap<u16, crate::profile::Opcode>,
    pub context_before: usize,
    pub context_after: usize,
    pub scanned_scripts: usize,
    pub occurrences: usize,
    pub scripts: Vec<ScriptUses>,
}

#[derive(Serialize)]
pub struct ScriptUses {
    pub group: u32,
    pub file: u32,
    pub source_sha256: String,
    pub name: Vec<u8>,
    pub locals: crate::wire::Counts,
    pub args: crate::wire::Counts,
    pub instruction_count: usize,
    pub uses: Vec<usize>,
    /// Overlapping context windows are emitted once, in source order.
    pub context: Vec<LocatedInstruction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalization_error: Option<String>,
}

#[derive(Serialize)]
pub struct LocatedInstruction {
    pub pc: usize,
    #[serde(flatten)]
    pub instruction: crate::semantic::Instruction,
}

/// Inspect all uses of exact-profile wire opcodes or recovered registration
/// commands. There is no sample cap and no revision fallback. Each script is
/// decoded and byte-round-tripped, even if it has no selected use.
pub fn uses(
    root: &Path,
    book: &Book,
    opcodes: &[u16],
    commands: &[String],
    context_before: usize,
    context_after: usize,
) -> Result<UseReport> {
    let mut selected: BTreeSet<_> = opcodes.iter().copied().collect();
    for opcode in &selected {
        book.opcode(*opcode)?;
    }
    for command in commands {
        let matching: Vec<_> = book
            .profile
            .opcodes
            .iter()
            .filter(|row| row.command.as_deref() == Some(command.as_str()))
            .map(|row| row.id)
            .collect();
        ensure!(
            !matching.is_empty(),
            "profile has no recovered registration command {command}"
        );
        selected.extend(matching);
    }
    ensure!(
        !selected.is_empty(),
        "select an opcode or recovered registration command"
    );
    let (_, index) = load_index(root, book)?;
    let mut report = UseReport {
        build: book.profile.build,
        client_md5: book.profile.client_md5.clone(),
        profile_sha256: book.sha256.clone(),
        script_index_sha256: book.profile.script_index_sha256.clone(),
        corpus_sha256: String::new(),
        opcodes: selected
            .iter()
            .map(|id| Ok((*id, book.opcode(*id)?.clone())))
            .collect::<Result<_>>()?,
        selected_opcodes: selected,
        context_before,
        context_after,
        scanned_scripts: usize::default(),
        occurrences: usize::default(),
        scripts: Vec::new(),
    };
    let mut corpus = Sha256::new();
    for group in &index.group_id {
        for (file, bytes) in load_group(root, book.profile.script_archive, &index, *group)? {
            let script = decode(&bytes, book)
                .with_context(|| format!("script group {group} file {file}"))?;
            ensure!(
                encode(&script, book)? == bytes,
                "byte mismatch in group {group} file {file}"
            );
            corpus.update(group.to_be_bytes());
            corpus.update(file.to_be_bytes());
            corpus.update(Sha256::digest(&bytes));
            report.scanned_scripts += ONE_SCRIPT;
            let matched: Vec<_> = script
                .code
                .iter()
                .enumerate()
                .filter_map(|(pc, instruction)| {
                    report
                        .selected_opcodes
                        .contains(&instruction.opcode)
                        .then_some(pc)
                })
                .collect();
            if matched.is_empty() {
                continue;
            }
            report.occurrences += matched.len();
            let (normalized, normalization_error) = match crate::semantic::normalize(&bytes, book) {
                Ok(normalized) => (Some(normalized), None),
                Err(error) => (None, Some(format!("{error:#}"))),
            };
            let mut context = BTreeSet::new();
            for pc in &matched {
                let end = pc
                    .saturating_add(context_after)
                    .saturating_add(INSTRUCTION_ADVANCE)
                    .min(script.code.len());
                context.extend(pc.saturating_sub(context_before)..end);
            }
            let context = context
                .into_iter()
                .map(|pc| {
                    let wire = &script.code[pc];
                    let registration = book.opcode(wire.opcode)?;
                    report
                        .opcodes
                        .entry(wire.opcode)
                        .or_insert_with(|| registration.clone());
                    Ok(LocatedInstruction {
                        pc,
                        instruction: normalized.as_ref().map_or_else(
                            || crate::semantic::Instruction {
                                wire: wire.clone(),
                                operation: None,
                            },
                            |normalized| normalized.instructions[pc].clone(),
                        ),
                    })
                })
                .collect::<Result<_>>()?;
            report.scripts.push(ScriptUses {
                group: *group,
                file,
                source_sha256: digest(&bytes),
                name: script.name,
                locals: script.locals,
                args: script.args,
                instruction_count: script.code.len(),
                uses: matched,
                context,
                normalization_error,
            });
        }
    }
    report.corpus_sha256 = format!("{:x}", corpus.finalize());
    Ok(report)
}

/// Load the exact script index bound to this profile.
pub fn load_index(root: &Path, book: &Book) -> Result<(Vec<u8>, ArchiveIndex)> {
    let archive = book.profile.script_archive;
    let path = root
        .join(INDEX_ARCHIVE.to_string())
        .join(format!("{archive}.dat"));
    let container = std::fs::read(&path).with_context(|| path.display().to_string())?;
    ensure!(
        digest(&container) == book.profile.script_index_sha256,
        "script archive index does not match the profile"
    );
    let index = ArchiveIndex::decode(&decompress(&container)?)?;
    Ok((container, index))
}

/// Check and unpack a rostered group, preserving its sparse file IDs.
pub fn load_group(
    root: &Path,
    archive: u32,
    index: &ArchiveIndex,
    group: u32,
) -> Result<BTreeMap<u32, Vec<u8>>> {
    ensure!(
        index.group_id.binary_search(&group).is_ok(),
        "group {group} is absent from archive {archive}"
    );
    let path = root.join(archive.to_string()).join(format!("{group}.dat"));
    let container = std::fs::read(&path).with_context(|| path.display().to_string())?;
    ensure!(
        crc32fast::hash(&container) == index.group_checksums[group as usize] as u32,
        "checksum mismatch in archive {archive} group {group}"
    );
    Ok(unpack_group(index, group, &container)?)
}
