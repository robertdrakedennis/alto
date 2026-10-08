//! `validate`: the corpus gates as a command.
//!
//! Opens the runtime packs, decodes every script file (with the embedded 910
//! book) and every interface component (with its group's parentlayer),
//! re-encodes each, and byte-compares. Any decode failure or byte mismatch is
//! a hard error with the group/file that produced it — reported, never
//! skipped — and the process exits nonzero.

use crate::config::{
    decode_enum, decode_param, decode_struct, decode_var, decode_varbit, encode_enum, encode_param,
    encode_struct, encode_var, encode_varbit, var_domain_from_group,
};
use crate::dbtable::{decode_dbrow, decode_dbtable, encode_dbrow, encode_dbtable};
use crate::error::{NativeError, Result};
use crate::execution::{METADATA_FILE, SCRIPT_FILE, Table};
use crate::interface::{decode_component, encode_component};
use crate::opcode::OpcodeBook;
use crate::pack::PackArchive;
use crate::script::{decode_script, encode_script};
use crate::sprite::{decode_sprite, encode_sprite};
use std::collections::BTreeMap;
use std::path::Path;

/// Outcome of validating the whole scripts pack.
#[derive(Debug)]
pub struct ValidateReport {
    /// Script files decoded.
    pub scripts: usize,
    /// Instructions decoded across all scripts.
    pub instructions: usize,
    /// `(group, file)` pairs that failed to decode or re-encode identically.
    pub failures: Vec<String>,
}

impl ValidateReport {
    /// Whether every script decoded and round-tripped byte-identical.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Validate every script in the runtime scripts pack under `pack_root`.
pub fn validate_scripts_pack(pack_root: &Path) -> Result<ValidateReport> {
    validate_scripts_file(&pack_root.join("client.scripts.js5"))
}

/// Validate every script in one scripts pack file.
pub fn validate_scripts_file(pack_path: &Path) -> Result<ValidateReport> {
    let archive = PackArchive::open(pack_path)
        .map_err(|error| NativeError::Invalid(format!("{}: {error}", pack_path.display())))?;
    let book = OpcodeBook::embedded()?;

    let mut report = ValidateReport {
        scripts: 0,
        instructions: 0,
        failures: Vec::new(),
    };
    let mut execution = Table::default();
    let mut decoded = BTreeMap::new();
    for group in archive.group_ids() {
        let files = archive
            .group_files(group)
            .map_err(|error| NativeError::Invalid(format!("group {group}: {error}")))?;
        let Some(files) = files else { continue };
        for (file, bytes) in &files {
            if *file == METADATA_FILE && files.contains_key(&SCRIPT_FILE) {
                let attempted = (|| -> Result<()> {
                    let id = i32::try_from(group).map_err(|_| {
                        NativeError::Invalid("execution script ID out of range".into())
                    })?;
                    let table = Table::from_group(id, bytes)?;
                    let script_bytes = &files[&SCRIPT_FILE];
                    table.validate(id, script_bytes, &decode_script(script_bytes, &book)?)?;
                    execution.extend(table)?;
                    Ok(())
                })();
                if let Err(error) = attempted {
                    report
                        .failures
                        .push(format!("{group}/{file}: execution: {error}"));
                }
                continue;
            }
            match decode_script(bytes, &book) {
                Ok(script) => {
                    if *file == SCRIPT_FILE
                        && !files.contains_key(&METADATA_FILE)
                        && let Err(error) =
                            crate::execution::Accounting::default().validate_script(&script)
                    {
                        report
                            .failures
                            .push(format!("{group}/{file}: execution: {error}"));
                    }
                    report.scripts += 1;
                    report.instructions += script.code.len();
                    if *file == SCRIPT_FILE && files.contains_key(&METADATA_FILE) {
                        decoded.insert(group as i32, script.clone());
                    }
                    match encode_script(&script, &book) {
                        Ok(re_encoded) if re_encoded == *bytes => {}
                        Ok(re_encoded) => report.failures.push(format!(
                            "{group}/{file}: re-encoded {} bytes, original {} bytes",
                            re_encoded.len(),
                            bytes.len()
                        )),
                        Err(error) => {
                            report
                                .failures
                                .push(format!("{group}/{file}: re-encode: {error}"));
                        }
                    }
                }
                Err(error) => {
                    report
                        .failures
                        .push(format!("{group}/{file}: decode: {error}"));
                }
            }
        }
    }
    if let Err(error) = execution.validate_links(&decoded) {
        report.failures.push(format!("execution links: {error}"));
    }
    Ok(report)
}

/// Outcome of validating the whole interfaces pack.
#[derive(Debug)]
pub struct ValidateInterfacesReport {
    /// Component files decoded.
    pub components: usize,
    /// Interface groups walked.
    pub groups: usize,
    /// `(group, file)` pairs that failed to decode or re-encode identically.
    pub failures: Vec<String>,
}

impl ValidateInterfacesReport {
    /// Whether every component decoded and round-tripped byte-identical.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Validate every component in the runtime interfaces pack under `pack_root`.
pub fn validate_interfaces_pack(pack_root: &Path) -> Result<ValidateInterfacesReport> {
    let pack_path = pack_root.join("client.interfaces.js5");
    let archive = PackArchive::open(&pack_path)
        .map_err(|error| NativeError::Invalid(format!("{}: {error}", pack_path.display())))?;

    let mut report = ValidateInterfacesReport {
        components: 0,
        groups: 0,
        failures: Vec::new(),
    };
    for group in archive.group_ids() {
        report.groups += 1;
        let parentlayer = (group << 16) as i32;
        let files = archive
            .group_files(group)
            .map_err(|error| NativeError::Invalid(format!("group {group}: {error}")))?;
        let Some(files) = files else { continue };
        for (file, bytes) in &files {
            match decode_component(bytes, parentlayer) {
                Ok(component) => {
                    report.components += 1;
                    match encode_component(&component, parentlayer) {
                        Ok(re_encoded) if re_encoded == *bytes => {}
                        Ok(re_encoded) => report.failures.push(format!(
                            "{group}/{file}: re-encoded {} bytes, original {} bytes",
                            re_encoded.len(),
                            bytes.len()
                        )),
                        Err(error) => {
                            report
                                .failures
                                .push(format!("{group}/{file}: re-encode: {error}"));
                        }
                    }
                }
                Err(error) => {
                    report
                        .failures
                        .push(format!("{group}/{file}: decode: {error}"));
                }
            }
        }
    }
    Ok(report)
}

/// Outcome of validating the config packs.
#[derive(Clone, Debug, Default)]
pub struct ValidateConfigsReport {
    /// Entries decoded per kind, in walk order.
    pub entries: Vec<(String, usize)>,
    /// Failures naming kind, group/file, and the stage that broke.
    pub failures: Vec<String>,
}

impl ValidateConfigsReport {
    /// Whether every entry decoded and round-tripped byte-identical.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }

    /// Total entries validated.
    #[must_use]
    pub fn total(&self) -> usize {
        self.entries.iter().map(|(_, count)| count).sum()
    }
}

/// Check one decoded model against its re-encoding, recording any mismatch.
fn check_roundtrip<T>(
    report: &mut ValidateConfigsReport,
    what: &str,
    bytes: &[u8],
    decoded: Result<T>,
    encode: impl FnOnce(&T) -> Result<Vec<u8>>,
) where
    T: PartialEq + std::fmt::Debug,
{
    match decoded {
        Ok(model) => match encode(&model) {
            Ok(re_encoded) if re_encoded == bytes => {}
            Ok(re_encoded) => report.failures.push(format!(
                "{what}: re-encoded {} bytes, original {}",
                re_encoded.len(),
                bytes.len()
            )),
            Err(error) => report.failures.push(format!("{what}: re-encode: {error}")),
        },
        Err(error) => report.failures.push(format!("{what}: decode: {error}")),
    }
}

/// Validate every enum/struct/param/var/varbit entry in the runtime config
/// packs under `pack_root`. Group coverage mirrors `src/config.rs`'s pack map.
pub fn validate_configs_pack(pack_root: &Path) -> Result<ValidateConfigsReport> {
    fn walk(
        report: &mut ValidateConfigsReport,
        pack_root: &Path,
        pack_file: &str,
        groups: Option<&[u32]>,
        mut check: impl FnMut(&mut ValidateConfigsReport, u32, u32, &[u8]),
    ) -> Result<usize> {
        let pack_path = pack_root.join(pack_file);
        let archive = PackArchive::open(&pack_path)
            .map_err(|error| NativeError::Invalid(format!("{}: {error}", pack_path.display())))?;
        let mut count = 0_usize;
        for group in archive.group_ids() {
            if groups.is_some_and(|wanted| !wanted.contains(&group)) {
                continue;
            }
            let files = archive
                .group_files(group)
                .map_err(|error| NativeError::Invalid(format!("group {group}: {error}")))?;
            let Some(files) = files else { continue };
            for (file, bytes) in &files {
                count += 1;
                check(report, group, *file, bytes);
            }
        }
        Ok(count)
    }

    let mut report = ValidateConfigsReport::default();

    let count = walk(
        &mut report,
        pack_root,
        "client.enum.config.js5",
        None,
        |report, group, file, bytes| {
            check_roundtrip(
                report,
                &format!("enum {group}/{file}"),
                bytes,
                decode_enum(bytes),
                encode_enum,
            );
        },
    )?;
    report.entries.push(("enum".to_string(), count));

    let count = walk(
        &mut report,
        pack_root,
        "client.struct.config.js5",
        None,
        |report, group, file, bytes| {
            check_roundtrip(
                report,
                &format!("struct {group}/{file}"),
                bytes,
                decode_struct(bytes),
                encode_struct,
            );
        },
    )?;
    report.entries.push(("struct".to_string(), count));

    let count = walk(
        &mut report,
        pack_root,
        "client.config.js5",
        Some(&[11]),
        |report, group, file, bytes| {
            check_roundtrip(
                report,
                &format!("param {group}/{file}"),
                bytes,
                decode_param(bytes),
                encode_param,
            );
        },
    )?;
    report.entries.push(("param".to_string(), count));

    // Var groups by domain, plus the varbit group. The group list mirrors
    // `var_group_id` in both directions (unknown groups stay unread).
    let var_groups = [60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 75, 80];
    let count = walk(
        &mut report,
        pack_root,
        "client.config.js5",
        Some(&var_groups),
        |report, group, file, bytes| {
            if group == 69 {
                check_roundtrip(
                    report,
                    &format!("varbit {group}/{file}"),
                    bytes,
                    decode_varbit(bytes),
                    encode_varbit,
                );
                return;
            }
            match var_domain_from_group(group) {
                Ok(domain) => check_roundtrip(
                    report,
                    &format!("var {group}/{file}"),
                    bytes,
                    decode_var(bytes, domain),
                    |model| encode_var(model, domain),
                ),
                Err(error) => report
                    .failures
                    .push(format!("var {group}/{file}: domain: {error}")),
            }
        },
    )?;
    report.entries.push(("var/varbit".to_string(), count));

    Ok(report)
}

/// Validate every dbtable schema and dbrow tuple in `client.config.js5`
/// groups 40/41. Folded into [`ValidateConfigsReport`] as `dbtable`/`dbrow`
/// entries so `validate` covers the whole configs milestone.
pub fn validate_dbtable_pack(pack_root: &Path, report: &mut ValidateConfigsReport) -> Result<()> {
    fn walk(
        report: &mut ValidateConfigsReport,
        pack_root: &Path,
        group: u32,
        check: impl Fn(&mut ValidateConfigsReport, u32, u32, &[u8]),
    ) -> Result<usize> {
        let pack_path = pack_root.join("client.config.js5");
        let archive = PackArchive::open(&pack_path)
            .map_err(|error| NativeError::Invalid(format!("{}: {error}", pack_path.display())))?;
        let files = archive
            .group_files(group)
            .map_err(|error| NativeError::Invalid(format!("group {group}: {error}")))?;
        let Some(files) = files else {
            return Ok(0);
        };
        let mut count = 0_usize;
        for (file, bytes) in &files {
            count += 1;
            check(report, group, *file, bytes);
        }
        Ok(count)
    }

    let count = walk(report, pack_root, 40, |report, group, file, bytes| {
        check_roundtrip(
            report,
            &format!("dbtable {group}/{file}"),
            bytes,
            decode_dbtable(bytes),
            encode_dbtable,
        );
    })?;
    report.entries.push(("dbtable".to_string(), count));

    let count = walk(report, pack_root, 41, |report, group, file, bytes| {
        check_roundtrip(
            report,
            &format!("dbrow {group}/{file}"),
            bytes,
            decode_dbrow(bytes),
            encode_dbrow,
        );
    })?;
    report.entries.push(("dbrow".to_string(), count));
    Ok(())
}

/// Outcome of validating the sprites pack.
#[derive(Clone, Debug, Default)]
pub struct ValidateSpritesReport {
    /// Sprite sheets decoded.
    pub sheets: usize,
    /// Failures naming group/file and the stage that broke.
    pub failures: Vec<String>,
}

impl ValidateSpritesReport {
    /// Whether every sheet decoded and round-tripped byte-identical.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Validate every sprite sheet in `client.sprites.js5` under `pack_root`.
/// The pack is large (~537MB, held transiently); the walk itself is streaming
/// over groups.
pub fn validate_sprites_pack(pack_root: &Path) -> Result<ValidateSpritesReport> {
    let pack_path = pack_root.join("client.sprites.js5");
    let archive = PackArchive::open(&pack_path)
        .map_err(|error| NativeError::Invalid(format!("{}: {error}", pack_path.display())))?;

    let mut report = ValidateSpritesReport::default();
    for group in archive.group_ids() {
        let files = archive
            .group_files(group)
            .map_err(|error| NativeError::Invalid(format!("group {group}: {error}")))?;
        let Some(files) = files else { continue };
        for (file, bytes) in &files {
            match decode_sprite(bytes) {
                Ok(sheet) => {
                    report.sheets += 1;
                    match encode_sprite(&sheet) {
                        Ok(re_encoded) if re_encoded == *bytes => {}
                        Ok(re_encoded) => report.failures.push(format!(
                            "{group}/{file}: re-encoded {} bytes, original {} bytes",
                            re_encoded.len(),
                            bytes.len()
                        )),
                        Err(error) => {
                            report
                                .failures
                                .push(format!("{group}/{file}: re-encode: {error}"));
                        }
                    }
                }
                Err(error) => {
                    report
                        .failures
                        .push(format!("{group}/{file}: decode: {error}"));
                }
            }
        }
    }
    Ok(report)
}
