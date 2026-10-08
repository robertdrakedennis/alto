//! Ordered player/NPC logic updates, including the movement/facing/animation
//! interactions. Called once per logic cycle.
//! Sequence sound callbacks are retained as actor events and forwarded by the
//! game/session owner into the existing audio request queue.
pub use crate::sequence_sound::SequenceSound;
use crate::sequence_sound::{self, Emitter};
use crate::{
    animation_playback::{self, AnimationRandom},
    entities910::{
        animation_state::{Config, Node},
        movement, Player,
    },
    protocol910::{
        bas_types::Bas,
        config_types,
        npc::Npcs,
        sequence_types::Sequence,
        terrain::{self, Terrain},
        walk_animation, Context, Error, Players,
    },
};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Error>;

pub struct Inputs<'a> {
    pub selection: &'a Config,
    pub sequences: &'a BTreeMap<i32, Sequence>,
    pub bases: &'a BTreeMap<i32, Bas>,
    pub npcs: &'a BTreeMap<i32, config_types::Npc>,
    /// The local player's variable state, read to resolve multi-NPC types
    /// (a varbit when `bit`, else a varp); `None` reads every variable as
    /// absent.
    pub vars: Option<&'a dyn Fn(bool, i32) -> Option<i32>>,
}

/// The BAS the actor update uses for an NPC and whether a `multinpc` type
/// currently resolves. The override wins, then a resolved morph's own BAS,
/// then the base type's.
fn npc_bas(bas_override: i32, t: &config_types::Npc, c: &Inputs) -> (i32, bool) {
    let selected = t.multinpc.as_ref().map(|list| {
        let read = |bit: bool, id: i32| c.vars.and_then(|vars| vars(bit, id));
        crate::config::select_multi(t.multivarbit, t.multivarp, list, &read)
    });
    let visible = !matches!(selected, Some(None));
    if bas_override != -1 {
        return (bas_override, visible);
    }
    // An absent id lists as a default type whose `bas` is -1.
    let morph = selected
        .flatten()
        .and_then(|id| c.npcs.get(&(id as i32)))
        .map_or(-1, |morph| morph.bas);
    (if morph != -1 { morph } else { t.bas }, visible)
}

fn sequence<'a>(n: &Node, c: &'a Inputs) -> Result<&'a Sequence> {
    c.sequences
        .get(&n.id())
        .ok_or(Error::UnsupportedContext("actor sequence"))
}
/// Advance a node one tick and play the sequence sounds it raised.
fn advance(
    n: &mut Node,
    emitter: Option<Emitter>,
    c: &Inputs,
    rng: &mut AnimationRandom,
    sounds: &mut Vec<SequenceSound>,
) -> Result<bool> {
    if n.sequence.is_none() {
        return Ok(false);
    }
    let changed = animation_playback::advance(n, sequence(n, c)?, 1, n.skeletal_range, rng);
    play_triggers(n, emitter, c, rng, sounds);
    Ok(changed)
}

fn play_triggers(
    n: &mut Node,
    emitter: Option<Emitter>,
    c: &Inputs,
    rng: &mut AnimationRandom,
    sounds: &mut Vec<SequenceSound>,
) {
    sequence_sound::play_triggers(n, emitter, c.sequences, rng, sounds);
}
fn gate(n: &Node) -> Option<movement::Gate> {
    n.sequence.as_ref().map(|s| movement::Gate {
        delayed: n.delay != 0,
        moving_priority: s.moving,
        stationary_priority: s.stationary,
    })
}
/// Spot-animation update: walk restart, delayed spot gates, main suppression,
/// then overlay delay/advance. These are distinct from movement's blocking rules.
fn animations(
    e: &mut Player,
    b: &Bas,
    c: &Inputs,
    cycle: i32,
    kind: &Kind,
    rng: &mut AnimationRandom,
    sounds: &mut Vec<SequenceSound>,
) -> Result<()> {
    let emitter = Emitter {
        level: e.level,
        fine_x: e.fine_x,
        fine_z: e.fine_z,
        local: kind.local,
        visible: kind.visible,
        targeted: if kind.own >= 32768 {
            -(kind.own - 32768) - 1
        } else {
            kind.own + 1
        },
        listener: kind.listener,
    };
    // The walk animation stays silent while an idle animation of it runs under
    // a started main animation.
    let walk_silent =
        e.actor.walk.idle && e.animation.main.sequence.is_some() && e.animation.main.delay == 0;
    let walk = &mut e.actor.walk;
    let walk_emitter = (!walk_silent).then_some(emitter);
    if advance(&mut walk.node, walk_emitter, c, rng, sounds)? && walk.node.finished {
        if walk.idle {
            let id = walk_animation::idle_animation(b, &mut || rng.next())?;
            let prior = walk.node.id();
            walk.node.set(id, 0, 0, c.selection)?;
            if id != prior && walk.node.sequence.is_some() {
                walk.node.flag = true;
            }
            walk.idle = walk.node.sequence.is_some();
        }
        // Restarting dereferences the newly selected sequence.
        if walk.node.sequence.is_none() {
            return Err(Error::Invalid("walk restart without sequence"));
        }
        walk.node.restart(0);
    }
    let moving = e.steps_remaining > 0 && e.forced[6] <= cycle && e.forced[7] < cycle;
    for spot in &mut e.animation.spots {
        if spot.id == -1 {
            continue;
        }
        if spot.node.delay != 0 {
            let effect = c
                .selection
                .effects
                .get(&spot.id)
                .ok_or(Error::UnsupportedContext("actor spot type"))?;
            if effect.looping {
                let priority = sequence(&spot.node, c)?.moving;
                if priority == 3 && moving {
                    spot.node.set(-1, 0, 0, c.selection)?;
                    spot.id = -1;
                    continue;
                }
                if priority == 1 && moving {
                    continue;
                }
            }
        }
        if advance(&mut spot.node, Some(emitter), c, rng, sounds)? && spot.node.finished {
            spot.node.set(-1, 0, 0, c.selection)?;
            spot.id = -1;
        }
    }
    let n = &mut e.animation.main;
    if let Some(seq) = &n.sequence {
        let priority = seq.moving;
        if priority == 3 && moving {
            e.animation.modes = None;
            n.set(-1, 0, 0, c.selection)?;
        } else if priority == 1 && moving {
            n.delay = 1;
        } else {
            if priority == 1 {
                n.delay = 0;
            }
            if advance(n, Some(emitter), c, rng, sounds)? && n.finished {
                e.animation.modes = None;
                n.set(-1, 0, 0, c.selection)?;
            }
        }
    }
    for slot in &mut e.animation.overlays {
        if let Some(n) = slot {
            if n.overlay_delay > 0 {
                n.overlay_delay -= 1;
            } else if advance(n, Some(emitter), c, rng, sounds)? && n.finished {
                *slot = None;
            }
        }
    }
    Ok(())
}
/// Per-wear-slot turning and deferred release.
fn wear_angle(e: &mut Player, b: &Bas, slots: usize, slot: usize, target: i32) -> Result<bool> {
    if e.actor.wear_angles.is_none() {
        if target == -1 {
            return Ok(true);
        }
        e.actor.wear_angles = Some(vec![-1; slots]);
    }
    let speed = if let Some(s) = &b.wear_turn_speeds {
        *s.get(slot)
            .ok_or(Error::Invalid("BAS wear rotation slot"))?
    } else {
        0
    };
    let speed = if speed > 0 { speed } else { 256 };
    let angles = e.actor.wear_angles.as_mut().unwrap();
    let old = angles
        .get_mut(slot)
        .ok_or(Error::Invalid("wear rotation slot"))?;
    if target == -1 && *old == -1 {
        return Ok(true);
    }
    if *old == -1 {
        *old = e.angle & 16383;
    }
    let delta = (if target == -1 {
        e.angle & 16383
    } else {
        target
    })
    .wrapping_sub(*old);
    if delta < -speed || delta > speed {
        *old = if (delta <= 0 || delta > 8192) && delta > -8192 {
            old.wrapping_sub(speed)
        } else {
            old.wrapping_add(speed)
        } & 16383;
        return Ok(false);
    }
    *old = target;
    if target == -1 && angles.iter().all(|&a| a == -1) {
        e.actor.wear_angles = None;
    }
    Ok(true)
}
fn atan(x: i32, z: i32) -> i32 {
    ((x as f64).atan2(z as f64) * 2607.5945876176133) as i32 & 16383
}
fn wear(
    e: &mut Player,
    b: &Bas,
    c: &Inputs,
    map: &Context,
    own: i32,
    target: &impl Fn(i32) -> Option<[f32; 2]>,
) -> Result<()> {
    if e.wear.is_none() && e.actor.wear_angles.is_none() {
        return Ok(());
    }
    let targets = e
        .wear
        .clone()
        .ok_or(Error::Invalid("null wear map with active angles"))?;
    let mut clear = true;
    for (slot, id) in targets.into_iter().enumerate() {
        if id == -1 {
            if !wear_angle(e, b, c.selection.slots, slot, -1)? {
                clear = false;
            }
            continue;
        }
        clear = false;
        let delta = if id & 0xc0000000u32 as i32 == 0xc0000000u32 as i32 {
            let coord = id & 0xfffffff;
            Some((
                (e.fine_x as i32).wrapping_sub(
                    ((coord >> 14).wrapping_sub(map.base_x))
                        .wrapping_mul(512)
                        .wrapping_add(256),
                ),
                (e.fine_z as i32).wrapping_sub(
                    ((coord & 16383).wrapping_sub(map.base_z))
                        .wrapping_mul(512)
                        .wrapping_add(256),
                ),
            ))
        } else {
            let id = if id & 32768 == 0 {
                id
            } else {
                (id & 32767) + 32768
            };
            let pos = if id == own {
                Some([e.fine_x, e.fine_z])
            } else {
                target(id)
            };
            pos.map(|p| {
                (
                    (e.fine_x as i32).wrapping_sub(p[0] as i32),
                    (e.fine_z as i32).wrapping_sub(p[1] as i32),
                )
            })
        };
        match delta {
            None => {
                wear_angle(e, b, c.selection.slots, slot, -1)?;
            }
            Some((0, 0)) => {}
            Some((x, z)) => {
                wear_angle(e, b, c.selection.slots, slot, atan(x, z))?;
            }
        }
    }
    if clear {
        e.wear = None;
        e.actor.wear_angles = None;
    }
    Ok(())
}
/// The actor's yaw/pitch/roll quaternion. The f32 operations and quaternion
/// multiplication order are kept exactly, including signed zero.
pub fn rotation(yaw: i32, pitch: i32, roll: i32) -> [f32; 4] {
    let axis = |n: i32, slot: usize| {
        let radians = (((n & 16383) as f32 / 16384.) as f64 * std::f64::consts::TAU) as f32;
        let half = (radians * 0.5) as f64;
        let s = half.sin() as f32;
        let mut q = [0. * s, 0. * s, 0. * s, half.cos() as f32];
        q[slot] = s;
        q
    };
    let mul = |a: [f32; 4], b: [f32; 4]| {
        [
            a[2] * b[1] + a[0] * b[3] + a[3] * b[0] - a[1] * b[2],
            a[0] * b[2] + a[3] * b[1] + (a[1] * b[3] - a[2] * b[0]),
            a[3] * b[2] + (a[2] * b[3] + a[1] * b[0] - a[0] * b[1]),
            a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
        ]
    };
    mul(mul(axis(yaw, 1), axis(pitch, 0)), axis(roll, 2))
}
struct Kind {
    own: i32,
    local: bool,
    /// The visibility gate for sequence sounds.
    visible: bool,
    /// The local player's level and target id.
    listener: Option<(i32, i32)>,
    turn: i32,
    smoothing: bool,
    face: Option<(i32, i32)>,
    /// Cutscene actors: the movement update skips the far-step snap and the
    /// catch-up speeds.
    special: bool,
}
/// Everything one logic pass reads: the config tables, the map, the scene
/// terrain and the current game cycle.
#[derive(Clone, Copy)]
pub struct TickContext<'a> {
    pub inputs: &'a Inputs<'a>,
    pub map: &'a Context,
    pub scene: Option<&'a Terrain>,
    pub cycle: i32,
}

/// What one logic pass writes besides the actors: the animation random
/// source and the sequence sounds collected for the audio owner.
pub struct TickOutput<'a> {
    pub rng: &'a mut AnimationRandom,
    pub sounds: &'a mut Vec<SequenceSound>,
}

/// One actor's logic update.
fn update(
    prior: &Player,
    b: &Bas,
    tick: &TickContext,
    kind: Kind,
    target: impl Fn(i32) -> Option<[f32; 2]>,
    out: &mut TickOutput,
) -> Result<(Player, bool)> {
    let TickContext {
        inputs: c,
        map,
        scene,
        cycle,
    } = *tick;
    let (rng, sounds) = (&mut *out.rng, &mut *out.sounds);
    let motion = b.movement();
    let mut spots = vec![];
    for s in &prior.animation.spots {
        if s.id != -1 && s.node.delay != 0 {
            let t = c
                .selection
                .effects
                .get(&s.id)
                .ok_or(Error::UnsupportedContext("actor movement spot"))?;
            if t.looping && t.sequence != -1 {
                let seq = c
                    .sequences
                    .get(&t.sequence)
                    .ok_or(Error::UnsupportedContext("actor movement spot sequence"))?;
                spots.push(movement::Gate {
                    delayed: true,
                    moving_priority: seq.moving,
                    stationary_priority: seq.stationary,
                });
            }
        }
    }
    let context = movement::Context {
        bas: &motion,
        turn_speed: kind.turn,
        smoothing: kind.smoothing,
        special: kind.special,
        main: gate(&prior.animation.main),
        spots,
    };
    let (mut e, mut speed, mut direction) = if prior.forced[6] > cycle || prior.forced[7] >= cycle {
        let n = &prior.animation.main;
        let can_advance = if n.sequence.is_none() || 1i32.wrapping_sub(n.delay) <= 0 {
            false
        } else {
            let seq = sequence(n, c)?;
            seq.skeletal != -1
                || seq.tween
                || n.time.wrapping_add(1i32.wrapping_sub(n.delay))
                    > *seq
                        .frames
                        .as_ref()
                        .and_then(|f| f.get(n.frame as usize))
                        .ok_or(Error::Invalid("forced animation frame"))?
        };
        // Propagate invalid terrain context instead of inventing a height.
        let failure = std::cell::RefCell::new(None);
        let e = movement::advance_forced(prior, &context, cycle, can_advance, |x, z, l| {
            match terrain::height(scene, x, z, l) {
                Ok(y) => y,
                Err(error) => {
                    *failure.borrow_mut() = Some(error);
                    0
                }
            }
        })?;
        if let Some(error) = failure.into_inner() {
            return Err(error);
        }
        (e, -1, 0)
    } else {
        let s = movement::advance_route(prior, &context)?;
        (s.state, s.speed, s.direction)
    };
    let x = e.fine_x as i32;
    let z = e.fine_z as i32;
    if x < 512
        || z < 512
        || x >= (map.width - 1) * 512
        || z >= (map.height - 1) * 512
        || (kind.local
            && (x < 6144
                || z < 6144
                || x >= (map.width - 12) * 512
                || z >= (map.height - 12) * 512))
    {
        e.animation.main.set(-1, 0, 0, c.selection)?;
        for s in &mut e.animation.spots {
            s.id = -1;
            s.node.set(-1, 0, 0, c.selection)?;
        }
        e.animation.modes = None;
        e.forced[6] = 0;
        e.forced[7] = 0;
        speed = -1;
        direction = 0;
        e.fine_x = e.x[0]
            .wrapping_mul(512)
            .wrapping_add(e.size.wrapping_mul(256)) as f32;
        e.fine_z = e.z[0]
            .wrapping_mul(512)
            .wrapping_add(e.size.wrapping_mul(256)) as f32;
        e.route_length = 0;
        e.steps_remaining = 0;
    }
    let delta = if e.target == kind.own {
        Some((0, 0))
    } else {
        target(e.target).map(|p| ((e.fine_x - p[0]) as i32, (e.fine_z - p[1]) as i32))
    };
    let tile = kind.face.map(|(x, z)| {
        (
            (e.fine_x as i32).wrapping_sub(
                x.wrapping_mul(256)
                    .wrapping_sub(map.base_x.wrapping_mul(512)),
            ),
            (e.fine_z as i32).wrapping_sub(
                z.wrapping_mul(256)
                    .wrapping_sub(map.base_z.wrapping_mul(512)),
            ),
        )
    });
    let (mut e, turn, consumed) =
        movement::advance_facing(&e, &context, delta, tile, kind.own < 32768)?;
    wear(&mut e, b, c, map, kind.own, &target)?;
    walk_animation::update(
        &mut e.actor.walk,
        b,
        &walk_animation::Input {
            speed: speed as i32,
            direction,
            turn,
            angle: e.angle,
            desired: e.desired_angle,
            turn_ticks: e.motion.turn_ticks,
            target: e.target,
        },
        c.selection,
        &mut || rng.next(),
    )?;
    e.animation
        .select_for_speed(speed, e.route_length, &mut e.steps_remaining, c.selection)?;
    animations(&mut e, b, c, cycle, &kind, rng, sounds)?;
    e.actor.rotation = rotation(e.angle, e.motion.pitch[0], e.motion.roll[0]);
    Ok((e, consumed))
}
/// Players then NPCs update in wire index order. Targets see any earlier
/// actor's newly updated position, including the player-to-NPC boundary.
/// Chat expiry follows both complete passes.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "update-order reference; production drives actors through entity_runtime (tests only)"
    )
)]
pub fn tick(
    players: &mut Players,
    npcs: &mut Npcs,
    c: &Inputs,
    map: &Context,
    scene: Option<&Terrain>,
    cycle: i32,
    rng: &mut AnimationRandom,
) -> Result<()> {
    let mut sounds = Vec::new();
    let tick = TickContext {
        inputs: c,
        map,
        scene,
        cycle,
    };
    tick_with_sounds(
        players,
        npcs,
        &tick,
        &mut TickOutput {
            rng,
            sounds: &mut sounds,
        },
    )
}

/// The player/NPC logic pass, collecting sequence sounds.
pub fn tick_with_sounds(
    players: &mut Players,
    npcs: &mut Npcs,
    tick: &TickContext,
    out: &mut TickOutput,
) -> Result<()> {
    let c = tick.inputs;
    let map = tick.map;
    let default_bas = Bas::default();
    let bas = |id| {
        if id == -1 {
            Ok(&default_bas)
        } else {
            c.bases
                .get(&id)
                .ok_or(Error::UnsupportedContext("actor BAS"))
        }
    };
    let position = |id: i32, p: &Players, n: &Npcs| {
        if id < 0 {
            None
        } else if id < 32768 {
            n.entities
                .get(&(id as usize))
                .map(|n| [n.path.fine_x, n.path.fine_z])
        } else {
            p.players
                .get((id - 32768) as usize)
                .and_then(Option::as_ref)
                .map(|p| [p.fine_x, p.fine_z])
        }
    };
    // The local player's level changes only through packets, so one sample
    // holds for the whole logic pass.
    let listener = players
        .players
        .get(map.local)
        .and_then(Option::as_ref)
        .map(|p| (p.level, -(map.local as i32) - 1));
    for &id in &players.high_indices.clone() {
        if let Some(p) = players.players.get(id).and_then(Option::as_ref) {
            let (next, _) = update(
                p,
                bas(p.appearance.bas)?,
                tick,
                Kind {
                    own: id as i32 + 32768,
                    local: id == map.local,
                    visible: true,
                    listener,
                    turn: 256,
                    smoothing: true,
                    face: None,
                    special: false,
                },
                |target| position(target, players, npcs),
                out,
            )?;
            players.players[id] = Some(next);
        }
    }
    for &id in &npcs.slots.clone() {
        if let Some(n) = npcs.entities.get(&id) {
            let t = c
                .npcs
                .get(&n.type_id)
                .ok_or(Error::UnsupportedContext("actor NPC type"))?;
            let (bas_id, visible) = npc_bas(n.bas_override, t, c);
            let (next, consumed) = update(
                &n.path,
                bas(bas_id)?,
                tick,
                Kind {
                    own: id as i32,
                    local: false,
                    visible,
                    listener,
                    turn: n.turn_speed,
                    smoothing: t.walksmoothing,
                    face: if n.face_x == -1 {
                        None
                    } else {
                        Some((n.face_x, n.face_z))
                    },
                    special: false,
                },
                |target| position(target, players, npcs),
                out,
            )?;
            let n = npcs.entities.get_mut(&id).unwrap();
            n.path = next;
            if consumed {
                n.face_x = -1;
            }
        }
    }
    for &id in &players.high_indices {
        if let Some(p) = players.players.get_mut(id).and_then(Option::as_mut) {
            if let Some(chat) = &mut p.chat {
                chat.tick();
            }
        }
    }
    for id in &npcs.slots {
        if let Some(n) = npcs.entities.get_mut(id) {
            if let Some(chat) = &mut n.path.chat {
                chat.tick();
            }
        }
    }
    Ok(())
}

/// The logic pass and chat expiry for the cutscene entities, in the
/// cutscene's entity array order. `order` is `(entity index, is_npc, actor
/// key)`; players live at their slot in `players`, NPCs under their entity
/// index in `npcs`. `listener` is the world's local player (level, local
/// target index): audio still listens from it while a cutscene plays.
pub fn tick_cutscene(
    players: &mut Players,
    npcs: &mut Npcs,
    order: &[(usize, bool, usize)],
    tick: &TickContext,
    out: &mut TickOutput,
    listener: Option<(i32, i32)>,
) -> Result<()> {
    let c = tick.inputs;
    let default_bas = Bas::default();
    let bas = |id| {
        if id == -1 {
            Ok(&default_bas)
        } else {
            c.bases
                .get(&id)
                .ok_or(Error::UnsupportedContext("actor BAS"))
        }
    };
    // Cutscene actors are never targeted: every target resolves to none.
    let none = |_: i32| None;
    for &(_, npc, key) in order {
        if npc {
            let Some(n) = npcs.entities.get(&key) else {
                continue;
            };
            let t = c
                .npcs
                .get(&n.type_id)
                .ok_or(Error::UnsupportedContext("actor NPC type"))?;
            let (bas_id, visible) = npc_bas(n.bas_override, t, c);
            let (next, consumed) = update(
                &n.path,
                bas(bas_id)?,
                tick,
                Kind {
                    own: key as i32,
                    local: false,
                    turn: n.turn_speed,
                    smoothing: t.walksmoothing,
                    face: if n.face_x == -1 {
                        None
                    } else {
                        Some((n.face_x, n.face_z))
                    },
                    special: true,
                    visible,
                    listener,
                },
                none,
                out,
            )?;
            let n = npcs.entities.get_mut(&key).unwrap();
            n.path = next;
            if consumed {
                n.face_x = -1;
            }
        } else if let Some(p) = players.players.get(key).and_then(Option::as_ref) {
            let (next, _) = update(
                p,
                bas(p.appearance.bas)?,
                tick,
                Kind {
                    own: key as i32 + 32768,
                    local: false,
                    turn: 256,
                    smoothing: true,
                    face: None,
                    special: true,
                    visible: true,
                    listener,
                },
                none,
                out,
            )?;
            players.players[key] = Some(next);
        }
    }
    for &(_, npc, key) in order {
        let chat = if npc {
            npcs.entities
                .get_mut(&key)
                .and_then(|n| n.path.chat.as_mut())
        } else {
            players
                .players
                .get_mut(key)
                .and_then(Option::as_mut)
                .and_then(|p| p.chat.as_mut())
        };
        if let Some(chat) = chat {
            chat.tick();
        }
    }
    Ok(())
}

#[cfg(test)]
mod sequence_sound_tests {
    use super::*;

    fn fixture(
        sound: Vec<i32>,
    ) -> (
        BTreeMap<i32, Sequence>,
        crate::entities910::animation_state::Config,
    ) {
        let mut seq = Sequence::empty(7);
        seq.sound = Some(vec![Some(sound)]);
        seq.volume = Some(vec![200]);
        seq.remote_volume_percent = 40;
        let sequences = BTreeMap::from([(7, seq)]);
        let config = crate::entities910::animation_state::Config {
            sequences: BTreeMap::new(),
            effects: BTreeMap::new(),
            slots: 0,
        };
        (sequences, config)
    }

    #[test]
    fn npc_morph_bas_follows_override_and_multinpc_resolution() {
        let (sequences, selection) = fixture(vec![]);
        let bases = BTreeMap::new();
        let mut base = config_types::Npc::empty(1);
        base.bas = 100;
        base.multivarp = 5;
        // multinpc [2, -1, 3, default 4].
        base.multinpc = Some(vec![2, -1, 3, 4]);
        let mut npcs = BTreeMap::new();
        let mut morph = config_types::Npc::empty(2);
        morph.bas = 200;
        npcs.insert(2, morph);
        // Type 3 keeps the default BAS -1; type 4 is absent from the list.
        npcs.insert(3, config_types::Npc::empty(3));
        let value = std::cell::Cell::new(0);
        let read = |bit: bool, id: i32| (!bit && id == 5).then(|| value.get());
        let c = Inputs {
            selection: &selection,
            sequences: &sequences,
            bases: &bases,
            npcs: &npcs,
            vars: Some(&read),
        };
        // A resolved morph with its own BAS supplies it.
        assert_eq!(npc_bas(-1, &base, &c), (200, true));
        // The BAS override wins over the morph.
        assert_eq!(npc_bas(7, &base, &c), (7, true));
        // A null selection keeps the base BAS and hides the NPC.
        value.set(1);
        assert_eq!(npc_bas(-1, &base, &c), (100, false));
        // A morph without BAS (-1) falls back to the base type's.
        value.set(2);
        assert_eq!(npc_bas(-1, &base, &c), (100, true));
        // Out of range selects the default entry; a missing type lists as a
        // default type (BAS -1).
        value.set(9);
        assert_eq!(npc_bas(-1, &base, &c), (100, true));
        // Plain types are always visible with their own BAS.
        assert_eq!(npc_bas(-1, &npcs[&2], &c), (200, true));
    }
}
