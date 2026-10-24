//! Player and basic variable type lists.
//! Player/NPC records live in config groups 60/61.
//! Bit definitions/domain binding are supplied by pack_varbits.
//! Local array state/timing uses entities910::varps, not this sparse provider.
//! Sparse records remain visible; packet maps contain only definitions with a data type.
use crate::{
    cache::Pack,
    protocol910::{
        variable_types::{Domain, Variable},
        variables::Config,
    },
};
use anyhow::Result;
use std::collections::BTreeMap;
pub struct Inputs {
    pub players: BTreeMap<i32, Variable>,
    pub npcs: BTreeMap<i32, Variable>,
}
pub fn load(pack: &Pack) -> Result<Inputs> {
    let mut maps = vec![];
    for (group, domain) in [(60, Domain::Player), (61, Domain::Npc)] {
        let raw = pack.read_group("config", group)?;
        let n = raw.keys().last().map_or(0, |id| id + 1);
        let mut map = BTreeMap::new();
        for id in 0..n {
            map.insert(
                id as i32,
                match raw.get(&id) {
                    Some(b) => Variable::decode(domain, id as i32, b).map_err(|e| {
                        anyhow::anyhow!("variable domain {} id {id}: {e:?}", domain.id())
                    })?,
                    None => Variable::empty(domain, id as i32),
                },
            );
        }
        maps.push(map);
    }
    Ok(Inputs {
        players: maps.remove(0),
        npcs: maps.remove(0),
    })
}
impl Inputs {
    pub fn definitions(&self, domain: Domain) -> &BTreeMap<i32, Variable> {
        match domain {
            Domain::Player => &self.players,
            Domain::Npc => &self.npcs,
        }
    }
    pub fn packet_config(&self, domain: Domain) -> Config {
        self.definitions(domain)
            .iter()
            .filter_map(|(&id, t)| t.base_type().map(|kind| (id, kind)))
            .collect()
    }
}
