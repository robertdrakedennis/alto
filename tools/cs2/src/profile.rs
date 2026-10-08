use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROFILE_FORMAT: u32 = 1;
const MD5_HEX_DIGITS: usize = 32;
const SHA256_HEX_DIGITS: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub major: u32,
    pub minor: u32,
}

impl std::fmt::Display for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Operand layouts proven by the client's decoder and registration table.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Byte,
    Int,
    TypedConstant,
    Variable,
    Varbit16,
    Varbit24,
}

/// Reviewed normal-path traffic, separate from target runtime adaptation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalysisContract {
    Core,
    DatabaseField,
    Enum {
        string_type: u16,
    },
    Fixed {
        pops: [u16; 3],
        pushes: [u16; 3],
    },
    Hook {
        /// Recorded addressing mode, independent of the target opcode name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        explicit_component: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transmit_domain: Option<u8>,
        codec: HookCodec,
        clear_script_id: i32,
        inactive_script_ids: Vec<i32>,
    },
}

impl AnalysisContract {
    pub(crate) fn hook_component(&self, command: &str) -> Option<bool> {
        let Self::Hook {
            explicit_component, ..
        } = self
        else {
            return None;
        };
        explicit_component.or_else(|| {
            if native910::xref::hook_setter(command) {
                Some(true)
            } else {
                command.strip_prefix("cc_").and_then(|suffix| {
                    native910::xref::hook_setter(&format!("if_{suffix}")).then_some(false)
                })
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalDefaults {
    Initialized,
    Unknown,
}

impl From<LocalDefaults> for native910::dataflow::LocalDefaults {
    fn from(value: LocalDefaults) -> Self {
        match value {
            LocalDefaults::Initialized => Self::Initialized,
            LocalDefaults::Unknown => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DescriptorUnits {
    Utf16,
    Utf8Bytes,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PositiveTriggers {
    TransmitList,
    Trap,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookCodec {
    pub descriptor_units: DescriptorUnits,
    pub positive_triggers: PositiveTriggers,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retained_triggers: bool,
}

impl From<HookCodec> for native910::dataflow::HookCodec {
    fn from(value: HookCodec) -> Self {
        Self {
            retained_triggers: value.retained_triggers,
            descriptor_units: match value.descriptor_units {
                DescriptorUnits::Utf16 => native910::dataflow::DescriptorUnits::Utf16,
                DescriptorUnits::Utf8Bytes => native910::dataflow::DescriptorUnits::Utf8Bytes,
            },
            positive_triggers: match value.positive_triggers {
                PositiveTriggers::TransmitList => {
                    native910::dataflow::PositiveTriggers::TransmitList
                }
                PositiveTriggers::Trap => native910::dataflow::PositiveTriggers::Trap,
            },
        }
    }
}

/// Recovered conversion policy for raw constant bytes. Absence remains unknown.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StringEncoding {
    Windows1252DropUnassigned,
    Windows1252QuestionMark,
}

/// Duplicate-key policy recovered from the donor's switch-table construction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchLookup {
    FirstEntryWins,
    LastEntryWins,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Opcode {
    pub id: u16,
    pub encoding: Encoding,
    /// Recovered operation identity. Cross-revision behavior equivalence is a
    /// separate obligation of the importer, never implied by a matching name.
    pub command: Option<String>,
    /// Ghidra names are hints, never proof of command semantics.
    pub symbol_hint: Option<String>,
    pub handler_address: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<AnalysisContract>,
}

impl Opcode {
    pub(crate) fn is_hook(&self) -> bool {
        matches!(self.analysis, Some(AnalysisContract::Hook { .. }))
            || self.command.as_deref().is_some_and(|command| {
                native910::xref::hook_setter(command)
                    || command
                        .strip_prefix("cc_")
                        .is_some_and(|suffix| native910::xref::hook_setter(&format!("if_{suffix}")))
            })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub format: u32,
    pub build: Build,
    pub client_md5: String,
    /// SHA-256 of the compressed script archive index, including its header.
    pub script_index_sha256: String,
    pub script_archive: u32,
    #[serde(default)]
    pub string_encoding: Option<StringEncoding>,
    #[serde(default)]
    pub switch_lookup: Option<SwitchLookup>,
    /// Unknown entry storage is retained as unknown by source inspection.
    #[serde(default)]
    pub analysis_locals: Option<LocalDefaults>,
    pub opcodes: Vec<Opcode>,
}

/// Validated profile with exact opcode membership. No revision fallback.
pub struct Book {
    pub profile: Profile,
    pub sha256: String,
    rows: BTreeMap<u16, usize>,
}

impl Book {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let profile: Profile = serde_json::from_slice(bytes)?;
        ensure!(
            profile.format == PROFILE_FORMAT,
            "unsupported profile format {}",
            profile.format
        );
        ensure!(
            is_digest(&profile.client_md5, MD5_HEX_DIGITS),
            "invalid client MD5"
        );
        ensure!(
            is_digest(&profile.script_index_sha256, SHA256_HEX_DIGITS),
            "invalid script index SHA-256"
        );
        ensure!(!profile.opcodes.is_empty(), "empty opcode table");
        let mut rows = BTreeMap::new();
        for (index, opcode) in profile.opcodes.iter().enumerate() {
            ensure!(
                rows.insert(opcode.id, index).is_none(),
                "duplicate opcode {}",
                opcode.id
            );
            ensure!(
                opcode.command.as_ref().is_none_or(|name| !name.is_empty()),
                "empty command for opcode {}",
                opcode.id
            );
            if let Some(analysis) = &opcode.analysis {
                let command = opcode.command.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("analysis has no recovered command for opcode {}", opcode.id)
                })?;
                if matches!(analysis, AnalysisContract::Enum { .. }) {
                    ensure!(command == "enum", "enum analysis on {command}");
                }
                if matches!(analysis, AnalysisContract::DatabaseField) {
                    ensure!(
                        command == "db_getfield",
                        "database field analysis on {command}"
                    );
                }
                if let AnalysisContract::Hook {
                    clear_script_id,
                    inactive_script_ids,
                    ..
                } = analysis
                {
                    ensure!(
                        analysis.hook_component(command).is_some(),
                        "hook addressing mode is unreviewed for {command}"
                    );
                    ensure!(*clear_script_id < 0, "hook clear ID must be negative");
                    let mut inactive = std::collections::BTreeSet::new();
                    ensure!(
                        inactive_script_ids
                            .iter()
                            .all(|id| *id < 0 && id != clear_script_id && inactive.insert(*id)),
                        "invalid or duplicated inactive hook ID"
                    );
                }
            }
        }
        Ok(Self {
            profile,
            sha256: digest(bytes),
            rows,
        })
    }

    pub fn opcode(&self, id: u16) -> Result<&Opcode> {
        match self.rows.get(&id) {
            Some(index) => Ok(&self.profile.opcodes[*index]),
            None => bail!("opcode {id} is absent from build {}", self.profile.build),
        }
    }

    pub fn check_identity(&self, build: &str, client_md5: &str, index: &[u8]) -> Result<()> {
        ensure!(
            build == self.profile.build.to_string(),
            "profile build mismatch: requested {build}, profile {}",
            self.profile.build
        );
        ensure!(
            client_md5 == self.profile.client_md5,
            "profile client MD5 mismatch"
        );
        ensure!(
            digest(index) == self.profile.script_index_sha256,
            "script archive index does not match the profile"
        );
        Ok(())
    }
}

fn is_digest(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
