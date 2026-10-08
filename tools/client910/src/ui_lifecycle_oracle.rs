use crate::{
    session::{self, ActiveBinding},
    ui_changes::{Change, Changes},
    ui_components::{Arg, Component, Interface, Ref, Store},
    ui_components_oracle::{component, int, text},
    ui_hooks::{Executor, Request},
    ui_lifecycle::{self as life, Service, SubRef},
    ui_properties::State,
    ui_resources::{Keys, Source},
};
use std::{cell::RefCell, rc::Rc};
const PACKETS: &[(&str, u8)] = &[
    ("IF_OPENTOP", crate::proto::server::IF_OPENTOP),
    ("IF_OPENSUB", crate::proto::server::IF_OPENSUB),
    (
        "IF_OPENSUB_ACTIVE_LOC",
        crate::proto::server::IF_OPENSUB_ACTIVE_LOC,
    ),
    (
        "IF_OPENSUB_ACTIVE_PLAYER",
        crate::proto::server::IF_OPENSUB_ACTIVE_PLAYER,
    ),
    (
        "IF_OPENSUB_ACTIVE_NPC",
        crate::proto::server::IF_OPENSUB_ACTIVE_NPC,
    ),
    (
        "IF_OPENSUB_ACTIVE_OBJ",
        crate::proto::server::IF_OPENSUB_ACTIVE_OBJ,
    ),
    ("IF_CLOSESUB", crate::proto::server::IF_CLOSESUB),
    ("IF_MOVESUB", crate::proto::server::IF_MOVESUB),
    ("IF_SETTEXT", crate::proto::server::IF_SETTEXT),
    ("IF_SETHIDE", crate::proto::server::IF_SETHIDE),
    ("IF_SETPOSITION", crate::proto::server::IF_SETPOSITION),
    ("IF_SETSCROLLPOS", crate::proto::server::IF_SETSCROLLPOS),
    ("RUNCLIENTSCRIPT", crate::proto::server::RUNCLIENTSCRIPT),
];
const COMMANDS: &[&str] = &[
    "if_opensubclient",
    "if_closesubclient",
    "if_hassub",
    "if_hassubmodal",
    "if_hassuboverlay",
    "if_gettop",
    "if_debug_getopenifcount",
];
struct Missing;
impl Source for Missing {
    fn capacity(&self) -> usize {
        8
    }
    fn ready(&mut self, _: i32) -> anyhow::Result<bool> {
        Ok(false)
    }
    fn group_capacity(&mut self, _: i32) -> anyhow::Result<usize> {
        unreachable!()
    }
    fn file(&mut self, _: i32, _: i32, _: Keys) -> anyhow::Result<Option<Vec<u8>>> {
        unreachable!()
    }
    fn discard(&mut self, _: i32) -> anyhow::Result<()> {
        Ok(())
    }
}
#[derive(Default)]
struct Recorder {
    nodes: Vec<Ref>,
    subs: Vec<SubRef>,
    hooks: Vec<u8>,
    arm: Option<(i32, i32)>,
}
fn packed(c: Option<&Ref>) -> i32 {
    c.map_or(-1, |c| c.borrow().f.parentlayer)
}
fn sid(r: &mut Recorder, n: &SubRef) -> i32 {
    if let Some(i) = r.subs.iter().position(|v| Rc::ptr_eq(v, n)) {
        i as i32
    } else {
        r.subs.push(n.clone());
        r.subs.len() as i32 - 1
    }
}
fn table(o: &mut Vec<u8>, r: &mut Recorder, s: &State) {
    let nodes = s.life.subs.ordered().cloned().collect::<Vec<_>>();
    int(o, nodes.len() as i32);
    for n in nodes {
        int(o, sid(r, &n));
        let n = n.borrow();
        for v in [n.parent, n.id, n.kind] {
            int(o, v);
        }
        match &n.binding {
            None => int(o, 0),
            Some(ActiveBinding::Loc {
                coord, shape, id, ..
            }) => {
                for v in [1, coord.level, coord.x, coord.z, *shape, *id] {
                    int(o, v);
                }
            }
            Some(ActiveBinding::Player { index }) => {
                int(o, 2);
                int(o, *index);
            }
            Some(ActiveBinding::Npc { index }) => {
                int(o, 3);
                int(o, *index);
            }
            Some(ActiveBinding::Obj { coord, id }) => {
                for v in [4, coord.level, coord.x, coord.z, *id] {
                    int(o, v);
                }
            }
        }
    }
}
impl Executor for Recorder {
    fn run(
        &mut self,
        store: &mut Store,
        s: &mut State,
        r: Request,
        limit: usize,
    ) -> anyhow::Result<()> {
        let mut o = vec![];
        int(&mut o, packed(r.component.as_ref()));
        int(&mut o, limit as i32);
        let a = r.args.as_ref().unwrap();
        int(&mut o, a.len() as i32);
        for a in a {
            match a {
                Arg::Int(v) => {
                    int(&mut o, 0);
                    int(&mut o, *v)
                }
                Arg::String(v) => {
                    int(&mut o, 2);
                    text(&mut o, Some(v));
                }
                _ => unreachable!(),
            }
        }
        int(&mut o, s.life.top);
        table(&mut o, self, s);
        self.hooks.extend(o);
        if self
            .arm
            .is_some_and(|(p, _)| p == packed(r.component.as_ref()))
        {
            let (_, p) = self.arm.take().unwrap();
            life::command(store, s, COMMANDS[1], &mut vec![p]).unwrap()?;
        }
        Ok(())
    }
}
fn fixture(r: &mut Recorder) -> (Store, State, Changes) {
    *r = Recorder::default();
    let mut store = Store::with_source(Box::new(Missing));
    let mut state = State::default();
    state.layout.canvas = [800, 600];
    for g in 1..8 {
        let mut a = vec![];
        for n in 0..16 {
            let mut c = Component::default();
            c.f.parentlayer = (g << 16) | n;
            c.f.r#type = if n == 15 { 0 } else { 3 };
            c.f.wsize = 120 + n;
            c.f.hsize = 80 + n;
            c.f.scrollheight = 40;
            c.f.scrolly = 17;
            c.f.xmode = (n % 6) as i8;
            c.f.ymode = ((n + 1) % 6) as i8;
            c.f.xpos = n * 2;
            c.f.ypos = n * 3;
            c.f.invobject = if n % 2 == 0 { -1 } else { 100 };
            c.f.modelobjwidth = if n % 3 == 0 { 64 } else { 0 };
            c.f.hashook = true;
            for h in ["onload", "onsubchange", "ondialogabort", "onresize"] {
                c.hooks
                    .insert(h, vec![Arg::Int(1), Arg::Int(c.f.parentlayer)]);
            }
            let c = Rc::new(RefCell::new(c));
            r.nodes.push(c.clone());
            a.push(Some(c));
        }
        store.interfaces.insert(g, Interface::new(a));
        store.resources.as_mut().unwrap().loaded.insert(g);
    }
    (store, state, Changes::default())
}
fn snapshot(o: &mut Vec<u8>, r: &mut Recorder, store: &mut Store, s: &mut State) {
    int(o, s.life.top);
    int(o, s.life.verify);
    o.push(s.life.verify_changed as u8);
    int(o, packed(s.life.pressed_continue.as_ref()));
    o.extend(s.life.redraw.map(u8::from));
    match s.life.map_flag {
        None => o.push(0),
        Some([x, z]) => {
            o.push(1);
            int(o, x);
            int(o, z);
        }
    }
    table(o, r, s);
    int(o, s.layout.active_masks.len() as i32);
    for (k, v) in &s.layout.active_masks {
        o.extend(k.to_be_bytes());
        int(o, *v);
    }
    int(o, r.hooks.len() as i32);
    o.append(&mut r.hooks);
    int(o, store.updated.len() as i32);
    for c in store.updated.drain(..) {
        int(o, packed(Some(&c)));
    }
    int(o, s.life.services.len() as i32);
    for service in s.life.services.drain(..) {
        match service {
            Service::RemoveMenuOptions(id) => {
                int(o, 1);
                int(o, id);
            }
            Service::ResetAnimations {
                interface_id,
                keys,
                components,
            } => {
                int(o, 0);
                int(o, interface_id);
                o.push(keys.is_some() as u8);
                if let Some(keys) = keys {
                    for k in keys {
                        int(o, k);
                    }
                }
                int(o, components.len() as i32);
                for c in components {
                    int(o, packed(Some(&c)));
                }
            }
        }
    }
    int(o, s.layout.hooks.len() as i32);
    for request in s.layout.hooks.drain(..) {
        let c = request.component.unwrap();
        int(o, packed(Some(&c)));
    }
    int(o, s.layout.actor_layers.len() as i32);
    for ([x, y], h) in s.layout.actor_layers.drain(..) {
        int(o, x);
        int(o, y);
        o.push(h as u8);
    }
    for g in 0..8 {
        o.push(store.resources.as_ref().unwrap().loaded.contains(&g) as u8);
        o.push(store.interfaces.contains_key(&g) as u8);
    }
    for c in &r.nodes {
        component(o, &c.borrow());
        for pair in [&c.borrow().recolour, &c.borrow().retexture] {
            o.push(pair.is_some() as u8);
            if let Some((a, b)) = pair {
                for v in a.iter().chain(b) {
                    o.extend(v.to_be_bytes());
                }
            }
        }
    }
}
#[derive(Clone)]
struct Action([i32; 8], Vec<u8>);
fn action(op: i32, a: i32, b: i32, c: i32) -> Action {
    Action([op, a, b, c, 0, 0, 0, 0], vec![])
}
fn p4(b: &mut Vec<u8>, v: i32, alt: u8) {
    let a = v.to_be_bytes();
    b.extend(match alt {
        0 => a,
        1 => [a[3], a[2], a[1], a[0]],
        2 => [a[2], a[3], a[0], a[1]],
        3 => [a[1], a[0], a[3], a[2]],
        _ => unreachable!(),
    });
}
fn p2(b: &mut Vec<u8>, v: i32, alt: u8) {
    let [h, l] = (v as u16).to_be_bytes();
    b.extend(match alt {
        0 => [h, l],
        1 => [l, h],
        2 => [h, l.wrapping_add(128)],
        3 => [l.wrapping_add(128), h],
        _ => unreachable!(),
    });
}
fn packet(op: i32, p: i32, id: i32, kind: i32, seed: i32) -> Action {
    let k = [
        seed.wrapping_mul(1234567),
        i32::MIN + seed,
        seed ^ 0x13579bdf,
        seed ^ 0x2468ace0,
    ];
    let mut b = vec![];
    let coord = if seed % 2 == 0 { -1 } else { 0x3123abcd };
    match op {
        0 => {
            for (i, a) in [(3, 2), (2, 1), (0, 2), (1, 0)] {
                p4(&mut b, k[i], a);
            }
            b.push(kind as u8);
            p2(&mut b, id, 3);
        }
        1 => {
            p4(&mut b, k[2], 2);
            p4(&mut b, p, 1);
            b.push((kind as u8).wrapping_neg());
            p4(&mut b, k[3], 0);
            p2(&mut b, id, 0);
            p4(&mut b, k[1], 2);
            p4(&mut b, k[0], 2);
        }
        2 => {
            p4(&mut b, p, 1);
            p4(&mut b, coord, 3);
            p2(&mut b, id, 0);
            b.push(128u8.wrapping_sub(kind as u8));
            for (i, a) in [(0, 3), (1, 0), (2, 2), (3, 3)] {
                p4(&mut b, k[i], a);
            }
            b.push((seed & 127) as u8);
            p4(&mut b, k[0], 0);
        }
        3 => {
            p4(&mut b, k[0], 2);
            p4(&mut b, k[2], 3);
            p4(&mut b, p, 2);
            p4(&mut b, k[3], 0);
            b.push(kind as u8);
            p2(&mut b, seed ^ 65535, 0);
            p4(&mut b, k[1], 3);
            p2(&mut b, id, 3);
        }
        4 => {
            p2(&mut b, id, 3);
            for (i, a) in [(0, 1), (1, 3), (3, 3)] {
                p4(&mut b, k[i], a);
            }
            p2(&mut b, seed ^ 65535, 1);
            b.push(kind as u8);
            p4(&mut b, k[2], 0);
            p4(&mut b, p, 1);
        }
        5 => {
            p2(&mut b, id, 0);
            p4(&mut b, k[2], 3);
            p4(&mut b, coord, 2);
            p4(&mut b, p, 1);
            p4(&mut b, k[3], 0);
            p2(&mut b, seed ^ 65535, 1);
            p4(&mut b, k[0], 3);
            b.push((kind as u8).wrapping_add(128));
            p4(&mut b, k[1], 2);
        }
        6 => p4(&mut b, p, 2),
        7 => {
            p4(&mut b, p, 2);
            p4(&mut b, id, 2);
        }
        8 => {
            p4(&mut b, p, 1);
            b.extend(1u8..=255);
            b.push(0);
        }
        9 => {
            p4(&mut b, p, 0);
            b.push((kind as u8).wrapping_neg());
        }
        10 => {
            p2(&mut b, -32768 + seed, 1);
            p2(&mut b, 32767 - seed, 2);
            p4(&mut b, p, 1);
        }
        11 => {
            p4(&mut b, p, 0);
            p2(&mut b, seed ^ 65535, 2);
        }
        12 => {
            b.extend(b"lsz\0");
            p4(&mut b, k[0], 0);
            b.extend(1u8..=255);
            b.push(0);
            p4(&mut b, k[1], 0);
            p4(&mut b, 1, 0);
        }
        _ => unreachable!(),
    };
    Action([1, op, 0, 0, 0, 0, 0, 0], b)
}
#[test]
fn replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-lifecycle");
    let out = scratch.dir().to_path_buf();
    let mut input = vec![];
    let mut output = vec![];
    let mut actions = vec![];
    for round in 0..10 {
        actions.push(action(0, 0, 0, 0));
        actions.push(action(7, i32::MAX, 0, 0));
        actions.push(packet(0, 0, 1, round, round));
        for op in 1..6 {
            actions.push(packet(op, 65536 + op, op + 1, round % 4, round));
        }
        actions.push(packet(1, 65546, 7, 1, round));
        actions.push(packet(1, 65546, 7, 3, round));
        actions.push(packet(1, 65546, 6, 0, round));
        for cmd in 2..7 {
            actions.push(action(2, cmd, 65537, 2));
        }
        actions.push(action(4, 65539, 0, 0));
        actions.push(packet(7, 65538, 65537, 0, round));
        actions.push(packet(7, 65537, 65537, 0, round));
        actions.push(packet(7, 65548, 65540, 0, round));
        for flag in [0, 1, 2, 127, 255] {
            actions.push(packet(9, 65545, 0, flag, round));
            actions.push(action(8, 0, 0, 0));
        }
        for op in [8, 10, 11, 12] {
            actions.push(packet(op, 65551, 0, 0, round));
        }
        actions.push(action(8, 0, 0, 0));
        actions.push(packet(6, 65548, 0, 0, round));
        actions.push(packet(9, -1, 0, 1, round));
        actions.push(action(8, 0, 0, 0));
        actions.push(packet(1, -1, 2, 1, round));
        actions.push(packet(0, 0, 0, 0, round));
        // Closing a child recursively unlinks the outer cursor's cached next node.
        actions.push(action(0, 0, 0, 0));
        actions.push(packet(0, 0, 1, 0, round));
        actions.push(packet(1, 65536, 2, 1, round));
        for (p, id) in [(131072, 3), (196608, 4), (131080, 5)] {
            actions.push(action(2, 0, p, id));
        }
        actions.push(action(5, 2 << 16, -1, 17));
        actions.push(action(5, (2 << 16) + 1, -1, 18));
        actions.push(packet(6, 65536, 0, 0, round));
        // A synchronous subchange hook closes a script-owned sibling.
        actions.push(action(0, 0, 0, 0));
        actions.push(packet(0, 0, 1, 0, round));
        actions.push(action(2, 0, 65538, 2));
        actions.push(action(6, 65536, 65538, 0));
        actions.push(packet(1, 65537, 3, 0, round));
        actions.push(action(2, 1, 65537, 0));
        // Delayed component consumer, including ignored keys and no-redraw branches.
        for kind in [
            3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 17, 20, 21, 22, 23, 0, 16, 255,
        ] {
            for n in 0..3 {
                let p = if kind == 12 { 65551 } else { 65537 + n };
                let payload = if kind == 3 {
                    vec![0xd8, 0x00, 0x00, 0x41]
                } else {
                    vec![]
                };
                actions.push(Action(
                    [
                        3,
                        kind,
                        p,
                        round.wrapping_mul(i32::MAX).wrapping_add(n),
                        -1234,
                        987654,
                        if kind == 17 || kind == 20 { n - 1 } else { 0 },
                        0,
                    ],
                    payload,
                ));
            }
        }
    }
    let names = crate::ui_component_fields::FIELD_NAMES;
    int(&mut input, names.len() as i32);
    for n in names {
        text(&mut input, Some(&n.encode_utf16().collect::<Vec<_>>()));
    }
    int(&mut input, actions.len() as i32);
    let mut r = Recorder::default();
    let (mut store, mut state, mut queue) = fixture(&mut r);
    let mut offsets = String::new();
    for (step, Action(a, b)) in actions.iter().enumerate() {
        for v in a {
            int(&mut input, *v);
        }
        int(&mut input, b.len() as i32);
        input.extend(b);
        let result = (|| -> anyhow::Result<Option<i32>> {
            match a[0] {
                0 => {
                    (store, state, queue) = fixture(&mut r);
                }
                1 => {
                    let opcode = PACKETS[a[1] as usize].1;
                    let event = session::parse_ui_event(opcode, b)?.unwrap();
                    for u in life::packet(&mut store, &mut state, &event, &mut r)? {
                        u.enqueue(&mut queue);
                    }
                }
                2 => {
                    let mut ints = match a[1] {
                        0 | 3 | 4 => vec![a[2], a[3]],
                        1 | 2 => vec![a[2]],
                        _ => vec![],
                    };
                    return life::command(
                        &mut store,
                        &mut state,
                        COMMANDS[a[1] as usize],
                        &mut ints,
                    )
                    .unwrap()
                    .map(|v| {
                        v.map(|v| match v {
                            native910::vm::Value::Int(v) => v,
                            _ => unreachable!(),
                        })
                    });
                }
                3 => {
                    let target = (a[6] as i64).wrapping_shl(32) | (a[2] as i64);
                    let c = Change {
                        key: (a[1] as i64).wrapping_shl(56) | target,
                        ints: [a[3], a[4], a[5]],
                        string: if a[1] == 3 {
                            Some(
                                b.chunks_exact(2)
                                    .map(|b| u16::from_be_bytes([b[0], b[1]]))
                                    .collect(),
                            )
                        } else {
                            None
                        },
                        ..Default::default()
                    };
                    life::apply_change(&mut store, &mut state, &c)?;
                }
                4 => state.life.pressed_continue = store.get(a[1], -1)?,
                5 => {
                    state.layout.active_masks.insert(
                        (a[1] as i64).wrapping_shl(32).wrapping_add(a[2] as i64),
                        a[3],
                    );
                }
                6 => r.arm = Some((a[1], a[2])),
                7 => state.life.verify = a[1],
                8 => {
                    while let Some(c) = queue.poll(|| 0) {
                        life::apply_change(&mut store, &mut state, &c)?;
                    }
                }
                _ => unreachable!(),
            }
            Ok(None)
        })();
        offsets.push_str(&format!("{} {} {:?}\n", output.len(), step, a));
        output.push(result.is_ok() as u8);
        if let Ok(v) = result {
            output.push(v.is_some() as u8);
            if let Some(v) = v {
                int(&mut output, v);
            }
        }
        snapshot(&mut output, &mut r, &mut store, &mut state);
    }
    std::fs::write(out.join("input.bin"), input)?;
    std::fs::write(out.join("rust.bin"), output)?;
    std::fs::write(out.join("rust-offsets.txt"), offsets)?;
    std::fs::write(out.join("count.txt"), actions.len().to_string())?;
    scratch.finish("ui-lifecycle", &[("rust.bin", "recording")]);
    Ok(())
}

#[test]
fn fixed_interface_packets_reject_every_truncation_and_trailer() {
    for op in [0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 11] {
        let Action(_, b) = packet(op, 65536, 2, 1, 910);
        let opcode = PACKETS[op as usize].1;
        assert!(session::parse_ui_event(opcode, &b).unwrap().is_some());
        for len in 0..b.len() {
            assert!(
                session::parse_ui_event(opcode, &b[..len]).is_err(),
                "opcode {opcode}, length {len}"
            );
        }
        let mut extra = b.clone();
        extra.push(0);
        assert!(session::parse_ui_event(opcode, &extra).is_err());
    }
    let Action(_, mut b) = packet(2, 65536, 2, 1, 910);
    b[27] |= 128;
    assert!(session::parse_ui_event(PACKETS[2].1, &b).is_err());
}

#[test]
fn live_varc_frames_reach_delayed_state_and_transmit_hooks() -> anyhow::Result<()> {
    use crate::{
        entity_runtime::bits_pack::Inputs,
        protocol910::varbits::Binding,
        ui_hook_host::{Domains, Runner},
        ui_vars::Value,
    };
    use std::collections::{BTreeMap, HashMap};

    // Interface packets. Integers use g1b_alt3/g4_alt1;
    // strings use g2 + CP1252 gjstr, with u8/u16 framing respectively.
    // Server opcodes >=128 use the two-byte gSmart1or2 opcode header.
    let mut wire = vec![128, crate::proto::server::CLIENT_SETVARC_SMALL, 133, 0, 7];
    wire.extend([128, crate::proto::server::CLIENT_SETVARC_LARGE]);
    wire.extend((-123456i32).to_le_bytes());
    wire.extend([0, 135]); // g2_alt2 -> 7
    wire.extend([
        crate::proto::server::CLIENT_SETVARCSTR_SMALL,
        4,
        0,
        8,
        0x80,
        0,
    ]);
    wire.push(crate::proto::server::CLIENT_SETVARCSTR_LARGE);
    wire.extend(303u16.to_be_bytes());
    wire.extend([0, 8]);
    wire.extend([b'a'; 300]);
    wire.push(0);

    // Both fragmented and coalesced TCP delivery must reach the installed
    // synchronous dispatcher and preserve packet order before the next tick.
    for chunk_size in [1, wire.len()] {
        let mut pending = Vec::new();
        let mut events = Vec::new();
        let mut writes = Vec::new();
        for chunk in wire.chunks(chunk_size) {
            pending.extend_from_slice(chunk);
            while let Some((frame, used)) = crate::net::decode_frame(&pending)? {
                assert!(session::handle_sync_frame(frame, &mut writes, &mut events)?.is_none());
                pending.drain(..used);
            }
        }
        assert!(pending.is_empty() && writes.is_empty());
        assert_eq!(
            events,
            vec![
                session::UiEvent::SetVarc { id: 7, value: -5 },
                session::UiEvent::SetVarc {
                    id: 7,
                    value: -123456
                },
                session::UiEvent::SetVarcString {
                    id: 8,
                    value: vec![0x20ac]
                },
                session::UiEvent::SetVarcString {
                    id: 8,
                    value: vec![b'a' as u16; 300]
                },
            ]
        );

        let mut defs = BTreeMap::new();
        for (id, kind) in [(7, 0), (8, 36)] {
            let mut def = Binding::empty(2, id);
            def.data_type = Some(kind);
            def.lifetime = Some(0);
            defs.insert(id, def);
        }
        let definitions = Inputs {
            definitions: BTreeMap::from([(2, defs)]),
            raw: BTreeMap::new(),
            count: 0,
        };
        let mut variables = crate::ui_vars::State::default();
        variables.client.values.insert(7, Value::Int(42));
        variables.delayed.push_client(1, 7, 1000);
        let (mut store, mut state, _) = fixture(&mut Recorder::default());
        let mut pool = crate::ui_hooks::Pool::default();
        let scripts: HashMap<i32, native910::script::CompiledScript> = HashMap::new();
        let mut engine = crate::ui_hooks_oracle::Strict;
        let mut now = || 1000;
        {
            // This is the packet runner used by Runtime::packet, sharing the
            // same delayed queue that scripts and State::poll consume.
            let mut runner = Runner {
                pool: &mut pool,
                provider: &scripts,
                engine: &mut engine,
                domains: Domains::Plain {
                    changes: &mut variables.delayed,
                    now: &mut now,
                },
                executions: vec![],
                missing: vec![],
            };
            for event in &events {
                runner.packet(&mut store, &mut state, event)?;
            }
        }
        assert_eq!(state.life.verify, 4);
        assert!(state.life.verify_changed);
        variables.poll(&definitions, || 1499)?;
        assert_eq!(variables.client.values[&7], Value::Int(42));
        assert_eq!(variables.varc_transmit.count, 0);
        assert_eq!(
            variables.client.values[&8],
            Value::String(vec![b'a' as u16; 300])
        );
        assert_eq!(variables.string_transmit.count, 1);
        assert_eq!(variables.string_transmit.ids[0], 8);
        variables.poll(&definitions, || 1500)?;
        assert_eq!(variables.client.values[&7], Value::Int(-123456));
        assert_eq!(variables.varc_transmit.count, 1);
        assert_eq!(variables.varc_transmit.ids[0], 7);
        variables.poll(&definitions, || 1501)?;
        assert_eq!(variables.varc_transmit.count, 1);
        assert_eq!(variables.string_transmit.count, 1);
    }
    Ok(())
}

#[test]
fn packet_update_keeps_the_script_delay_and_cached_payload() -> anyhow::Result<()> {
    let mut r = Recorder::default();
    let (mut store, mut state, mut changes) = fixture(&mut r);
    let target = 65536;
    changes.push_client(7, target, 1000);
    changes.cache(7, target).ints = [9, 88, 99];
    let event = session::UiEvent::SetHide {
        packed: target as u32,
        hidden: true,
        flag: 1,
    };
    life::packet(&mut store, &mut state, &event, &mut r)?
        .pop()
        .unwrap()
        .enqueue(&mut changes);
    assert_eq!(changes.cache(7, target).ints, [1, 88, 99]);
    assert!(changes.poll(|| 1499).is_none());
    assert!(!store.get(target as i32, -1)?.unwrap().borrow().f.hide);
    let change = changes.poll(|| 1500).unwrap();
    life::apply_change(&mut store, &mut state, &change)?;
    assert!(store.get(target as i32, -1)?.unwrap().borrow().f.hide);
    assert_eq!(store.updated.len(), 1);
    assert!(changes.poll(|| 1500).is_none());
    Ok(())
}

#[test]
fn interface_visual_packets_reach_component_consumers() -> anyhow::Result<()> {
    // Interface packets enqueue delayed
    // component state. Exercise the same packet -> delayed queue -> apply
    // change path used by Runtime::packet/tick; this is intentionally an
    // integration check rather than a parser-only byte fixture.
    let mut r = Recorder::default();
    let (mut store, mut state, mut changes) = fixture(&mut r);
    let target = 65536;
    state.local_player_uid = 77;
    let object = crate::config::Obj {
        inventory: crate::config::ObjInventory {
            zoom: 1200,
            angles: [11, 22, 33],
            offset: [4, 5],
            ..Default::default()
        },
        id: 321,
        name: "test object".to_string(),
        models: vec![],
        head_models: [[-1; 2]; 2],
        recol_s: vec![],
        recol_d: vec![],
        retex_s: vec![],
        retex_d: vec![],
        ops: std::array::from_fn(|_| None),
        iops: std::array::from_fn(|_| None),
        members: false,
        minimenu_colour: None,
        params: vec![],
        category: -1,
        scattered_drop: false,
    };
    state.objs = Some(Rc::new(crate::config::ObjStore::from_map(
        [(321, object)].into_iter().collect(),
    )));
    life::packet(
        &mut store,
        &mut state,
        &session::UiEvent::SetInterfaceHttpImage {
            packed: target as u32,
            image: 456,
        },
        &mut r,
    )?;
    let mut apply = |event: session::UiEvent| -> anyhow::Result<()> {
        for update in life::packet(&mut store, &mut state, &event, &mut r)? {
            update.enqueue(&mut changes);
        }
        let change = changes
            .poll(|| 1000)
            .ok_or_else(|| anyhow::anyhow!("missing delayed component update"))?;
        life::apply_change(&mut store, &mut state, &change)?;
        while let Some(change) = changes.poll(|| 1000) {
            life::apply_change(&mut store, &mut state, &change)?;
        }
        Ok(())
    };
    apply(session::UiEvent::SetInterfaceColour {
        packed: target as u32,
        colour: 0x1234,
    })?;
    apply(session::UiEvent::SetInterfaceModel {
        packed: target as u32,
        model_kind: 1,
        model: 1234,
        model_name_hash: -1,
        local_player: false,
    })?;
    apply(session::UiEvent::SetInterfaceModel {
        packed: target as u32,
        model_kind: 5,
        model: 0,
        model_name_hash: 0,
        local_player: true,
    })?;
    apply(session::UiEvent::SetInterfaceAngle {
        packed: target as u32,
        x: 101,
        y: 202,
        zoom: 303,
    })?;
    apply(session::UiEvent::SetInterfaceGraphic {
        packed: target as u32,
        graphic: 77,
    })?;
    apply(session::UiEvent::SetInterfaceTextAntiMacro {
        packed: target as u32,
        enabled: true,
    })?;
    apply(session::UiEvent::SetInterfaceTextFont {
        packed: target as u32,
        font: 44,
    })?;
    apply(session::UiEvent::SetInterfaceClickMask {
        packed: target as u32,
        enabled: true,
    })?;
    apply(session::UiEvent::SetInterfaceRecolour {
        packed: target as u32,
        index: 2,
        source: 12,
        destination: 34,
    })?;
    apply(session::UiEvent::SetInterfaceRetexture {
        packed: target as u32,
        index: 3,
        source: 56,
        destination: 78,
    })?;
    apply(session::UiEvent::SetInterfaceObject {
        packed: target as u32,
        object: 321,
        count: 654321,
    })?;
    apply(session::UiEvent::SetMapFlag { x: 12, z: 34 })?;
    let component = store
        .get(target, -1)?
        .ok_or_else(|| anyhow::anyhow!("component target"))?;
    let component = component.borrow();
    assert_eq!(
        component.f.colour,
        ((0x1234 & 31) << 3) + (((0x1234 >> 10) & 31) << 19) + (((0x1234 >> 5) & 31) << 11)
    );
    assert_eq!(component.f.modelkind, 5);
    assert_eq!(component.f.model, 77);
    assert_eq!(component.f.modelNameHash, 0);
    assert_eq!([component.f.invobject, component.f.invcount], [321, 654321]);
    assert_eq!(component.f.httpImageId, 456);
    assert_eq!(state.life.map_flag, Some([12, 34]));
    assert_eq!(
        [
            component.f.modelangle_x,
            component.f.modelangle_y,
            component.f.modelzoom
        ],
        [11, 22, 600]
    );
    assert_eq!(
        [
            component.f.modelxof,
            component.f.modelyof,
            component.f.modelangle_z
        ],
        [4, 5, 33]
    );
    assert_eq!(component.f.graphic, 77);
    assert!(component.f.textantimacro);
    assert_eq!(component.f.textfont, 44);
    assert!(component.f.clickmask);
    assert_eq!(component.recolour.as_ref().unwrap().0[2], 12);
    assert_eq!(component.recolour.as_ref().unwrap().1[2], 34);
    assert_eq!(component.retexture.as_ref().unwrap().0[3], 56);
    assert_eq!(component.retexture.as_ref().unwrap().1[3], 78);
    // Thirteen packets, of which the map flag does not count as a change.
    assert_eq!(state.life.verify, 12);
    Ok(())
}

#[test]
fn packet_runner_routes_script_lifecycle_and_shared_queue() -> anyhow::Result<()> {
    use crate::ui_hook_host::{Domains, Runner};
    use native910::script::{CompiledScript, Counts, Instruction, Operand};
    let mut r = Recorder::default();
    let (mut store, mut state, mut changes) = fixture(&mut r);
    let code = [
        ("push_constant_int", Operand::Int(65536)),
        ("push_constant_int", Operand::Int(2)),
        ("if_opensubclient", Operand::Byte(0)),
        ("push_constant_int", Operand::Int(65536)),
        ("if_hassub", Operand::Byte(0)),
        ("pop_int_local", Operand::Local(0)),
        ("push_constant_int", Operand::Int(65536)),
        ("push_constant_int", Operand::Int(2)),
        ("if_hassuboverlay", Operand::Byte(0)),
        ("pop_int_local", Operand::Local(1)),
        ("push_constant_int", Operand::Int(65536)),
        ("if_closesubclient", Operand::Byte(0)),
        ("push_constant_int", Operand::Int(65536)),
        ("if_hassub", Operand::Byte(0)),
        ("pop_int_local", Operand::Local(2)),
        ("return", Operand::Byte(0)),
    ]
    .into_iter()
    .map(|(command, operand)| Instruction {
        opcode: 0,
        command: command.into(),
        operand,
    })
    .collect();
    let scripts = std::collections::HashMap::from([(
        42,
        CompiledScript {
            name: None,
            locals: Counts {
                int: 3,
                ..Default::default()
            },
            args: Counts::default(),
            code,
        },
    )]);
    let mut pool = crate::ui_hooks::Pool::default();
    let mut engine = crate::ui_hooks_oracle::Strict;
    let mut now = || 1000;
    changes.push_client(7, 65536, 1000);
    {
        let mut runner = Runner {
            pool: &mut pool,
            provider: &scripts,
            engine: &mut engine,
            domains: Domains::Plain {
                changes: &mut changes,
                now: &mut now,
            },
            executions: vec![],
            missing: vec![],
        };
        runner.packet(
            &mut store,
            &mut state,
            &session::UiEvent::RunScript(session::RunClientScript {
                script_id: 42,
                args: vec![],
            }),
        )?;
        assert_eq!(
            runner.executions.len(),
            1,
            "script-owned opens must not execute onloads"
        );
        assert!(
            runner.executions[0].result.is_ok(),
            "{:?}",
            runner.executions[0].result
        );
        runner.packet(
            &mut store,
            &mut state,
            &session::UiEvent::SetHide {
                packed: 65536,
                hidden: true,
                flag: 1,
            },
        )?;
    }
    assert_eq!(pool.contexts[0].locals.ints, [1, 1, 0]);
    assert_eq!(state.life.verify, 2);
    assert!(state.life.subs.get(65536).is_none());
    assert!(changes.poll(|| 1499).is_none());
    life::apply_change(&mut store, &mut state, &changes.poll(|| 1500).unwrap())?;
    assert!(store.get(65536, -1)?.unwrap().borrow().f.hide);
    Ok(())
}
