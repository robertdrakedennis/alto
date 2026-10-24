//! Ground-object zone slice.
//! Ground-object refreshes are consumed by the ordinary transient scene owner;
//! zone_state/transient implement envelopes, queued loc changes and transient actors.
//! TODO(#gap-G-zone-other): remaining sound/text/prefetch/loc-animation zone opcodes.
use super::{Context, Error, Packet, Result};
use crate::entities910::GroundObject;
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug)]
pub enum Op {
    Add,
    Delete,
    Count,
    Reveal,
}
// Moved to rs910-config (Phase 2.6).
pub use rs910_config::types910::zone::*;
pub struct ZoneContext<'a> {
    pub map: &'a Context,
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub allow_outside: bool,
    pub types: &'a BTreeMap<i32, ObjectType>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Objects {
    /// Local render-cache revision, advanced by ordinary ground-object mutations.
    pub revision: u64,
    pub stacks: BTreeMap<i64, Vec<GroundObject>>,
    pub key_order: Vec<i64>,
}
#[derive(Debug)]
pub struct DecodedObjects {
    pub state: Objects,
    #[allow(dead_code, reason = "consumed byte count retained; no reader yet")]
    pub bytes: usize,
    pub refresh: Vec<(i32, i32, i32)>,
}
/// Caller selects the decoded opcode explicitly; unsupported zone opcodes must not
/// be routed here. Returns sorted logical stacks and in-scene refresh coordinates.
pub fn decode(op: Op, bytes: &[u8], prior: &Objects, c: &ZoneContext) -> Result<DecodedObjects> {
    let mut p = Packet::new(bytes);
    let (coord, id, count, previous, owner) = match op {
        Op::Add => {
            let coord = (128u32.wrapping_sub(p.byte()?) & 255) as i32;
            let id = p.alt3()?;
            let count = le2(&mut p)?;
            (coord, id, count, 0, -1)
        }
        Op::Delete => {
            let id = p.g2()?;
            let coord = (128u32.wrapping_sub(p.byte()?) & 255) as i32;
            (coord, id, 0, 0, -1)
        }
        Op::Count => {
            let coord = p.byte()? as i32;
            let id = p.g2()?;
            let old = p.g2()?;
            let new = p.g2()?;
            (coord, id, new, old, -1)
        }
        Op::Reveal => {
            let count = p.alt3()?;
            let coord = p.byte()? as i32;
            let hi = p.byte()?;
            let lo = p.byte()?.wrapping_sub(128) & 255;
            let id = ((hi << 8) | lo) as i32;
            let owner = p.g2()?;
            (coord, id, count, 0, owner)
        }
    };
    if p.pos != bytes.len() {
        return Err(Error::Invalid("zone packet length"));
    }
    let x = c.x.wrapping_add((coord >> 4) & 7);
    let z = c.z.wrapping_add(coord & 7);
    let inside = x >= 0 && z >= 0 && x < c.map.width && z < c.map.height;
    // Cast AFTER i32 packing: the packed int is sign-extended into an i64.
    let key = (c.level.wrapping_shl(28)
        | c.map.base_z.wrapping_add(z).wrapping_shl(14)
        | c.map.base_x.wrapping_add(x)) as i64;
    let mut s = prior.clone();
    let mut refresh = vec![];
    match op {
        Op::Add | Op::Reveal => {
            if (!matches!(op, Op::Reveal) || owner != c.map.local as i32)
                && (inside || c.allow_outside)
            {
                insert(&mut s, key, GroundObject { id, count }, c)?;
                if inside {
                    refresh.push((c.level, x, z))
                }
            }
        }
        Op::Delete => {
            if let Some(stack) = s.stacks.get_mut(&key) {
                if let Some(i) = stack.iter().position(|o| o.id == id) {
                    stack.remove(i);
                }
                if stack.is_empty() {
                    s.stacks.remove(&key);
                    s.key_order.retain(|k| *k != key);
                }
                if inside {
                    refresh.push((c.level, x, z))
                }
            }
        }
        Op::Count => {
            if let Some(stack) = s.stacks.get_mut(&key) {
                if let Some(i) = stack.iter().position(|o| o.id == id && o.count == previous) {
                    let mut obj = stack.remove(i);
                    obj.count = count;
                    insert(&mut s, key, obj, c)?;
                }
                if inside {
                    refresh.push((c.level, x, z))
                }
            }
        }
    }
    if s.stacks != prior.stacks {
        s.revision = prior.revision.wrapping_add(1);
    }
    Ok(DecodedObjects {
        state: s,
        bytes: p.pos,
        refresh,
    })
}
fn insert(s: &mut Objects, key: i64, obj: GroundObject, c: &ZoneContext) -> Result<()> {
    if let Some(stack) = s.stacks.get_mut(&key) {
        let value = cost(&obj, c)?;
        let mut at = stack.len();
        for (i, other) in stack.iter().enumerate() {
            if value > cost(other, c)? {
                at = i;
                break;
            }
        }
        stack.insert(at, obj);
    } else {
        s.stacks.insert(key, vec![obj]);
        s.key_order.push(key);
    }
    Ok(())
}
fn cost(o: &GroundObject, c: &ZoneContext) -> Result<i32> {
    let t = c
        .types
        .get(&o.id)
        .ok_or(Error::UnsupportedContext("missing ground-object type"))?;
    Ok(if t.stackable == 1 {
        o.count.wrapping_add(1).wrapping_mul(t.cost)
    } else {
        t.cost
    })
}
fn le2(p: &mut Packet) -> Result<i32> {
    let a = p.byte()?;
    Ok((a | (p.byte()? << 8)) as i32)
}
