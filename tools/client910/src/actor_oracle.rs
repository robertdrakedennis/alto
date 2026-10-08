//! Runs the production actor scheduler against recorded entity-update calls.
use crate::{
    actor,
    animation_playback::AnimationRandom,
    entities910::{
        animation_state::{Config, Effect, Node},
        chat::Chat,
        Npc, Player,
    },
    protocol910::{
        bas_types::Bas, config_types, npc::Npcs, sequence_types::Sequence, terrain::Terrain,
        Players,
    },
};
use std::{
    collections::BTreeMap,
    io::{BufWriter, Write},
};
fn sequences() -> BTreeMap<i32, Sequence> {
    (1..=25)
        .map(|id| {
            let mut s = Sequence::empty(id);
            s.frames = Some(vec![2 + id % 3, 3, 4]);
            s.frame_ids = Some(vec![1, 2, 3]);
            s.replayoff = id % 4 - 1;
            s.replaycount = 1 + id % 3;
            s.moving = id % 4;
            s.stationary = (id + 1) % 3;
            s.tween = id % 2 == 0;
            if id == 25 {
                s.skeletal = 0;
            }
            (id, s)
        })
        .collect()
}
fn bas(v: i32) -> Bas {
    let mut b = Bas::default();
    let mut fields = [
        &mut b.readyanim,
        &mut b.readyanim_l,
        &mut b.readyanim_r,
        &mut b.walkanim,
        &mut b.walkanim_b,
        &mut b.walkanim_l,
        &mut b.walkanim_r,
        &mut b.runanim,
        &mut b.runanim_b,
        &mut b.runanim_l,
        &mut b.runanim_r,
        &mut b.crawlanim,
        &mut b.crawlanim_b,
        &mut b.crawlanim_l,
        &mut b.crawlanim_r,
        &mut b.crawl_turn_left,
        &mut b.crawl_turn_right,
        &mut b.run_turn_left,
        &mut b.run_turn_right,
        &mut b.walk_turn_left,
        &mut b.walk_turn_right,
    ];
    for (i, f) in fields.iter_mut().enumerate() {
        **f = if (v + i as i32) % 7 == 0 {
            -1
        } else {
            i as i32 + 1
        };
    }
    b.readyanim = -1;
    b.extra_seq_ids = Some(vec![22, 23, 24]);
    b.idle_weights = Some(vec![1, 3, 2]);
    b.idle_weight_total = 6;
    b.walkspeed = if v % 3 == 0 { -1 } else { 32 + v % 4 * 16 };
    b.turn_accel = if v % 2 == 0 { 0 } else { 64 };
    b.turn_max = 512;
    b.roll_accel = if v % 3 == 1 { 20 } else { 0 };
    b.roll_max = 80;
    b.roll_target = 100;
    b.pitch_accel = if v % 3 == 2 { 15 } else { 0 };
    b.pitch_max = 60;
    b.pitch_target = 90;
    b.wear_turn_speeds = Some(vec![0, 17, 512]);
    b
}
fn entity(v: i32, k: i32, c: &Config) -> Player {
    let mut e = Player {
        fine_x: 25856.25 + k as f32 * 120.,
        fine_z: 25856.5 - k as f32 * 130.,
        ..Default::default()
    };
    e.motion.y = 37.75;
    e.route_length = ((v + k) % 4) as usize;
    e.steps_remaining = (v + k) % 3;
    e.seq_trigger = (v + k) % 2;
    e.angle = (v * 137 + k * 4096) & 16383;
    e.desired_angle = e.angle;
    e.target = [7, 32771, 2, 32769][k as usize];
    for j in 0..10 {
        e.x[j] = 50 + (j as i32 + 1) * ((v + k) % 3 - 1);
        e.z[j] = 50 + (j as i32 + 1) * ((v + k + 1) % 3 - 1);
        e.speeds[j] = ((v + k) % 3) as i8;
    }
    e.animation
        .main
        .set(1 + (v + k) % 25, (v + k) % 4, 0, c)
        .unwrap();
    e.animation.overlays = (0..3)
        .map(|j| {
            let mut n = Node::default();
            n.set(1 + (v + k + j) % 25, 0, 0, c).unwrap();
            n.overlay_delay = j;
            Some(n)
        })
        .collect();
    for j in 0..5 {
        let id = 1 + (v + k + j) % 25;
        let s = &mut e.animation.spots[j as usize];
        s.id = id;
        s.node
            .set(id, j % 3, if id % 2 == 1 { 0 } else { 2 }, c)
            .unwrap();
    }
    e.chat = Some(Chat {
        text: Some("fixture".into()),
        colour: 2,
        effect: 3,
        total: 1 + (v + k) % 12,
        time: 1 + (v + k) % 12,
    });
    e.wear = Some(vec![
        0xc0000000u32 as i32 | ((3155 + k) << 14) | (3255 - k),
        [32769, 7, 32771, 2][k as usize],
        900,
    ]);
    if (v + k) % 3 == 0 {
        e.forced = [51, 49, 55, 45, 1, 2, 7, 17, 12000];
    }
    e
}
fn prepare(n: &mut Node, t: i32, k: i32, v: i32) {
    if n.sequence.as_ref().is_some_and(|s| s.skeletal) && (t + k + v) % 3 == 0 {
        n.skeletal_range = Some((2, 11));
    }
}
fn external(e: &mut Player, t: i32, k: i32, v: i32) {
    if t == 5 {
        e.wear = Some(vec![-1; 3]);
    }
    if t == 11 {
        e.wear = Some(vec![
            [32771, 32769, 7, 2][k as usize],
            900,
            0xc0000000u32 as i32 | (3151 << 14) | 3252,
        ]);
    }
    if t == 19 {
        e.animation.modes = Some(vec![25, 7, 11, 15, 19]);
        e.animation.main.delay = 2;
        e.steps_remaining = e.route_length as i32;
    }
    if t == 30 {
        e.fine_x = if k % 2 == 0 { 500. } else { 6000. };
    }
    if t == 32 {
        e.route_length = 2;
        e.x[0] = 52;
        e.z[0] = 51;
        e.x[1] = 51;
        e.z[1] = 50;
        e.speeds[0] = 2;
        e.speeds[1] = 1;
    }
    if t == 44 {
        e.wear = Some(vec![-1; 3]);
        if k < 2 {
            e.face_override = 4000;
        }
    }
    prepare(&mut e.animation.main, t, k, v);
    prepare(&mut e.actor.walk.node, t, k, v);
    for s in &mut e.animation.spots {
        prepare(&mut s.node, t, k, v);
    }
    for n in e.animation.overlays.iter_mut().flatten() {
        prepare(n, t, k, v);
    }
}
struct Dump {
    b: Vec<u8>,
    labels: Vec<(usize, String)>,
}
impl Dump {
    fn ints(&mut self, name: &str, vs: &[i32]) {
        self.labels.push((self.b.len(), name.into()));
        for v in vs {
            self.b.extend(v.to_be_bytes());
        }
    }
    fn array(&mut self, name: &str, a: Option<&[i32]>) {
        self.ints(name, &[a.map_or(-1, |a| a.len() as i32)]);
        if let Some(a) = a {
            self.ints(name, a);
        }
    }
    fn node(&mut self, name: &str, n: Option<&Node>, delay: i32) {
        if let Some(n) = n {
            self.ints(
                name,
                &[
                    1,
                    n.id(),
                    n.time,
                    n.delay,
                    n.loops,
                    n.frame,
                    n.next,
                    n.finished as i32,
                    n.mode,
                    n.flag as i32,
                    delay,
                    (n.sequence.as_ref().is_some_and(|s| s.skeletal) && n.skeletal_range.is_some())
                        as i32,
                ],
            );
        } else {
            self.ints(name, &[0]);
        }
    }
    fn actor(&mut self, e: &Player, face: i32) {
        self.ints(
            "motion",
            &[
                e.route_length as i32,
                e.steps_remaining,
                e.seq_trigger,
                e.motion.speed,
                e.angle,
                e.motion.yaw_velocity,
                e.desired_angle,
                e.motion.turn_ticks,
                e.motion.roll[0],
                e.motion.roll[1],
                e.motion.pitch[0],
                e.motion.pitch[1],
                e.target,
                face,
                e.forced[6],
                e.forced[7],
            ],
        );
        self.ints(
            "position",
            &[
                e.fine_x.to_bits() as i32,
                e.motion.y.to_bits() as i32,
                e.fine_z.to_bits() as i32,
            ],
        );
        self.ints("rotation", &e.actor.rotation.map(|v| v.to_bits() as i32));
        for j in 0..10 {
            self.ints("route", &[e.x[j], e.z[j], e.speeds[j] as i32]);
        }
        self.array("wear", e.wear.as_deref());
        self.array("wear-angles", e.actor.wear_angles.as_deref());
        self.array("modes", e.animation.modes.as_deref());
        self.node("walk", Some(&e.actor.walk.node), 0);
        self.ints("idle", &[e.actor.walk.idle as i32]);
        self.node("main", Some(&e.animation.main), 0);
        for s in &e.animation.spots {
            self.ints("spot-id", &[s.id]);
            self.node("spot", Some(&s.node), 0);
        }
        for n in &e.animation.overlays {
            self.node(
                "overlay",
                n.as_ref(),
                n.as_ref().map_or(0, |n| n.overlay_delay),
            );
        }
        let chat = e.chat.as_ref().unwrap();
        self.ints("chat", &[chat.text.is_some() as i32, chat.time]);
    }
}
#[test]
fn combined_scheduler_replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("actors");
    let out = scratch.dir().to_path_buf();
    let mut bytes = BufWriter::new(std::fs::File::create(out.join("rust.bin"))?);
    let mut offsets = BufWriter::new(std::fs::File::create(out.join("offsets.txt"))?);
    let seq = sequences();
    let c = Config {
        sequences: seq.iter().map(|(&id, s)| (id, s.selection())).collect(),
        effects: (1..=25)
            .map(|id| {
                (
                    id,
                    Effect {
                        sequence: id,
                        looping: id % 2 == 1,
                    },
                )
            })
            .collect(),
        slots: 3,
    };
    let mut terrain = Terrain::new(104, 104).unwrap();
    for l in 0..4 {
        for x in 0..=104 {
            for z in 0..=104 {
                let i = terrain.point(l, x, z);
                terrain.heights[i] =
                    -(l as i32) * 960 + x as i32 * 7 - z as i32 * 11 + (x * z % 17) as i32 * 8;
            }
        }
    }
    for x in 45..59 {
        for z in 45..59 {
            if (x + z) % 3 == 0 {
                let i = terrain.tile(1, x, z);
                terrain.tiles[i].flags = 2;
            }
        }
    }
    let map = terrain.context(1, 3100, 3200).unwrap();
    let mut offset = 0;
    for v in 0..96i32 {
        let bases = BTreeMap::from([(0, bas(v)), (1, bas(v))]);
        let types = (2..=3)
            .map(|k| {
                let mut n = config_types::Npc::empty(k);
                n.bas = k % 2;
                n.walksmoothing = (v + k) % 2 == 0;
                (k, n)
            })
            .collect();
        let inputs = actor::Inputs {
            selection: &c,
            sequences: &seq,
            bases: &bases,
            npcs: &types,
            vars: None,
        };
        let mut players = Players {
            high_indices: vec![3, 1],
            ..Default::default()
        };
        players.players[3] = Some(entity(v, 0, &c));
        players.players[1] = Some(entity(v, 1, &c));
        let mut npcs = Npcs {
            slots: vec![7, 2],
            ..Default::default()
        };
        for k in 2..=3 {
            let mut n = Npc::new([0; 4]);
            n.path = entity(v, k, &c);
            n.type_id = k;
            n.turn_speed = if (v + k) % 5 == 0 { 0 } else { 256 };
            n.face_x = 6307;
            n.face_z = 6501;
            npcs.entities.insert(if k == 2 { 7 } else { 2 }, n);
        }
        let mut rng = AnimationRandom::new(910 + v as u64);
        let mut reads = 0;
        for t in 1..=64 {
            if t == 40 {
                players.players[3] = Some(entity(v, 0, &c));
            }
            for (k, id) in [3, 1].into_iter().enumerate() {
                external(players.players[id].as_mut().unwrap(), t, k as i32, v);
            }
            for (k, id) in [7, 2].into_iter().enumerate() {
                external(
                    &mut npcs.entities.get_mut(&id).unwrap().path,
                    t,
                    k as i32 + 2,
                    v,
                );
            }
            let mut observed = rng.clone();
            actor::tick(
                &mut players,
                &mut npcs,
                &inputs,
                &map,
                Some(&terrain),
                t,
                &mut rng,
            )
            .map_err(|e| anyhow::anyhow!("scenario {v},tick {t}: {e:?}"))?;
            // Count actual random draws from the external RNG state, without
            // assuming which actor transitions ought to have consumed them.
            let mut used = 0;
            while observed.state != rng.state {
                observed.next();
                used += 1;
                anyhow::ensure!(used < 100, "unbounded actor RNG");
            }
            reads += used;
            let mut d = Dump {
                b: vec![],
                labels: vec![],
            };
            d.ints("frame", &[v, t, reads]);
            for id in [3, 1] {
                let e = players.players[id].as_ref().unwrap();
                d.actor(e, e.face_override);
            }
            for id in [7, 2] {
                let n = &npcs.entities[&id];
                d.actor(&n.path, n.face_x);
            }
            for (i, label) in d.labels {
                writeln!(offsets, "{} {v} {t} {label}", offset + i)?;
            }
            offset += d.b.len();
            bytes.write_all(&d.b)?;
        }
    }
    bytes.flush()?;
    offsets.flush()?;
    scratch.finish("actors", &[("rust.bin", "recording")]);
    Ok(())
}
