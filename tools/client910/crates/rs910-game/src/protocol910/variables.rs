//! Sparse variable values: set and clear. A fine coordinate is the supported
//! serializable object; a component hook is not serializable and is rejected.
use super::{Error, Packet, Result};
pub use crate::entities910::variables::Value;
use crate::entities910::Player;
use std::collections::BTreeMap;
/// Validated variable base type serial id, keyed by var ID.
pub type Config = BTreeMap<i32, u8>;
pub(super) fn read(
    p: &mut Packet,
    e: &mut Player,
    c: Option<&Config>,
    clear: bool,
    npc: bool,
) -> Result<()> {
    let c = c.ok_or(Error::UnsupportedContext("variable definitions"))?;
    if clear {
        e.variables.clear();
    }
    p.g2()?;
    let count = p.byte()?;
    for _ in 0..count {
        let kind = if npc && clear {
            p.byte()?.wrapping_neg() & 255
        } else {
            128u32.wrapping_sub(p.byte()?) & 255
        };
        let id = p.g2()?;
        let v = match kind {
            0 => Value::Int(((p.g2()? as u32) << 16 | p.g2()? as u32) as i32),
            1 => {
                let mut n = 0u64;
                for _ in 0..8 {
                    n = (n << 8) | p.byte()? as u64;
                }
                Value::Long(n as i64)
            }
            2 => {
                if p.byte()? != 0 {
                    return Err(Error::Invalid("gjstr2 prefix"));
                }
                Value::String(p.string()?)
            }
            3 => {
                let level = p.byte()? as i32;
                let mut v = [level, 0, 0, 0];
                for x in &mut v[1..] {
                    *x = ((p.g2()? as u32) << 16 | p.g2()? as u32) as i32;
                }
                Value::FineCoord(v)
            }
            _ => return Err(Error::Invalid("unsupported variable base type")),
        };
        if c.get(&id).copied() != Some(kind as u8) {
            return Err(Error::Invalid("variable definition/base type mismatch"));
        }
        e.variables.insert(id, v);
    }
    Ok(())
}
