//! Config decoders for scenery (loc), items (obj), NPCs, animation sequences
//! and inventory types.
//!
//! Each type is a plain struct with its own opcode table (see
//! [`crate::opcode_table`]). The tables cover the full opcode list of the
//! revision so no byte is misaligned: opcodes the client reads and drops are
//! consumed and dropped, and anything outside the table is an error naming
//! the id and the opcode, never a silent skip. Post-decode rules that change
//! stored fields are applied by each `decode_*` function.
//!
//! Members gating is a per-store [`AllowMembers`] policy. Decoding keeps the
//! members-independent values plus which operation slots came from the
//! members-only opcodes, and the `*_for` views and [`Obj::members_gated`]
//! apply the gate.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use crate::cache::Pack;
use crate::opcode_table::Input;

mod inv;
mod loc;
mod npc;
mod obj;
mod seq;

pub use inv::{decode_inv, Inv, InvStore};
pub use loc::{decode_loc, Loc, LocStore};
pub use npc::{decode_npc, Npc, NpcStore};
pub use obj::{decode_obj, obj_default_iops, obj_default_ops, Obj, ObjInventory, ObjStore};
pub use rs910_core::cp1252::cp1252;
pub use seq::{decode_seq, Seq, SeqStore};

/// Archive holding loc configs.
pub const LOC_ARCHIVE: &str = "loc.config";
/// Archive holding obj configs.
pub const OBJ_ARCHIVE: &str = "obj.config";
/// Archive holding npc configs.
pub const NPC_ARCHIVE: &str = "npc.config";
/// Archive holding seq configs.
pub const SEQ_ARCHIVE: &str = "seq.config";
/// Archive holding inventory types (group 5 of the shared `config` archive).
pub const INV_ARCHIVE: &str = "config";
/// Loc ids split as `group << 8 | file`: 256 files per group.
pub(crate) const LOC_GROUP_BITS: u32 = 8;
/// Obj ids split the same way.
pub(crate) const OBJ_GROUP_BITS: u32 = 8;
/// Npc ids split as `group << 7 | file`: 128 files per group.
pub(crate) const NPC_GROUP_BITS: u32 = 7;
/// Seq ids split like npc ids.
pub(crate) const SEQ_GROUP_BITS: u32 = 7;

/// Whether members-only content applies. Config lists start with `true` and
/// the login flow sets it from the world's members flag; decoding happens
/// once, so the flag lives beside the decoded entries and the members views
/// read it.
#[derive(Clone, Debug)]
pub struct AllowMembers(std::cell::Cell<bool>);

impl Default for AllowMembers {
    fn default() -> Self {
        Self(std::cell::Cell::new(true))
    }
}

impl AllowMembers {
    pub fn get(&self) -> bool {
        self.0.get()
    }

    /// Set the flag; true when the value changed, which is when dependent
    /// caches must be rebuilt.
    pub fn set(&self, allow: bool) -> bool {
        self.0.replace(allow) != allow
    }
}

/// A type parameter value: an integer or a string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamValue {
    Int(i32),
    Str(String),
}

/// A parameter table: a count, then per entry a string flag, a 24-bit key and
/// the value. Entries are appended to `params` in file order.
pub(crate) fn read_params<E>(
    source: Input<E>,
    params: &mut Vec<(i32, ParamValue)>,
) -> Result<(), E> {
    let count = source.byte()?;
    for _ in 0..count {
        let is_string = source.byte()? == 1;
        let key = source.medium()? as i32;
        let value = if is_string {
            ParamValue::Str(source.text()?)
        } else {
            ParamValue::Int(source.int()?)
        };
        params.push((key, value));
    }
    Ok(())
}

/// The morph selector shared by locs and NPCs. `read(true, id)` reads a varbit
/// and `read(false, id)` a player varp; `None` means the variable is unknown,
/// which leaves the index at `-1`. The result is the selected type id, or
/// `None` for an explicit `-1` slot or `-1` fallback.
pub fn select_multi(
    multivarbit: i32,
    multivarp: i32,
    list: &[i32],
    read: &dyn Fn(bool, i32) -> Option<i32>,
) -> Option<u32> {
    let mut value = -1;
    if multivarbit != -1 {
        if let Some(v) = read(true, multivarbit) {
            value = v;
        }
    } else if multivarp != -1 {
        if let Some(v) = read(false, multivarp) {
            value = v;
        }
    }
    let last = list.len().checked_sub(1)?;
    let id = if value >= 0 && (value as usize) < last {
        list[value as usize]
    } else {
        list[last]
    };
    u32::try_from(id).ok()
}

/// Load every group listed in `archive`'s index and decode each file with
/// `decode`, rebuilding config ids as `group << group_bits | file_id`.
fn load_all<T>(
    pack: &Pack,
    archive: &str,
    group_bits: u32,
    decode: fn(u32, &[u8]) -> Result<T>,
) -> Result<BTreeMap<u32, T>> {
    let index = pack.read_archive_index(archive)?;
    let mut entries = BTreeMap::new();
    for &group in &index.group_id {
        let files = pack
            .read_group(archive, group)
            .with_context(|| format!("{archive} group {group}"))?;
        for (file_id, bytes) in &files {
            let id = group
                .checked_shl(group_bits)
                .and_then(|base| base.checked_add(*file_id))
                .ok_or_else(|| {
                    anyhow::anyhow!("{archive}: id overflow for group {group} file {file_id}")
                })?;
            let entry = decode(id, bytes).with_context(|| format!("{archive} id {id}"))?;
            entries.insert(id, entry);
        }
    }
    Ok(entries)
}

/// [`load_all`] over an archive already read into `files` (ids
/// `group << group_bits | file_id`, as [`load_all`] builds them).
fn decode_records<T>(
    archive: &str,
    files: &BTreeMap<i32, Vec<u8>>,
    decode: fn(u32, &[u8]) -> Result<T>,
) -> Result<BTreeMap<u32, T>> {
    let mut entries = BTreeMap::new();
    for (&id, bytes) in files {
        let id = u32::try_from(id).with_context(|| format!("{archive}: negative id {id}"))?;
        let entry = decode(id, bytes).with_context(|| format!("{archive} id {id}"))?;
        entries.insert(id, entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod members_tests;
#[cfg(test)]
mod tests;
