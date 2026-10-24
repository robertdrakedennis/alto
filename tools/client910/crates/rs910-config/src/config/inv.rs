//! Inventory type configs: the container size and shop stock.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::INV_ARCHIVE;
use crate::cache::Pack;
use crate::opcode_table::{at, decode_record, Entry, Input, Record, Rule, Slot, Table, Unknown};

/// One inventory type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inv {
    /// Config id (the file id in the inventory group).
    pub id: u32,
    /// Number of slots.
    pub size: u16,
    /// Shop stock as `(item, count)` pairs.
    pub stock: Vec<(i32, i32)>,
}

/// Inventory opcodes of this revision.
static INV_OPCODES: Table<Inv, anyhow::Error> = Table::new(
    &[
        Entry::new(at(2), Rule::Short(|inv, _, v| inv.size = v)),
        Entry::new(at(4), Rule::Custom(read_stock)),
    ],
    Unknown::Reject,
);

fn read_stock(source: Input<anyhow::Error>, inv: &mut Inv, _: Slot) -> Result<()> {
    let count = source.byte()?;
    inv.stock.clear();
    for _ in 0..count {
        let item = i32::from(source.short()?);
        let amount = i32::from(source.short()?);
        inv.stock.push((item, amount));
    }
    Ok(())
}

/// Decode one inventory entry. An unknown opcode is an error naming the id
/// and the opcode.
pub fn decode_inv(id: u32, data: &[u8]) -> Result<Inv> {
    let record = Record {
        kind: "inv",
        id: i64::from(id),
    };
    let blank = Inv {
        id,
        size: 0,
        stock: Vec::new(),
    };
    decode_record(&INV_OPCODES, "config", record, data, blank)
}

/// Inventory-type store (one fixed group of the `config` archive; the config
/// id is the file id).
pub struct InvStore {
    entries: BTreeMap<u32, Inv>,
}

impl InvStore {
    /// The group holding the inventory types.
    pub const GROUP: u32 = 5;

    /// A store over decoded entries, so tests can inject synthetic ones.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn from_map(entries: BTreeMap<u32, Inv>) -> Self {
        Self { entries }
    }

    pub fn load(pack: &Pack) -> Result<Self> {
        let files = pack
            .read_group(INV_ARCHIVE, Self::GROUP)
            .with_context(|| format!("{} group {}", INV_ARCHIVE, Self::GROUP))?;
        let mut entries = BTreeMap::new();
        for (file_id, bytes) in &files {
            let entry = decode_inv(*file_id, bytes)
                .with_context(|| format!("{} group {} id {file_id}", INV_ARCHIVE, Self::GROUP))?;
            entries.insert(*file_id, entry);
        }
        Ok(Self { entries })
    }

    /// Look up one inventory type by id.
    pub fn get(&self, id: u32) -> Option<&Inv> {
        self.entries.get(&id)
    }

    /// Number of decoded entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
