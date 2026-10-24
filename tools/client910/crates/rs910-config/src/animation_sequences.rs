//! Sequence loader: sequence ids split as `group << 7 | file`; blend groups
//! live in config group 77.
use crate::types910::pack_types as types_pack;
use crate::{
    cache::Pack,
    types910::sequence_types::{Group, Sequence},
};
use anyhow::Result;
use std::collections::BTreeMap;
pub struct Sequences {
    pub groups: BTreeMap<i32, Group>,
    pub sequences: BTreeMap<i32, Sequence>,
}
pub fn load(pack: &Pack) -> Result<Sequences> {
    decode(pack, &types_pack::Records::read(pack, "seq.config", 7)?)
}
/// [`load`] over an already read seq archive (the blend groups still come
/// from `pack`).
pub fn decode(pack: &Pack, records: &types_pack::Records) -> Result<Sequences> {
    let mut groups = BTreeMap::new();
    for (id, b) in pack.read_group("config", 77)? {
        groups.insert(
            id as i32,
            Group::decode(&b).map_err(|e| anyhow::anyhow!("sequence group {id}: {e:?}"))?,
        );
    }
    let (count, raw) = (records.count, &records.files);
    let mut sequences = BTreeMap::new();
    for id in 0..count {
        let mut s = if let Some(b) = raw.get(&id) {
            Sequence::decode(id, b).map_err(|e| anyhow::anyhow!("sequence {id}: {e:?}"))?
        } else {
            Sequence::empty(id)
        };
        s.post_decode(&groups);
        sequences.insert(id, s);
    }
    Ok(Sequences { groups, sequences })
}
