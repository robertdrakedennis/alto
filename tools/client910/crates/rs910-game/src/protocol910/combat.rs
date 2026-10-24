//! PLAYER 0x20 / NPC 0x8 hit and headbar masks. Real replacement and stable queue-order rules.
use super::{Error, Packet, Result};
use crate::entities910::combat::{Bar, Combat};
use std::collections::BTreeMap;
// Moved to rs910-config (Phase 2.6).
pub use rs910_config::types910::combat::*;
pub struct Config {
    pub slots: usize,
    pub bars: usize,
    pub updates: usize,
    pub hits: BTreeMap<i32, HitType>,
    pub types: BTreeMap<i32, BarType>,
}
pub(super) fn read(
    p: &mut Packet,
    s: &mut Combat,
    c: &Config,
    cycle: i32,
    npc: bool,
) -> Result<()> {
    if c.slots == 0 || c.slots > 128 || s.hits.len() != c.slots || s.cursor >= c.slots {
        return Err(Error::Invalid("hitmark capacity"));
    }
    let n = p.byte()?.wrapping_sub(128) & 255;
    for _ in 0..n {
        let mut id = p.smart1()?;
        let damage;
        let mut secondary = -1;
        let mut secondary_damage = -1;
        if id == 32767 {
            id = p.smart1()?;
            damage = p.smart1()?;
            secondary = p.smart1()?;
            secondary_damage = p.smart1()?;
        } else if id == 32766 {
            id = -1;
            damage = if npc {
                p.byte()?.wrapping_neg() & 255
            } else {
                p.byte()?
            } as i32;
        } else {
            damage = p.smart1()?;
        }
        let delay = p.smart1()?;
        hit(
            s,
            c,
            [id, damage, secondary, secondary_damage],
            cycle,
            delay,
        )?;
    }
    let n = if npc {
        p.byte()?.wrapping_sub(128) & 255
    } else {
        128u32.wrapping_sub(p.byte()?) & 255
    };
    for _ in 0..n {
        let id = p.smart1()?;
        let duration = p.smart1()?;
        let t = c
            .types
            .get(&id)
            .ok_or(Error::UnsupportedContext("headbar type"))?;
        if duration == 32767 {
            s.bars.retain(|b| b.id != id);
            continue;
        }
        let delay = p.smart1()?;
        let start = (128u32.wrapping_sub(p.byte()?) & 255) as i32;
        let end = if duration > 0 {
            if npc {
                p.byte()? as i32
            } else {
                (p.byte()?.wrapping_neg() & 255) as i32
            }
        } else {
            start
        };
        let values = [cycle.wrapping_add(delay), start, end, duration];
        if let Some(b) = s.bars.iter_mut().find(|b| b.id == id) {
            update(b, values, c.updates);
            continue;
        }
        let mut before = None;
        let mut remove = None;
        let mut worst = t.hide;
        for (i, b) in s.bars.iter().enumerate() {
            let old = c
                .types
                .get(&b.id)
                .ok_or(Error::UnsupportedContext("retained headbar type"))?;
            if old.show <= t.show {
                before = Some(i + 1);
            }
            if old.hide > worst {
                remove = Some(b.id);
                worst = old.hide;
            }
        }
        if remove.is_none() && s.bars.len() >= c.bars {
            continue;
        }
        let count = s.bars.len();
        let mut b = Bar {
            id,
            updates: vec![],
        };
        update(&mut b, values, c.updates);
        s.bars.insert(before.unwrap_or(0), b);
        if count >= c.bars {
            let id = remove.ok_or(Error::Invalid("headbar eviction"))?;
            s.bars.retain(|b| b.id != id);
        }
    }
    Ok(())
}
/// Add a hitmark to the actor's queue.
pub fn hit(s: &mut Combat, c: &Config, values: [i32; 4], cycle: i32, delay: i32) -> Result<()> {
    let expired = s.hits.iter().all(|h| h[4] <= cycle);
    let full = s.hits.iter().all(|h| h[4] > cycle);
    let (mode, duration) = if values[0] >= 0 {
        let t = c
            .hits
            .get(&values[0])
            .ok_or(Error::UnsupportedContext("hitmark type"))?;
        (t.replace, t.duration)
    } else {
        (-1, 0)
    };
    let slot;
    if full {
        if mode == -1 {
            return Ok(());
        }
        let mut i = 0;
        let mut value = if mode == 0 {
            s.hits[0][4]
        } else if mode == 1 {
            s.hits[0][1]
        } else {
            0
        };
        for j in 1..s.hits.len() {
            if mode == 0 && s.hits[j][4] < value {
                i = j;
                value = s.hits[j][4];
            } else if mode == 1 && s.hits[j][1] < value {
                i = j;
                value = s.hits[j][1];
            }
        }
        if mode == 1 && value >= values[1] {
            return Ok(());
        }
        slot = i;
    } else {
        if expired {
            s.cursor = 0
        }
        let mut found = None;
        for _ in 0..c.slots {
            let i = s.cursor;
            s.cursor = (i + 1) % c.slots;
            if s.hits[i][4] <= cycle {
                found = Some(i);
                break;
            }
        }
        let Some(i) = found else { return Ok(()) };
        slot = i;
    }
    s.hits[slot] = [
        values[0],
        values[1],
        values[2],
        values[3],
        cycle.wrapping_add(duration).wrapping_add(delay),
    ];
    Ok(())
}
fn update(b: &mut Bar, values: [i32; 4], max: usize) {
    if let Some(v) = b.updates.iter_mut().find(|v| v[0] == values[0]) {
        *v = values;
        return;
    }
    let at = b
        .updates
        .iter()
        .rposition(|v| v[0] <= values[0])
        .map(|i| i + 1);
    if let Some(at) = at {
        b.updates.insert(at, values);
        if b.updates.len() > max {
            b.updates.remove(0);
        }
    } else if b.updates.len() < max {
        b.updates.insert(0, values)
    }
}
/// Draw-time headbar retrieval: it removes superseded/expired updates itself.
pub fn visible_update(b: &mut Bar, cycle: i32, t: &BarType) -> Option<[i32; 4]> {
    if b.updates.first()?.first().copied()? > cycle {
        return None;
    }
    while b.updates.len() > 1 && b.updates[1][0] <= cycle {
        b.updates.remove(0);
    }
    let v = b.updates[0];
    if t.duration.wrapping_add(v[0]).wrapping_add(v[3]) > cycle {
        Some(v)
    } else {
        b.updates.remove(0);
        None
    }
}
