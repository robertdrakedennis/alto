//! BAS (movement animation set) types: config group 32.
use crate::{cache::Pack, protocol910::bas_types::Bas};
use anyhow::Result;
use std::collections::BTreeMap;
pub fn load(pack: &Pack) -> Result<BTreeMap<i32, Bas>> {
    let raw = pack.read_group("config", 32)?;
    let count = raw.keys().last().map_or(0, |v| v + 1);
    let mut out = BTreeMap::new();
    for id in 0..count {
        let b = match raw.get(&id) {
            Some(b) => Bas::decode(id as i32, b).map_err(|e| anyhow::anyhow!("BAS {id}: {e:?}"))?,
            None => Bas::default(),
        };
        out.insert(id as i32, b);
    }
    Ok(out)
}
