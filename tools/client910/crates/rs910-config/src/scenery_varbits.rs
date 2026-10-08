//! The client keeps seven variable domains. World, region, controller and
//! global are valid domain ids but have no client list; requests for them
//! fail exactly at binding. The other domains use the same basic variable
//! decoder as NPC variables.
use crate::{
    cache::Pack,
    types910::{
        varbits::{self, Binding, Type},
        variable_types::{Domain, Variable},
        variables::Value,
        Error,
    },
};
use anyhow::Result;
use std::collections::BTreeMap;
pub struct Inputs {
    pub definitions: BTreeMap<u8, BTreeMap<i32, Binding>>,
    pub raw: BTreeMap<u32, Vec<u8>>,
    #[allow(dead_code, reason = "group capacity retained; no reader yet")]
    pub count: usize,
}
pub fn load(pack: &Pack) -> Result<Inputs> {
    let mut definitions = BTreeMap::new();
    for (domain, group) in [
        (0, 60),
        (1, 61),
        (2, 62),
        (5, 65),
        (6, 66),
        (7, 67),
        (9, 80),
    ] {
        let mut map = BTreeMap::new();
        let raw = pack.read_group("config", group)?;
        let n = raw.keys().last().map_or(0, |id| id + 1);
        for id in 0..n {
            let parser_domain = if domain == 0 {
                Domain::Player
            } else {
                Domain::Npc
            };
            let v = match raw.get(&id) {
                Some(b) => Variable::decode(parser_domain, id as i32, b)
                    .map_err(|e| anyhow::anyhow!("domain {domain} variable {id}: {e:?}"))?,
                None => Variable::empty(parser_domain, id as i32),
            };
            map.insert(
                id as i32,
                Binding {
                    domain,
                    id: id as i32,
                    data_type: v.data_type,
                    lifetime: v.lifetime,
                    legacy: v.legacy,
                    client_code: v.client_code,
                },
            );
        }
        definitions.insert(domain, map);
    }
    let raw = pack.read_group("config", 69)?;
    let count = raw.keys().last().map_or(0, |id| id + 1) as usize;
    Ok(Inputs {
        definitions,
        raw,
        count,
    })
}
impl Inputs {
    pub fn binding(&self, domain: u8, id: i32) -> std::result::Result<Option<Binding>, Error> {
        Ok(self.definitions.get(&domain).map(|map| {
            map.get(&id)
                .cloned()
                .unwrap_or_else(|| Binding::empty(domain, id))
        }))
    }
    pub fn get(&self, id: i32, allow_unbound: bool) -> std::result::Result<Type, varbits::Failure> {
        match u32::try_from(id).ok().and_then(|id| self.raw.get(&id)) {
            None => Ok(Type::empty(id)),
            Some(b) => Type::decode(id, b, Some(&|d, id| self.binding(d, id)), allow_unbound),
        }
    }
    /// Typed sparse reads.
    /// The caller chooses the actual domain's sparse values; local varp arrays have
    /// different initialization/scheduling and must not use this sparse accessor.
    pub fn get_sparse(
        &self,
        t: &Type,
        values: &BTreeMap<i32, Value>,
    ) -> std::result::Result<i32, Error> {
        let base = t
            .binding
            .as_ref()
            .ok_or(Error::UnsupportedContext("unbound varbit"))?;
        let value = match values.get(&base.id) {
            Some(v) => v.clone(),
            None => base.default_value()?,
        };
        let Value::Int(value) = value else {
            return Err(Error::Invalid("varbit base is not an integer"));
        };
        t.get(value).map_err(|_| Error::Invalid("varbit range"))
    }
}
