//! Hitmark and headbar types: config groups 46/72.
//! Capacities come from GraphicsDefaults; callers supply their real logic rate.
use crate::{
    cache::Pack,
    protocol910::{
        combat::Config,
        combat_types::{Bar, Hit},
        defaults::Graphics,
    },
};
use anyhow::Result;
use std::collections::BTreeMap;
pub struct Types {
    pub hits: BTreeMap<i32, Hit>,
    pub bars: BTreeMap<i32, Bar>,
}
pub fn load(pack: &Pack) -> Result<Types> {
    let mut hits = BTreeMap::new();
    let mut bars = BTreeMap::new();
    for (group, hit) in [(46, true), (72, false)] {
        let raw = pack.read_group("config", group)?;
        let count = raw.keys().last().map_or(0, |id| id + 1);
        for id in 0..count {
            let b = raw.get(&id);
            if hit {
                hits.insert(
                    id as i32,
                    match b {
                        Some(b) => Hit::decode(id as i32, b)
                            .map_err(|e| anyhow::anyhow!("hitmark {id}: {e:?}"))?,
                        None => Hit::default(),
                    },
                );
            } else {
                bars.insert(
                    id as i32,
                    match b {
                        Some(b) => Bar::decode(id as i32, b)
                            .map_err(|e| anyhow::anyhow!("headbar {id}: {e:?}"))?,
                        None => Bar::default(),
                    },
                );
            }
        }
    }
    Ok(Types { hits, bars })
}
impl Types {
    pub fn config(&self, g: &Graphics, logic_rate: i32) -> Config {
        let limits = g.entity_limits(logic_rate);
        Config {
            slots: limits.hitmarks,
            bars: limits.headbars,
            updates: limits.headbar_updates,
            hits: self.hits.iter().map(|(&id, t)| (id, t.combat())).collect(),
            types: self.bars.iter().map(|(&id, t)| (id, t.combat())).collect(),
        }
    }
}
