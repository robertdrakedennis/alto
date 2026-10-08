//! The 910 CS2 opcode book: canonical command name ↔ scrambled opcode id, plus
//! the large-operand width table.
//!
//! Provenance: `data/opcodes-910.txt`, `data/opcodes-large-910.txt`, and
//! `data/opcode-aliases-910.txt` are vendored copies of the
//! `generate-cs2-data` views of `cs2/registry-910.json` from the alerion cache
//! tooling — themselves extracted from the 910 client's command dispatch
//! and command table. They are embedded with `include_str!` so the
//! binary carries its own truth: no data-dir flags, no wrong-book failure mode.
//! Refresh by re-copying the three files and re-applying the local renames
//! (the seven rows the client leaves unnamed are called `retail_trap_<opcode>`,
//! `disabled_command_1144` and `getparentlayer_alias`, and
//! `can_run_classic_client` is opcode 1097); the layout parsers below
//! accept the generator's exact row shapes (`name,id[,gate]`, `id,flag`,
//! `alt,canonical`).
//!
//! Ground-truth notes against alto's own 910 client: the id→name mappings
//! match the client's dispatch switch (e.g. opcode 165 is `add`), and the
//! large-operand flags match its command table with zero mismatches. The enum's `index` fields are NOT trusted for ids — the deob
//! mislabels some (it claims `add` is 842, the dispatch proves 165).

use crate::error::{NativeError, Result};
use std::collections::BTreeMap;

const OPCODES_910: &str = include_str!("../data/opcodes-910.txt");
const OPCODES_LARGE_910: &str = include_str!("../data/opcodes-large-910.txt");
const OPCODE_ALIASES_910: &str = include_str!("../data/opcode-aliases-910.txt");

/// Revision this book describes.
pub const BOOK_BUILD: u32 = 910;

/// Canonical 910 command name ↔ opcode id, with the large-operand width table.
///
/// Duplicate-name policy: the generator emits client-truth rows before
/// synthetic port-work rows, so when one name lists two ids (today only
/// `if_getparentlayer` at real dispatch 1015 and synthetic 2016), the FIRST
/// row wins `name → id` while EVERY row wins `id → name`. Both ids decode to
/// the command; encoding uses the real dispatch slot. (A last-wins loader
/// orphans id 1015 into a synthetic fallback and cannot reassemble the 99+
/// real scripts that call it.)
#[derive(Clone, Debug)]
pub struct OpcodeBook {
    by_id: Vec<Option<String>>,
    by_name: BTreeMap<String, u16>,
    large_by_id: Vec<bool>,
    /// Encode-only aliases: a cross-build name for the same engine opcode maps
    /// to this build's canonical name. Consulted only when a direct name lookup
    /// misses, so decode output is unaffected.
    aliases: BTreeMap<String, String>,
}

impl OpcodeBook {
    /// The embedded 910 book.
    pub fn embedded() -> Result<Self> {
        let mut rows: Vec<(String, u16)> = Vec::new();
        for raw in OPCODES_910.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            let mut parts = line.split(',');
            let name = parts
                .next()
                .ok_or_else(|| invalid_row(line))?
                .trim()
                .to_string();
            let id_text = parts.next().ok_or_else(|| invalid_row(line))?.trim();
            // Optional third column: minimum build gate. Rows gated above 910
            // do not belong to this book.
            if let Some(gate) = parts.next().map(str::trim).filter(|text| !text.is_empty()) {
                let gate: u32 = gate.parse().map_err(|_| invalid_row(line))?;
                if gate > BOOK_BUILD {
                    continue;
                }
            }
            let id: u16 = id_text.parse().map_err(|_| invalid_row(line))?;
            rows.push((name, id));
        }

        // First row wins `name → id` (client truth precedes synthetic rows);
        // every row wins `id → name` so all listed ids decode.
        let mut by_name = BTreeMap::<String, u16>::new();
        for (name, id) in &rows {
            by_name.entry(name.clone()).or_insert(*id);
        }
        let max_id = rows.iter().map(|(_, id)| *id).max().unwrap_or(0);
        let mut by_id = vec![None; usize::from(max_id).saturating_add(1)];
        for (name, id) in &rows {
            if by_id[usize::from(*id)].is_some() {
                return Err(NativeError::Invalid(format!("duplicate opcode ID {id}")));
            }
            by_id[usize::from(*id)] = Some(name.clone());
        }

        let mut large_by_id = vec![false; by_id.len()];
        for raw in OPCODES_LARGE_910.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            let mut parts = line.split(',');
            let id: usize = parts
                .next()
                .ok_or_else(|| invalid_row(line))?
                .trim()
                .parse()
                .map_err(|_| invalid_row(line))?;
            let flag = parts
                .next()
                .ok_or_else(|| invalid_row(line))?
                .trim()
                .parse::<u8>()
                .map_err(|_| invalid_row(line))?;
            if id >= large_by_id.len() {
                large_by_id.resize(id + 1, false);
            }
            if flag > 1 {
                return Err(invalid_row(line));
            }
            large_by_id[id] = flag != 0;
        }

        let mut aliases = BTreeMap::<String, String>::new();
        for raw in OPCODE_ALIASES_910.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            let mut parts = line.split(',');
            let alt = parts
                .next()
                .ok_or_else(|| invalid_row(line))?
                .trim()
                .to_string();
            let canonical = parts
                .next()
                .ok_or_else(|| invalid_row(line))?
                .trim()
                .to_string();
            if !by_name.contains_key(&canonical) {
                return Err(invalid_row(line));
            }
            if let Some(existing) = by_name.get(&alt)
                && Some(existing) != by_name.get(&canonical)
            {
                return Err(invalid_row(line));
            }
            if aliases.insert(alt, canonical).is_some() {
                return Err(invalid_row(line));
            }
        }

        Ok(Self {
            by_id,
            by_name,
            large_by_id,
            aliases,
        })
    }

    /// Canonical command name for an opcode id. Unknown ids are invalid data —
    /// 910's book is complete for 910 bytes, so there is no synthetic fallback.
    pub fn name(&self, opcode: u16) -> Result<&str> {
        self.by_id
            .get(usize::from(opcode))
            .and_then(Option::as_deref)
            .ok_or_else(|| {
                NativeError::Invalid(format!("missing 910 opcode mapping for id {opcode}"))
            })
    }

    /// Opcode id for a canonical command name (or a known alias).
    pub fn opcode_for(&self, name: &str) -> Result<u16> {
        if let Some(id) = self.by_name.get(name) {
            return Ok(*id);
        }
        if let Some(canonical) = self.aliases.get(name)
            && let Some(id) = self.by_name.get(canonical)
        {
            return Ok(*id);
        }
        Err(NativeError::Invalid(format!(
            "missing 910 opcode mapping for name '{name}'"
        )))
    }

    /// Whether the opcode carries a full i32 generic operand instead of one byte.
    #[must_use]
    pub fn has_large_operand(&self, opcode: u16) -> bool {
        self.large_by_id
            .get(usize::from(opcode))
            .copied()
            .unwrap_or(false)
    }

    /// Number of canonical commands in the book.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether the book holds any commands.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Every numeric encoding, including explicitly unverified synthetic entries.
    pub fn entries(&self) -> impl Iterator<Item = (u16, &str)> {
        self.by_id
            .iter()
            .enumerate()
            .filter_map(|(id, name)| name.as_deref().map(|name| (id as u16, name)))
    }

    /// Canonical command names.
    pub fn commands(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }
}

fn invalid_row(line: &str) -> NativeError {
    NativeError::Invalid(format!("malformed opcode table row: {line:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_book_matches_client_spot_checks() {
        let book = OpcodeBook::embedded().unwrap();
        assert!(!book.is_empty());
        // Spot checks against the 910 client's known dispatch ids.
        assert_eq!(book.opcode_for("push_constant_string").unwrap(), 1376);
        assert_eq!(book.opcode_for("if_sendtofront").unwrap(), 12);
        assert_eq!(book.opcode_for("db_find_with_count").unwrap(), 593);
        assert_eq!(book.name(1144).unwrap(), "disabled_command_1144");
        assert!(book.has_large_operand(1376));
        assert!(!book.has_large_operand(842));
        // Encode-only alias resolves; decode is unaffected.
        assert_eq!(
            book.opcode_for("enum").unwrap(),
            book.opcode_for("_enum").unwrap()
        );
        assert!(book.name(book.opcode_for("_enum").unwrap()).unwrap() != "enum");
        assert!(book.name(9999).is_err());
        assert!(book.opcode_for("no_such_command").is_err());
    }

    #[test]
    fn duplicate_name_prefers_real_dispatch_slot() {
        // `if_getparentlayer` lists twice: real dispatch 1015, synthetic 2016.
        // Both ids decode; encoding uses the real slot.
        let book = OpcodeBook::embedded().unwrap();
        assert_eq!(book.opcode_for("if_getparentlayer").unwrap(), 1015);
        assert_eq!(book.name(1015).unwrap(), "if_getparentlayer");
        assert_eq!(book.name(2016).unwrap(), "if_getparentlayer");
    }
}
