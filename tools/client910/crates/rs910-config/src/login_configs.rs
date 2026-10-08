//! The config archives the game owner and the interface engine both decode
//! at login, read from the pack once.
//!
//! The game owner keeps its own packet-facing item, NPC and sequence types
//! and the interface engine its own stores, so the obj, npc and seq archives
//! are shared as read files and each owner decodes them. Locs and the
//! variable bindings decode to the same values on both sides, so they are
//! decoded once: the game owner reads loc footprints from the
//! [`LocStore`] before the interface engine takes it, and both hold the
//! bindings through one [`Arc`].
use std::sync::Arc;

use anyhow::Result;

use crate::cache::Pack;
use crate::config::{
    LocStore, LOC_ARCHIVE, LOC_GROUP_BITS, NPC_ARCHIVE, NPC_GROUP_BITS, OBJ_ARCHIVE,
    OBJ_GROUP_BITS, SEQ_ARCHIVE, SEQ_GROUP_BITS,
};
use crate::scenery_varbits;
use crate::types910::pack_types::Records;

/// One read of each shared config archive. A failure stays with its archive:
/// the game owner fails its login on it, the interface engine logs it and
/// leaves that store missing, as each did when it read the archive itself.
pub struct LoginConfigs {
    pub objs: Result<Records>,
    pub npcs: Result<Records>,
    pub seqs: Result<Records>,
    /// The decoded locs and the archive's id capacity.
    pub locs: Result<(LocStore, i32)>,
    pub varbits: Result<Arc<scenery_varbits::Inputs>>,
}

impl LoginConfigs {
    pub fn read(pack: &Pack) -> Self {
        Self {
            objs: Records::read(pack, OBJ_ARCHIVE, OBJ_GROUP_BITS),
            npcs: Records::read(pack, NPC_ARCHIVE, NPC_GROUP_BITS),
            seqs: Records::read(pack, SEQ_ARCHIVE, SEQ_GROUP_BITS),
            locs: Records::read(pack, LOC_ARCHIVE, LOC_GROUP_BITS)
                .and_then(|records| Ok((LocStore::from_records(&records.files)?, records.count))),
            varbits: scenery_varbits::load(pack).map(Arc::new),
        }
    }

    /// The obj archive, for an owner that fails without it.
    pub fn required_objs(&self) -> Result<&Records> {
        required(&self.objs)
    }

    /// The npc archive, for an owner that fails without it.
    pub fn required_npcs(&self) -> Result<&Records> {
        required(&self.npcs)
    }

    /// The seq archive, for an owner that fails without it.
    pub fn required_seqs(&self) -> Result<&Records> {
        required(&self.seqs)
    }

    /// The decoded locs and the loc archive's id capacity, for an owner that
    /// fails without them.
    pub fn required_locs(&self) -> Result<(&LocStore, i32)> {
        required(&self.locs).map(|(store, count)| (store, *count))
    }

    /// The variable bindings, for an owner that fails without them.
    pub fn required_varbits(&self) -> Result<Arc<scenery_varbits::Inputs>> {
        required(&self.varbits).cloned()
    }
}

fn required<T>(entry: &Result<T>) -> Result<&T> {
    entry.as_ref().map_err(|error| anyhow::anyhow!("{error:#}"))
}
