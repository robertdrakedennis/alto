//! Production item/NPC config provider. NPC ids split as `group << 7 | file`
//! and item ids as `group << 8 | file`. A missing file inside a known archive
//! is the type's default; a missing archive or a malformed existing record is
//! a hard load error.
use crate::{
    cache::Pack,
    types910::config_types::{resolve_items, Item, Npc},
};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
pub struct Types {
    pub items: BTreeMap<i32, Item>,
    pub npcs: BTreeMap<i32, Npc>,
    #[allow(dead_code, reason = "group capacity retained; no reader yet")]
    pub item_count: i32,
    #[allow(dead_code, reason = "group capacity retained; no reader yet")]
    pub npc_count: i32,
}
pub fn records(pack: &Pack, name: &str, bits: u32) -> Result<(i32, BTreeMap<i32, Vec<u8>>)> {
    let index = pack.read_archive_index(name)?;
    let last = *index.group_id.last().context("empty config archive")?;
    let count = index.file_count_for_group(last)?;
    let capacity = if count == 0 {
        0
    } else {
        index.file_id_for_group_index(last, count - 1)? + 1
    };
    let limit = (last << bits) + capacity;
    let mut records = BTreeMap::new();
    for group in index.group_id.iter().copied() {
        if index.file_count_for_group(group)? == 0 {
            continue;
        }
        for (file, b) in pack.read_group(name, group)? {
            anyhow::ensure!(
                file < (1 << bits),
                "{name} file {group}/{file} outside the group size"
            );
            records.insert(((group << bits) | file) as i32, b);
        }
    }
    Ok((limit as i32, records))
}
/// One config archive's files keyed by type id, read once so several
/// decoders can share the read.
pub struct Records {
    /// One past the last id the archive's last group can hold.
    pub count: i32,
    pub files: BTreeMap<i32, Vec<u8>>,
}
impl Records {
    /// [`records`] as an owned value.
    pub fn read(pack: &Pack, name: &str, bits: u32) -> Result<Self> {
        let (count, files) = records(pack, name, bits)?;
        Ok(Self { count, files })
    }
}
pub fn load(pack: &Pack, members: bool) -> Result<Types> {
    decode(
        &Records::read(pack, "obj.config", 8)?,
        &Records::read(pack, "npc.config", 7)?,
        members,
    )
}
/// The item and NPC types from already read obj and npc archives.
pub fn decode(objs: &Records, npc_records: &Records, members: bool) -> Result<Types> {
    let item_count = objs.count;
    let mut raw = BTreeMap::new();
    for (&id, b) in &objs.files {
        raw.insert(
            id,
            Item::decode(id, b).map_err(|e| anyhow::anyhow!("item {id}: {e:?}"))?,
        );
    }
    let items = resolve_items(&raw, 0..item_count, members)
        .map_err(|e| anyhow::anyhow!("item resolution: {e:?}"))?;
    let npc_count = npc_records.count;
    let bytes = &npc_records.files;
    let mut npcs = BTreeMap::new();
    for id in 0..npc_count {
        npcs.insert(
            id,
            if let Some(b) = bytes.get(&id) {
                Npc::decode(id, b).map_err(|e| anyhow::anyhow!("NPC {id}: {e:?}"))?
            } else {
                Npc::empty(id)
            },
        );
    }
    Ok(Types {
        items,
        npcs,
        item_count,
        npc_count,
    })
}

impl Types {
    /// Populate the two type tables used by real player appearance packets.
    /// Wear positions, palettes, titles and staff/session policy remain explicit.
    pub fn install_appearance_types(&self, c: &mut crate::types910::appearance::Config) {
        c.items = self
            .items
            .iter()
            .map(|(&id, t)| (id, t.appearance()))
            .collect();
        c.npc_sizes = self.npcs.iter().map(|(&id, t)| (id, t.size)).collect();
    }
    pub fn npc_types(&self) -> BTreeMap<i32, crate::types910::npc::NpcType> {
        self.npcs
            .iter()
            .map(|(&id, t)| (id, t.packet_type()))
            .collect()
    }
    pub fn npc_customisations(&self) -> crate::types910::npc_custom::Config {
        self.npcs
            .iter()
            .map(|(&id, t)| (id, t.customisation()))
            .collect()
    }
    pub fn ground_types(&self) -> BTreeMap<i32, crate::types910::zone::ObjectType> {
        self.items
            .iter()
            .map(|(&id, t)| (id, t.ground_type()))
            .collect()
    }
}
