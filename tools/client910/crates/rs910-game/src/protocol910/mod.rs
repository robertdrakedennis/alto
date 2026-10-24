//! PLAYER_INFO CPU decoder: four aligned passes and signed bit packing.
//! All mask fields present in the baseline have CPU decoders; unknown bits and
//! missing explicit config inputs fail atomically, never consume-and-drop.
/// The decode error moved to `entities910` (Phase 2.2), which
/// entity state and these appliers share.
pub use crate::entities910::Error;
use crate::entities910::{LowPlayer, Player};
use rs910_config::types910::packet::*; // Moved to rs910-config (Phase 2.6).
type Result<T> = std::result::Result<T, Error>;
/// Explicit map context; entities do not depend on a Scene or graphics toolkit.
#[derive(Clone)]
pub struct Context {
    pub local: usize,
    pub base_x: i32,
    pub base_z: i32,
    pub width: i32,
    pub height: i32,
    pub bridges: Vec<(i32, i32)>,
}
impl Context {
    fn bridge(&self, x: i32, z: i32) -> i32 {
        i32::from(
            x >= 0 && z >= 0 && x < self.width && z < self.height && self.bridges.contains(&(x, z)),
        )
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Players {
    pub players: Vec<Option<Player>>,
    /// Local incarnation tokens for retained render references (never sent on wire).
    pub generations: Vec<u32>,
    pub low: Vec<Option<LowPlayer>>,
    pub nsn: Vec<i8>,
    pub speeds: Vec<i8>,
    pub high_indices: Vec<usize>,
    /// The high-resolution index array (always 2048 entries): the first
    /// `high_indices.len()` entries mirror `high_indices`; the rest keep
    /// whatever an earlier, longer list wrote (only the prefix is rewritten,
    /// and `reset` never clears it). The 2D entity element pass reads these
    /// stale entries for NPC rows.
    pub high_resolution_indices: Vec<usize>,
    pub low_indices: Vec<usize>,
    pub update_ids: Vec<usize>,
    pub current_level: i32,
    pub appearances: Vec<Option<crate::entities910::appearance::CachedPacket>>,
    pub head_icons: Vec<Option<crate::entities910::appearance::CachedPacket>>,
}
impl Players {
    /// The rebuilt list is written over the front of the retained
    /// 2048-entry array.
    fn record_high_resolution_indices(&mut self) {
        if self.high_resolution_indices.len() != 2048 {
            self.high_resolution_indices.resize(2048, 0);
        }
        for (slot, &index) in self.high_indices.iter().enumerate() {
            self.high_resolution_indices[slot] = index;
        }
    }
    /// `players[high_resolution_indices[row]]`. A row past 2048 would be out
    /// of bounds; no such row exists while the high-resolution list and NPC slots share one frame.
    pub fn high_resolution_player(&self, row: usize) -> Option<&Player> {
        let index = *self.high_resolution_indices.get(row)?;
        self.players.get(index)?.as_ref()
    }
}
impl Default for Players {
    fn default() -> Self {
        Self {
            players: vec![None; 2048],
            generations: vec![0; 2048],
            low: vec![None; 2048],
            nsn: vec![0; 2048],
            speeds: vec![1; 2048],
            high_indices: vec![],
            high_resolution_indices: vec![0; 2048],
            low_indices: vec![],
            update_ids: vec![],
            current_level: 0,
            appearances: vec![None; 2048],
            head_icons: vec![None; 2048],
        }
    }
}
#[derive(Debug)]
pub struct Decoded {
    pub state: Players,
    #[cfg_attr(not(test), allow(dead_code, reason = "read by tests only"))]
    pub bytes: usize,
    pub bit_pos: usize,
}
const STEP: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];
const RUN: [(i32, i32); 16] = [
    (-2, -2),
    (-1, -2),
    (0, -2),
    (1, -2),
    (2, -2),
    (-2, -1),
    (2, -1),
    (-2, 0),
    (2, 0),
    (-2, 1),
    (2, 1),
    (-2, 2),
    (-1, 2),
    (0, 2),
    (1, 2),
    (2, 2),
];
fn validate(c: &Context, s: &Players) -> Result<()> {
    if c.local >= 2048
        || c.width <= 0
        || c.height <= 0
        || s.players.len() != 2048
        || s.low.len() != 2048
        || s.nsn.len() != 2048
        || s.speeds.len() != 2048
        || s.appearances.len() != 2048
        || s.head_icons.len() != 2048
    {
        return Err(Error::Invalid("context/state dimensions"));
    }
    if s.high_indices
        .iter()
        .chain(&s.low_indices)
        .any(|&i| i >= 2048)
    {
        return Err(Error::Invalid("index"));
    }
    if s.players.iter().flatten().any(|p| p.route_length >= 10) {
        return Err(Error::Invalid("player queue length"));
    }
    Ok(())
}
/// Retain cached packets on re-entry.
pub fn initialize_cached(
    bytes: &[u8],
    prior: &Players,
    c: &Context,
    a: Option<&appearance::Config>,
) -> Result<Decoded> {
    let mut s = prior.clone();
    s.high_indices.clear();
    s.low_indices.clear();
    validate(c, &s)?;
    let mut p = Packet::new(bytes);
    let v = p.bits(30)?;
    let mut e = Player::default();
    e.level = v >> 28;
    e.occlude_level = e.level;
    e.tele(
        ((v >> 14) & 16383).wrapping_sub(c.base_x),
        (v & 16383).wrapping_sub(c.base_z),
    );
    e.occlude_level += c.bridge(e.x[0], e.z[0]);
    s.current_level = e.level;
    apply_cached(&mut s, &mut e, c.local, a)?;
    s.nsn[c.local] = 0;
    s.players[c.local] = Some(e);
    s.generations[c.local] = s.generations[c.local].wrapping_add(1);
    s.high_indices.push(c.local);
    s.record_high_resolution_indices();
    for i in 1..2048 {
        if i == c.local {
            continue;
        }
        let v = p.bits(18)?;
        s.low[i] = Some(LowPlayer {
            partner: 0,
            coord: ((v >> 16) << 28) + (((v >> 8) & 255) << 14) + (v & 255),
            angle: 0,
            target: -1,
            suppress_partner: false,
        });
        s.nsn[i] = 0;
        s.low_indices.push(i);
    }
    p.access_bytes();
    if p.pos != bytes.len() {
        return Err(Error::Invalid("initial packet length"));
    }
    Ok(Decoded {
        state: s,
        bytes: p.pos,
        bit_pos: p.bit,
    })
}
#[cfg(any(test, feature = "test-hooks"))] // test-only decode entry
pub fn decode_with_appearance(
    bytes: &[u8],
    prior: &Players,
    c: &Context,
    a: Option<&appearance::Config>,
) -> Result<Decoded> {
    decode_at(bytes, prior, c, a, 0)
}
#[cfg(any(test, feature = "test-hooks"))] // test-only decode entry
pub fn decode_at(
    bytes: &[u8],
    prior: &Players,
    c: &Context,
    a: Option<&appearance::Config>,
    cycle: i32,
) -> Result<Decoded> {
    let mut state = prior.clone();
    let d = apply_full(
        bytes,
        &mut state,
        &PlayerContext {
            world: c,
            appearance: a,
            combat: None,
            animation: None,
            variables: None,
            chat_timeout: None,
            cycle,
        },
    )?;
    Ok(Decoded {
        state,
        bytes: bytes.len(),
        bit_pos: d.bit_pos,
    })
}
pub struct PlayerContext<'a> {
    pub world: &'a Context,
    pub appearance: Option<&'a appearance::Config>,
    pub combat: Option<&'a combat::Config>,
    pub animation: Option<&'a crate::entities910::animation_state::Config>,
    pub variables: Option<&'a variables::Config>,
    pub chat_timeout: Option<i32>,
    pub cycle: i32,
}
/// An accepted in-place PLAYER_INFO apply (it always consumes the whole
/// payload); `undo` restores the prior state if the owner rejects the packet
/// after decoding.
pub struct AppliedPlayers {
    pub bit_pos: usize,
    pub undo: PlayersUndo,
}
/// Prior values an in-place PLAYER_INFO apply overwrote, so a rejected packet
/// restores `Players` exactly without copying all 2048 slots up front: every
/// per-slot entry of a slot the packet touched (saved on first touch), the
/// per-packet index lists and `nsn`, and the rewritten prefix of
/// `high_resolution_indices`.
pub struct PlayersUndo {
    touched: [u64; 32],
    slots: Vec<SavedSlot>,
    nsn: Vec<i8>,
    high_indices: Vec<usize>,
    low_indices: Vec<usize>,
    update_ids: Vec<usize>,
    resolution: Option<Resolution>,
    current_level: i32,
}
struct SavedSlot {
    index: usize,
    player: Option<Player>,
    low: Option<LowPlayer>,
    speed: i8,
    generation: u32,
    appearance: Option<crate::entities910::appearance::CachedPacket>,
    head_icons: Option<crate::entities910::appearance::CachedPacket>,
}
enum Resolution {
    Prefix(Vec<usize>),
    Whole(Vec<usize>),
}
impl PlayersUndo {
    /// Takes the prior index lists; the apply rebuilds them.
    fn begin(s: &mut Players) -> Self {
        Self {
            touched: [0; 32],
            slots: vec![],
            nsn: s.nsn.clone(),
            high_indices: std::mem::take(&mut s.high_indices),
            low_indices: std::mem::take(&mut s.low_indices),
            update_ids: std::mem::take(&mut s.update_ids),
            resolution: None,
            current_level: s.current_level,
        }
    }
    /// Saves slot `i` before its first change in this packet.
    fn touch(&mut self, s: &Players, i: usize) {
        let (word, bit) = (i / 64, 1u64 << (i % 64));
        if self.touched[word] & bit != 0 {
            return;
        }
        self.touched[word] |= bit;
        self.slots.push(SavedSlot {
            index: i,
            player: s.players[i].clone(),
            low: s.low[i].clone(),
            speed: s.speeds[i],
            generation: s.generations[i],
            appearance: s.appearances[i].clone(),
            head_icons: s.head_icons[i].clone(),
        });
    }
    pub fn rollback(self, s: &mut Players) {
        for slot in self.slots {
            let i = slot.index;
            s.players[i] = slot.player;
            s.low[i] = slot.low;
            s.speeds[i] = slot.speed;
            s.generations[i] = slot.generation;
            s.appearances[i] = slot.appearance;
            s.head_icons[i] = slot.head_icons;
        }
        s.nsn = self.nsn;
        s.high_indices = self.high_indices;
        s.low_indices = self.low_indices;
        s.update_ids = self.update_ids;
        s.current_level = self.current_level;
        match self.resolution {
            Some(Resolution::Prefix(prefix)) => {
                s.high_resolution_indices[..prefix.len()].copy_from_slice(&prefix)
            }
            Some(Resolution::Whole(whole)) => s.high_resolution_indices = whole,
            None => {}
        }
    }
}
/// The player info packet applied in place. On error `s` is exactly
/// the prior state (the undo log is replayed before returning).
pub fn apply_full(
    bytes: &[u8],
    s: &mut Players,
    context: &PlayerContext,
) -> Result<AppliedPlayers> {
    validate(context.world, s)?;
    let mut undo = PlayersUndo::begin(s);
    match apply_logged(bytes, s, context, &mut undo) {
        Ok(bit_pos) => Ok(AppliedPlayers { bit_pos, undo }),
        Err(error) => {
            undo.rollback(s);
            Err(error)
        }
    }
}
fn apply_logged(
    bytes: &[u8],
    s: &mut Players,
    context: &PlayerContext,
    u: &mut PlayersUndo,
) -> Result<usize> {
    let c = context.world;
    let a = context.appearance;
    let cycle = context.cycle;
    // `PlayersUndo::begin` took the prior index lists and cleared update_ids.
    let mut p = Packet::new(bytes);
    for pass in 0..4 {
        p.access_bits();
        let count = if pass < 2 {
            u.high_indices.len()
        } else {
            u.low_indices.len()
        };
        let want = pass == 1 || pass == 2;
        let mut skip = 0;
        for k in 0..count {
            let i = if pass < 2 {
                u.high_indices[k]
            } else {
                u.low_indices[k]
            };
            if (s.nsn[i] & 1 != 0) != want {
                continue;
            }
            if skip > 0 {
                skip -= 1;
                s.nsn[i] |= 2;
                continue;
            }
            if p.bits(1)? == 0 {
                let k = p.bits(2)?;
                skip = match k {
                    0 => 0,
                    1 => p.bits(5)?,
                    2 => p.bits(8)?,
                    _ => p.bits(11)?,
                };
                s.nsn[i] |= 2;
            } else if pass < 2 {
                high(&mut p, s, c, i, a, u)?;
            } else if low(&mut p, s, c, i, 0, a, u)? {
                s.nsn[i] |= 2;
            }
        }
        p.access_bytes();
        if skip != 0 {
            return Err(Error::Invalid("skip exceeds pass"));
        }
    }
    s.high_indices.reserve(u.high_indices.len());
    s.low_indices.reserve(u.low_indices.len());
    for i in 1..2048 {
        s.nsn[i] >>= 1;
        if s.players[i].is_some() {
            s.high_indices.push(i)
        } else {
            s.low_indices.push(i)
        }
    }
    let rewritten = s.high_indices.len();
    u.resolution = Some(if s.high_resolution_indices.len() == 2048 {
        Resolution::Prefix(s.high_resolution_indices[..rewritten].to_vec())
    } else {
        Resolution::Whole(s.high_resolution_indices.clone())
    });
    s.record_high_resolution_indices();
    for k in 0..s.update_ids.len() {
        let i = s.update_ids[k];
        p.g2()?;
        let mut mask = p.byte()?;
        if mask & 0x40 != 0 {
            mask += p.byte()? << 8
        }
        if mask & 0x1000 != 0 {
            mask += p.byte()? << 16
        }
        if mask
            & !(0x40
                | 0x1000
                | 0x10
                | 0x10000
                | 1
                | 4
                | 0x800
                | 8
                | 0x800000
                | 0x400
                | 0x80000
                | 0x20
                | 0x80
                | 0x8000
                | 0x100000
                | 0x2000
                | 2
                | 0x100
                | 0x400000
                | 0x40000
                | 0x20000
                | 0x200
                | 0x200000)
            != 0
        {
            return Err(Error::UnsupportedMask {
                entity_index: i,
                mask,
            });
        }
        u.touch(s, i);
        let e = s.players[i]
            .as_mut()
            .ok_or(Error::Invalid("mask for absent player"))?;
        if mask & 0x80 != 0 {
            animation::modes(&mut p, e, context.animation, false)?;
        }
        if mask & 0x40000 != 0 {
            variables::read(&mut p, e, context.variables, false, false)?;
        }
        if mask & 0x10 != 0 {
            e.face_override = p.alt3()?;
            if e.route_length == 0 {
                e.angle &= 16383;
                let d = (e.face_override - e.angle) & 16383;
                e.desired_angle = if d > 8192 {
                    e.angle - (16384 - d)
                } else {
                    e.angle + d
                };
                e.face_override = -1;
            }
        }
        if mask & 0x10000 != 0 {
            e.suppress_partner = (128u32.wrapping_sub(p.byte()?) & 255) == 1;
        }
        if mask & 0x20 != 0 {
            let cfg = context
                .combat
                .ok_or(Error::UnsupportedContext("combat config"))?;
            let state = e
                .combat
                .get_or_insert_with(|| crate::entities910::combat::Combat::new(cfg.slots));
            combat::read(&mut p, state, cfg, cycle, false)?;
        }
        if mask & 0x8000 != 0 {
            animation::overlays(&mut p, e, context.animation, false)?;
        }
        if mask & 0x100000 != 0 {
            animation::spot(&mut p, e, context.animation, 4, 3, 1, 1)?;
        }
        if mask & 4 != 0 {
            let len = p.byte()?.wrapping_sub(128) & 255;
            let mut data = vec![];
            for _ in 0..len {
                data.push(p.byte()?.wrapping_sub(128) as u8)
            }
            let mut cache = crate::entities910::appearance::CachedPacket { data, consumed: 0 };
            appearance::apply(
                &mut cache,
                e,
                a.ok_or(Error::UnsupportedContext("appearance config"))?,
            )?;
            s.appearances[i] = Some(cache);
        }
        if mask & 0x80000 != 0 {
            masks::wear(
                &mut p,
                e,
                a.ok_or(Error::UnsupportedContext("wear position config"))?
                    .wear
                    .len(),
                false,
            )?;
        }
        if mask & 0x2000 != 0 {
            animation::spot(&mut p, e, context.animation, 2, 0, 2, 1)?;
        }
        if mask & 2 != 0 {
            animation::spot(&mut p, e, context.animation, 0, 3, 3, 1)?;
        }
        if mask & 0x800 != 0 {
            let len = p.byte()?;
            let mut data = vec![];
            for _ in 0..len {
                data.push(p.byte()? as u8)
            }
            data.reverse();
            let mut cache = crate::entities910::appearance::CachedPacket { data, consumed: 0 };
            appearance::head_icons(&mut cache, e)?;
            s.head_icons[i] = Some(cache);
        }
        if mask & 0x20000 != 0 {
            variables::read(&mut p, e, context.variables, true, false)?;
        }
        if mask & 0x200 != 0 {
            chat::read(&mut p, e, context.chat_timeout, false, i == c.local, false)?;
        }
        if mask & 8 != 0 {
            masks::force(&mut p, e, cycle, false)?;
        }
        if mask & 1 != 0 {
            let t = p.g2()?;
            e.target = if t == 65535 { -1 } else { t };
        }
        if mask & 0x800000 != 0 {
            masks::tint(&mut p, e, cycle, false)?;
        }
        if mask & 0x400 != 0 {
            let v = p.byte()?.wrapping_neg() & 255;
            e.partner = if v <= 2 { v as i32 } else { 0 };
        }
        if mask & 0x100 != 0 {
            animation::spot(&mut p, e, context.animation, 1, 3, 2, 2)?;
        }
        if mask & 0x200000 != 0 {
            chat::read(&mut p, e, context.chat_timeout, true, i == c.local, false)?;
        }
        if mask & 0x400000 != 0 {
            animation::spot(&mut p, e, context.animation, 3, 3, 0, 1)?;
        }
    }
    if p.pos != bytes.len() {
        return Err(Error::Invalid("packet length"));
    }
    Ok(p.bit)
}
fn high(
    p: &mut Packet,
    s: &mut Players,
    c: &Context,
    i: usize,
    a: Option<&appearance::Config>,
    u: &mut PlayersUndo,
) -> Result<()> {
    let update = p.bits(1)? != 0;
    if update {
        s.update_ids.push(i)
    }
    let kind = p.bits(2)?;
    u.touch(s, i);
    let e = s.players[i]
        .as_mut()
        .ok_or(Error::Invalid("missing high player"))?;
    if kind == 0 {
        if !update {
            if i == c.local {
                return Err(Error::Invalid("remove local player"));
            }
            s.low[i] = Some(LowPlayer {
                partner: e.partner,
                coord: ((c.base_z.wrapping_add(e.z[0])) >> 6)
                    .wrapping_add((c.base_x.wrapping_add(e.x[0]) >> 6) << 14)
                    .wrapping_add(e.level << 28),
                angle: if e.face_override == -1 {
                    e.angle & 16383
                } else {
                    e.face_override
                },
                target: e.target,
                suppress_partner: e.suppress_partner,
            });
            s.players[i] = None;
            if p.bits(1)? != 0 {
                low(p, s, c, i, 0, a, u)?;
            }
        }
        return Ok(());
    }
    let x = e.x[0];
    let z = e.z[0];
    if kind == 1 {
        let d = p.bits(3)? as usize;
        let extra = p.bits(1)?;
        if extra != 0 {
            s.speeds[i] = 2;
            let (dx, dz) = [(0, 1), (-1, 0), (1, 0), (0, -1)][p.bits(2)? as usize];
            e.move_player(
                x.wrapping_add(dx),
                z.wrapping_add(dz),
                s.speeds[i],
                c.width,
                c.height,
            )
        }
        let (dx, dz) = STEP[d];
        e.move_player(
            x.wrapping_add(dx),
            z.wrapping_add(dz),
            s.speeds[i],
            c.width,
            c.height,
        );
    } else if kind == 2 {
        let (dx, dz) = RUN[p.bits(4)? as usize];
        e.move_player(
            x.wrapping_add(dx),
            z.wrapping_add(dz),
            s.speeds[i],
            c.width,
            c.height,
        );
    } else {
        let large = p.bits(1)? != 0;
        let (speed, dl, nx, nz) = if !large {
            let v = p.bits(15)?;
            let dx = (v >> 5) & 31;
            let dz = v & 31;
            (
                v >> 12,
                (v >> 10) & 3,
                x.wrapping_add(if dx > 15 { dx - 32 } else { dx }),
                z.wrapping_add(if dz > 15 { dz - 32 } else { dz }),
            )
        } else {
            let sp = p.bits(3)?;
            let v = p.bits(30)?;
            (
                sp,
                v >> 28,
                (c.base_x.wrapping_add(x).wrapping_add((v >> 14) & 16383) & 16383)
                    .wrapping_sub(c.base_x),
                (c.base_z.wrapping_add(z).wrapping_add(v & 16383) & 16383).wrapping_sub(c.base_z),
            )
        };
        if speed == 4 {
            e.tele(nx, nz)
        } else {
            s.speeds[i] = (speed - 1) as i8;
            e.move_player(nx, nz, s.speeds[i], c.width, c.height)
        }
        e.level = (e.level + dl) & 3;
        e.occlude_level = e.level + c.bridge(nx, nz);
        if i == c.local {
            s.current_level = e.level;
        }
    }
    Ok(())
}
fn low(
    p: &mut Packet,
    s: &mut Players,
    c: &Context,
    i: usize,
    depth: usize,
    a: Option<&appearance::Config>,
    u: &mut PlayersUndo,
) -> Result<bool> {
    // TODO(#gap-G-recursion): explicit safety limit; the reference decoder recurses without one.
    if depth > 64 {
        return Err(Error::Invalid("low recursion limit"));
    }
    u.touch(s, i);
    let kind = p.bits(2)?;
    if kind == 0 {
        if p.bits(1)? != 0 {
            low(p, s, c, i, depth + 1, a, u)?;
        }
        let x = p.bits(6)?;
        let z = p.bits(6)?;
        if p.bits(1)? != 0 {
            s.update_ids.push(i)
        }
        if s.players[i].is_some() {
            return Err(Error::Invalid("reuse occupied player"));
        }
        let l = s.low[i]
            .take()
            .ok_or(Error::Invalid("missing low player"))?;
        let mut e = Player::default();
        apply_cached(s, &mut e, i, a)?;
        e.angle = l.angle & 16383;
        e.desired_angle = e.angle;
        e.target = l.target;
        e.partner = l.partner;
        e.suppress_partner = l.suppress_partner;
        e.speeds[0] = s.speeds[i];
        e.level = l.coord >> 28;
        let nx = ((((l.coord >> 14) & 255) << 6) + x).wrapping_sub(c.base_x);
        let nz = (((l.coord & 255) << 6) + z).wrapping_sub(c.base_z);
        e.occlude_level = e.level + c.bridge(nx, nz);
        e.tele(nx, nz);
        s.players[i] = Some(e);
        s.generations[i] = s.generations[i].wrapping_add(1);
        return Ok(true);
    }
    let l = s.low[i]
        .as_mut()
        .ok_or(Error::Invalid("missing low player"))?;
    let v = l.coord;
    l.coord = if kind == 1 {
        ((((v >> 28) + p.bits(2)?) & 3) << 28) + (v & 0xfffffff)
    } else if kind == 2 {
        let k = p.bits(5)?;
        let (dx, dz) = STEP[(k & 7) as usize];
        ((((v >> 28) + (k >> 3)) & 3) << 28)
            .wrapping_add((((v >> 14) & 255) + dx) << 14)
            .wrapping_add((v & 255) + dz)
    } else {
        let k = p.bits(20)?;
        s.speeds[i] = ((k >> 18) - 1) as i8;
        ((((v >> 28) + ((k >> 16) & 3)) & 3) << 28)
            + ((((v >> 14) + ((k >> 8) & 255)) & 255) << 14)
            + (v.wrapping_add(k & 255) & 255)
    };
    Ok(false)
}
pub mod npc;
pub mod zone;

pub mod appearance;

fn apply_cached(
    s: &mut Players,
    e: &mut Player,
    i: usize,
    a: Option<&appearance::Config>,
) -> Result<()> {
    if let Some(cache) = &mut s.appearances[i] {
        appearance::apply(
            cache,
            e,
            a.ok_or(Error::UnsupportedContext("cached appearance config"))?,
        )?;
    }
    if let Some(cache) = &mut s.head_icons[i] {
        appearance::head_icons(cache, e)?;
    }
    Ok(())
}

mod masks;

pub mod combat;

pub mod animation;

pub mod variables;

pub mod npc_custom;

pub mod chat;

pub mod zone_state;

pub mod transient;

pub mod live;

pub mod rebuild_state;

/// The config decoders and pack adapters (moved to rs910-config's
/// `types910`, Phase 2.6).
pub use rs910_config::types910::{
    bas_types, combat_types, config_types, defaults, effect_types, idk_types, pack_defaults,
    pack_types, script_types, sequence_types, titles, varbits, variable_types,
};
pub mod terrain;

pub mod varp;
pub mod walk_animation;
