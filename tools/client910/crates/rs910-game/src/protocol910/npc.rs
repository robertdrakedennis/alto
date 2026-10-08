//! NPC info: updating NPCs, their slots, adding NPCs and their masks.
//! All baseline NPC mask fields have CPU parsers with explicit config dependencies.
//! NPC sound requests remain an audio-owner dependency, but sound metadata
//! and `multinpc` definitions must not prevent the live NPC body/menu
//! consumers from receiving the packet. Scene/menu owners resolve the
//! selected `multinpc` definition from retained varp/varbit state.
use super::{Context, Error, Packet, Result};
use crate::entities910::Npc;
use std::collections::BTreeMap;
// Moved to rs910-config (Phase 2.6).
pub use rs910_config::types910::npc::*;
/// Random samples for a new NPC: four values per created NPC, with textures
/// selecting the fourth range.
#[derive(Clone, Copy)]
pub enum Random<'a> {
    /// Explicit samples in creation order (oracle replays supply the recorded values).
    Samples(&'a [[f64; 4]]),
    /// Drawn on demand from a speculative copy of the committed stream. The
    /// decoder asks for creation samples strictly in order, so draw `k` equals
    /// sample `k` of a pre-drawn table; the owner commits only the consumed
    /// prefix.
    Stream(&'a dyn Fn() -> [f64; 4]),
}
impl Random<'_> {
    fn sample(&self, index: usize) -> Option<[f64; 4]> {
        match self {
            Self::Samples(samples) => samples.get(index).copied(),
            // NPC storage is bounded at 1024 creations.
            Self::Stream(draw) if index < 1024 => Some(draw()),
            Self::Stream(_) => None,
        }
    }
}
/// Random samples are supplied explicitly in creation order (see [`Random`]).
pub struct NpcContext<'a> {
    pub map: &'a Context,
    pub local_x: i32,
    pub local_z: i32,
    pub view_bits: usize,
    pub loop_cycle: i32,
    pub textures: bool,
    pub combat: Option<&'a super::combat::Config>,
    pub animation: Option<&'a crate::entities910::animation_state::Config>,
    pub variables: Option<&'a super::variables::Config>,
    pub customisation: Option<&'a super::npc_custom::Config>,
    pub wear_slots: Option<usize>,
    pub chat_timeout: Option<i32>,
    pub random: Random<'a>,
    pub types: &'a BTreeMap<i32, NpcType>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Npcs {
    pub entities: BTreeMap<usize, Npc>,
    pub slots: Vec<usize>,
    pub snapshot: Vec<usize>,
    pub masks: Vec<usize>,
    pub removals: Vec<usize>,
    pub serial: i32,
    pub head_counter: i32,
    pub body_counter: i32,
}
impl Default for Npcs {
    fn default() -> Self {
        Self {
            entities: Default::default(),
            slots: vec![],
            snapshot: vec![],
            masks: vec![],
            removals: vec![],
            serial: 0,
            head_counter: 1,
            body_counter: 1,
        }
    }
}

pub fn head_icons(values: Option<&[(i32, i16)]>) -> Option<([i32; 8], [i16; 8])> {
    let values = values?;
    let mut groups = [-1; 8];
    let mut icons = [-1; 8];
    for (slot, &(group, icon)) in values.iter().take(8).enumerate() {
        groups[slot] = group;
        icons[slot] = icon;
    }
    Some((groups, icons))
}

/// An accepted in-place NPC_INFO apply (it always consumes the whole payload);
/// `undo` restores the prior state if the owner rejects the packet after
/// decoding.
#[derive(Debug)]
pub struct AppliedNpcs {
    pub bit_pos: usize,
    pub random_used: usize,
    pub undo: Undo,
}
/// Prior values an in-place NPC_INFO apply overwrote, so a rejected packet
/// restores `Npcs` exactly without copying every NPC up front: the per-packet
/// lists and counters, the `update_serial` of NPCs only re-slotted, and a full
/// copy of each NPC the packet otherwise changes, adds or removes (first touch).
#[derive(Debug)]
pub struct Undo {
    serial: i32,
    head_counter: i32,
    body_counter: i32,
    slots: Vec<usize>,
    masks: Vec<usize>,
    removals: Vec<usize>,
    snapshot_len: usize,
    snapshot: Option<Vec<usize>>,
    serials: Vec<(usize, i32)>,
    /// `None` marks an NPC this packet created.
    entities: BTreeMap<usize, Option<Npc>>,
}
impl Undo {
    fn begin(s: &mut Npcs) -> Self {
        Self {
            serial: s.serial,
            head_counter: s.head_counter,
            body_counter: s.body_counter,
            slots: std::mem::take(&mut s.slots),
            masks: std::mem::take(&mut s.masks),
            removals: std::mem::take(&mut s.removals),
            snapshot_len: s.snapshot.len(),
            snapshot: None,
            serials: vec![],
            entities: BTreeMap::new(),
        }
    }
    fn save(&mut self, id: usize, npc: Option<&Npc>) {
        self.entities.entry(id).or_insert_with(|| npc.cloned());
    }
    fn removed(&mut self, id: usize, npc: Npc) {
        self.entities.entry(id).or_insert(Some(npc));
    }
    pub fn rollback(self, s: &mut Npcs) {
        for (id, npc) in self.entities {
            match npc {
                Some(npc) => {
                    s.entities.insert(id, npc);
                }
                None => {
                    s.entities.remove(&id);
                }
            }
        }
        // Reverse order: a re-slotted id's first record holds its prior serial.
        for (id, serial) in self.serials.into_iter().rev() {
            if let Some(npc) = s.entities.get_mut(&id) {
                npc.update_serial = serial;
            }
        }
        s.serial = self.serial;
        s.head_counter = self.head_counter;
        s.body_counter = self.body_counter;
        s.slots = self.slots;
        s.masks = self.masks;
        s.removals = self.removals;
        match self.snapshot {
            Some(snapshot) => s.snapshot = snapshot,
            None => s.snapshot.truncate(self.snapshot_len),
        }
    }
}
/// The NPC info packet applied in place. On error `s` is exactly the prior
/// state (the undo log is replayed before returning).
pub fn apply(bytes: &[u8], s: &mut Npcs, c: &NpcContext) -> Result<AppliedNpcs> {
    if !(1..=15).contains(&c.view_bits) {
        return Err(Error::Invalid("npc view bits"));
    }
    // The NPC arrays have fixed capacities; fail atomically rather than
    // accepting a stream that would overflow them.
    if s.entities.len() > 1024 || s.slots.len() > 1024 || s.snapshot.len() > 1024 {
        return Err(Error::Invalid("npc state capacity"));
    }
    let mut undo = Undo::begin(s);
    match apply_logged(bytes, s, c, &mut undo) {
        Ok((bit_pos, random_used)) => Ok(AppliedNpcs {
            bit_pos,
            random_used,
            undo,
        }),
        Err(error) => {
            undo.rollback(s);
            Err(error)
        }
    }
}
fn apply_logged(
    bytes: &[u8],
    s: &mut Npcs,
    c: &NpcContext,
    undo: &mut Undo,
) -> Result<(usize, usize)> {
    // `Undo::begin` took the prior slots and emptied masks/removals.
    s.serial = s.serial.wrapping_add(1);
    let mut p = Packet::new(bytes);
    let count = p.bits(8)? as usize;
    if count > undo.slots.len() {
        return Err(Error::Invalid("npc slot count grew"));
    }
    s.removals.extend_from_slice(&undo.slots[count..]);
    for k in 0..count {
        let id = undo.slots[k];
        let e = s
            .entities
            .get_mut(&id)
            .ok_or(Error::Invalid("missing npc"))?;
        let kind = if p.bits(1)? == 0 { -1 } else { p.bits(2)? };
        if kind == 3 {
            s.removals.push(id);
            continue;
        }
        s.slots.push(id);
        undo.serials.push((id, e.update_serial));
        e.update_serial = s.serial;
        if kind > 0 {
            undo.save(id, Some(&*e));
        }
        match kind {
            -1 => {}
            0 => s.masks.push(id),
            1 => {
                e.step(p.bits(3)? as usize, 1);
                if p.bits(1)? != 0 {
                    s.masks.push(id)
                }
            }
            2 => {
                if p.bits(1)? != 0 {
                    e.step(p.bits(3)? as usize, 2);
                    e.step(p.bits(3)? as usize, 2)
                } else {
                    e.step(p.bits(3)? as usize, 0)
                }
                if p.bits(1)? != 0 {
                    s.masks.push(id)
                }
            }
            _ => unreachable!(),
        }
    }
    let mut random_used = 0;
    while bytes.len() * 8 - p.bit >= 15 {
        let id = p.bits(15)? as usize;
        if id == 32767 {
            break;
        }
        let fresh = !s.entities.contains_key(&id);
        if s.slots.len() >= 1024 || (fresh && s.snapshot.len() >= 1024) {
            return Err(Error::Invalid("npc creation capacity"));
        }
        if fresh {
            let r = c
                .random
                .sample(random_used)
                .ok_or(Error::UnsupportedContext("missing npc random samples"))?;
            if r.iter().any(|x| !x.is_finite() || *x < 0. || *x >= 1.) {
                return Err(Error::Invalid("random sample"));
            }
            random_used += 1;
            undo.save(id, None);
            s.entities.insert(
                id,
                Npc::new([
                    (r[0] * 4.) as i32 + 32,
                    (r[1] * 2.) as i32 + 3,
                    (r[2] * 3.) as i32 + 16,
                    (r[3] * if c.textures { 6. } else { 12. }) as i32,
                ]),
            );
            s.snapshot.push(id);
        } else {
            undo.save(id, s.entities.get(&id));
        }
        s.slots.push(id);
        let e = s.entities.get_mut(&id).unwrap();
        e.update_serial = s.serial;
        let level = p.bits(2)?;
        let dz = signed(p.bits(c.view_bits)?, c.view_bits);
        if p.bits(1)? != 0 {
            s.masks.push(id)
        }
        let type_id = p.bits(15)?;
        let t = c
            .types
            .get(&type_id)
            .ok_or(Error::UnsupportedContext("missing NPC type"))?;
        e.head_icons = head_icons(t.head_icons.as_deref());
        e.covermarker = t.covermarker;
        e.type_id = type_id;
        e.name = t.name.clone();
        e.vislevel = t.vislevel;
        let tele = p.bits(1)? != 0;
        let angle = ((p.bits(3)? + 4) << 11) & 16383;
        let dx = signed(p.bits(c.view_bits)?, c.view_bits);
        e.path.size = t.size;
        e.turn_speed = t.turnspeed.wrapping_shl(3);
        if fresh {
            e.path.angle = angle;
            e.path.desired_angle = angle;
        }
        let x = c.local_x.wrapping_add(dx);
        let z = c.local_z.wrapping_add(dz);
        e.path.animation.cancel_for_move();
        e.path.level = level;
        e.path.occlude_level = level + c.map.bridge(x, z);
        if !tele
            && (-8..=8).contains(&x.wrapping_sub(e.path.x[0]))
            && (-8..=8).contains(&z.wrapping_sub(e.path.z[0]))
        {
            e.enqueue(x, z, 1)
        } else {
            e.path.tele(x, z)
        }
        if fresh {
            e.fade_alpha = 255;
            e.fade_start = c.loop_cycle;
        }
    }
    p.access_bytes();
    if s.masks.len() > 251 || s.removals.len() > 1000 {
        return Err(Error::Invalid("npc mask/removal capacity"));
    }
    for &id in &s.masks {
        p.g2()?;
        let mut m = p.byte()?;
        if m & 2 != 0 {
            m += p.byte()? << 8
        }
        if m & 0x200 != 0 {
            m += p.byte()? << 16
        }
        if m & 0x100000 != 0 {
            m += p.byte()? << 24
        }
        if m & !(2
            | 0x200
            | 0x100000
            | 0x1000000
            | 1
            | 0x800000
            | 0x20
            | 0x800
            | 0x20000000
            | 0x10
            | 0x10000
            | 0x8000
            | 0x10000000
            | 0x20000
            | 8
            | 0x4000000
            | 0x2000
            | 0x1000
            | 0x2000000
            | 0x40
            | 0x80
            | 0x8000000
            | 0x200000
            | 0x400000
            | 0x400
            | 0x80000
            | 0x40000
            | 0x4000
            | 4)
            != 0
        {
            return Err(Error::UnsupportedMask {
                entity_index: id,
                mask: m,
            });
        }
        let e = s.entities.get_mut(&id).unwrap();
        undo.save(id, Some(&*e));
        if m & 0x4000000 != 0 {
            super::animation::spot(&mut p, &mut e.path, c.animation, 3, 1, 3, 0)?;
        }
        if m & 0x2000 != 0 {
            super::animation::spot(&mut p, &mut e.path, c.animation, 1, 1, 3, 3)?;
        }
        if m & 0x1000 != 0 {
            super::animation::overlays(&mut p, &mut e.path, c.animation, true)?;
        }
        if m & 0x4000 != 0 {
            super::masks::wear(
                &mut p,
                &mut e.path,
                c.wear_slots
                    .ok_or(Error::UnsupportedContext("NPC wear defaults"))?,
                true,
            )?;
        }
        if m & 0x1000000 != 0 {
            e.op_mask = (p.byte()?.wrapping_sub(128) & 255) as i32
        }
        if m & 1 != 0 {
            e.face_x = le2(&mut p)?;
            e.face_z = le2(&mut p)?
        }
        if m & 0x80000 != 0 {
            let spec = c
                .customisation
                .and_then(|c| c.get(&e.type_id))
                .ok_or(Error::UnsupportedContext("NPC customisation config"))?;
            super::npc_custom::read(&mut p, spec, &mut s.head_counter, true)?;
        }
        if m & 0x2000000 != 0 {
            super::animation::spot(&mut p, &mut e.path, c.animation, 4, 0, 3, 2)?;
        }
        if m & 0x40 != 0 {
            super::animation::modes(&mut p, &mut e.path, c.animation, true)?;
        }
        if m & 0x800000 != 0 {
            let v = p.alt3()?;
            e.vislevel = if v == 65535 {
                c.types
                    .get(&e.type_id)
                    .ok_or(Error::UnsupportedContext("missing NPC type"))?
                    .vislevel
            } else {
                v
            }
        }
        if m & 0x10 != 0 {
            let id = p.smart2()?;
            let t = c
                .types
                .get(&id)
                .ok_or(Error::UnsupportedContext("NPC type change config"))?;
            e.type_id = id;
            // An NPC type change restarts the particle system.
            e.path.actor.particle_resets = e.path.actor.particle_resets.wrapping_add(1);
            if m & 0x10000 == 0 {
                e.name = t.name.clone();
            }
            if m & 0x800000 == 0 {
                e.vislevel = t.vislevel;
            }
            e.path.size = t.size;
            e.turn_speed = t.turnspeed.wrapping_shl(3);
            e.head_icons = head_icons(t.head_icons.as_deref());
            e.covermarker = t.covermarker;
        }
        if m & 0x8000 != 0 {
            super::masks::force(&mut p, &mut e.path, c.loop_cycle, true)?;
        }
        if m & 0x10000000 != 0 {
            super::masks::tint(&mut p, &mut e.path, c.loop_cycle, true)?;
        }
        if m & 0x20 != 0 {
            let v = p.alt3()?;
            e.path.target = if v == 65535 { -1 } else { v }
        }
        if m & 0x40000 != 0 {
            let n = p.byte()?;
            for _ in 0..n {
                let stat = p.byte()? as usize;
                let b = [p.byte()?, p.byte()?, p.byte()?, p.byte()?];
                let value = ((b[2] << 24) | (b[3] << 16) | (b[0] << 8) | b[1]) as i32;
                let b = [p.byte()?, p.byte()?, p.byte()?];
                let max = ((b[1] << 16) | (b[0] << 8) | b[2]) as i32;
                *e.stats
                    .get_mut(stat)
                    .ok_or(Error::Invalid("NPC stat index"))? = value;
                e.stat_max[stat] = max;
            }
        }
        if m & 8 != 0 {
            let cfg = c
                .combat
                .ok_or(Error::UnsupportedContext("NPC combat config"))?;
            let state = e
                .path
                .combat
                .get_or_insert_with(|| crate::entities910::combat::Combat::new(cfg.slots));
            super::combat::read(&mut p, state, cfg, c.loop_cycle, true)?;
        }
        if m & 0x200000 != 0 {
            super::variables::read(&mut p, &mut e.path, c.variables, true, true)?;
        }
        if m & 0x20000 != 0 {
            let bits = p.byte()?.wrapping_sub(128) & 255;
            let mut ids = [-1; 8];
            let mut groups = [-1; 8];
            for i in 0..8 {
                if bits & (1 << i) != 0 {
                    ids[i] = p.smart2()?;
                    groups[i] = (p.smart1()? - 1) as i16;
                }
            }
            e.head_icons = Some((ids, groups));
        }
        if m & 0x800 != 0 {
            let v = p.alt3()?;
            e.bas_override = if v == 65535 { -1 } else { v }
        }
        if m & 4 != 0 {
            super::chat::read(&mut p, &mut e.path, c.chat_timeout, false, false, true)?;
        }
        if m & 0x80 != 0 {
            super::animation::spot(&mut p, &mut e.path, c.animation, 0, 0, 3, 1)?;
        }
        if m & 0x400 != 0 {
            let spec = c
                .customisation
                .and_then(|c| c.get(&e.type_id))
                .ok_or(Error::UnsupportedContext("NPC customisation config"))?;
            e.body = super::npc_custom::read(&mut p, spec, &mut s.body_counter, false)?;
        }
        if m & 0x10000 != 0 {
            let name = p.string()?;
            e.name = if name.is_empty() {
                c.types
                    .get(&e.type_id)
                    .ok_or(Error::UnsupportedContext("NPC name config"))?
                    .name
                    .clone()
            } else {
                name
            };
        }
        if m & 0x20000000 != 0 {
            e.flag = p.byte()? == 1
        }
        if m & 0x8000000 != 0 {
            super::animation::spot(&mut p, &mut e.path, c.animation, 2, 0, 3, 0)?;
        }
        if m & 0x400000 != 0 {
            super::variables::read(&mut p, &mut e.path, c.variables, false, true)?;
        }
    }
    let mut removed = false;
    for &id in &s.removals {
        if s.entities
            .get(&id)
            .ok_or(Error::Invalid("missing removal npc"))?
            .update_serial
            != s.serial
        {
            if let Some(npc) = s.entities.remove(&id) {
                undo.removed(id, npc);
            }
            removed = true
        }
    }
    // Hash-table (64 buckets) iteration: bucket order then insertion order within bucket.
    if removed {
        undo.snapshot = Some(s.snapshot[..undo.snapshot_len].to_vec());
        s.snapshot.retain(|id| s.entities.contains_key(id));
        s.snapshot.sort_by_key(|id| id & 63);
    }
    if p.pos != bytes.len() {
        return Err(Error::Invalid("npc packet length"));
    }
    if s.entities.len() != s.slots.len()
        || s.snapshot
            .iter()
            .any(|id| s.entities[id].update_serial != s.serial)
    {
        return Err(Error::Invalid("npc final slots"));
    }
    Ok((p.bit, random_used))
}
fn signed(v: i32, n: usize) -> i32 {
    if v > (1 << (n - 1)) - 1 {
        v - (1 << n)
    } else {
        v
    }
}
fn le2(p: &mut Packet) -> Result<i32> {
    let a = p.byte()?;
    Ok((a | (p.byte()? << 8)) as i32)
}
