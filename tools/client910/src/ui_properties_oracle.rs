use crate::ui_components_oracle::{int, text};
use crate::{
    cache::Pack,
    ui_changes::Changes,
    ui_components::{self as c, Active, Arg, Component, Store},
    ui_properties::{self as p, State},
};
use native910::{
    opcode::OpcodeBook,
    script::{CompiledScript, Counts, Instruction, Operand},
    vm::{Session, Vm},
};
use std::{cell::RefCell, rc::Rc};
fn string(o: &mut Vec<u8>, v: &str) {
    text(o, Some(&v.encode_utf16().collect::<Vec<_>>()));
}
fn ints(o: &mut Vec<u8>, v: &[i32]) {
    int(o, v.len() as i32);
    for v in v {
        int(o, *v);
    }
}
fn strings(o: &mut Vec<u8>, v: &[Option<String>]) {
    int(o, v.len() as i32);
    for v in v {
        text(o, v.as_deref().map(native910::jstr::units).as_deref());
    }
}
fn longs(o: &mut Vec<u8>, v: &[i64]) {
    int(o, v.len() as i32);
    for v in v {
        o.extend(v.to_be_bytes());
    }
}
fn node(packed: i32, kind: i32) -> c::Ref {
    let mut c = Component::default();
    c.f.parentlayer = packed;
    c.f.r#type = kind;
    Rc::new(RefCell::new(c))
}
fn fixture(scenario: i32) -> (Store, State, Vec<c::Ref>, [Active; 2]) {
    let mut store = Store::default();
    let mut state = State::default();
    state.layout.canvas = [800 + scenario * 31, 600 - scenario * 7];
    state.layout.debug_bounds = scenario & 1 != 0;
    state.debug_visible = [scenario & 2 != 0, scenario & 4 != 0];
    let root = node(65536, if scenario & 1 == 0 { 0 } else { 3 });
    let child = node(65537, 4);
    let other = node(65538, 5);
    let sub = node(131072, 3);
    {
        let mut c = root.borrow_mut();
        let f = &mut c.f;
        f.clientcode = 1337;
        f.xpos = 11;
        f.ypos = 13;
        f.wsize = 320;
        f.hsize = 240;
        f.width = 300;
        f.height = 200;
        f.aspectwidth = 4;
        f.aspectheight = 3;
        f.x = 7;
        f.y = 9;
        f.scrollwidth = 400;
        f.scrollheight = 300;
        f.scrollx = 23;
        f.scrolly = 37;
        c.default_active[0] = 2048;
        c.hooks.insert("onresize", vec![Arg::Int(42), Arg::Int(7)]);
        c.f.hashook = true;
    }
    {
        let mut c = child.borrow_mut();
        let f = &mut c.f;
        f.layer = 65536;
        f.clientcode = 1405;
        f.widthSizeMode = 1;
        f.heightSizeMode = 2;
        f.wsize = 20;
        f.hsize = 8192;
        f.xmode = 1;
        f.ymode = 2;
        f.xpos = 3;
        f.ypos = 4;
        f.width = 99;
        f.height = 77;
    }
    {
        let mut c = sub.borrow_mut();
        let f = &mut c.f;
        f.widthSizeMode = 2;
        f.heightSizeMode = 2;
        f.wsize = 8192;
        f.hsize = 16384;
        f.xmode = 3;
        f.xpos = 4096;
    }
    let main = c::Interface::new(vec![
        Some(root.clone()),
        Some(child.clone()),
        Some(other.clone()),
    ]);
    main.borrow_mut().transient = scenario % 3 == 0;
    store.interfaces.insert(1, main.clone());
    store
        .interfaces
        .insert(2, c::Interface::new(vec![Some(sub.clone())]));
    let mut act = Active::default();
    store.create(&mut act, 65536, 3, 0).unwrap();
    let dynamic = act.component.clone().unwrap();
    root.borrow()
        .children
        .as_ref()
        .unwrap()
        .borrow_mut()
        .resize(2, None);
    {
        let mut c = dynamic.borrow_mut();
        c.f.wsize = 16;
        c.f.hsize = 20;
        c.f.width = 16;
        c.f.height = 20;
    }
    state.layout.subs.push((65536, 2));
    if scenario & 2 != 0 {
        state.layout.active_masks.insert((65536i64 << 32) - 1, 0);
    }
    store.updated.clear();
    (
        store,
        state,
        vec![root.clone(), child, other, sub, dynamic],
        [
            Active {
                interface: Some(main),
                component: Some(root),
            },
            act,
        ],
    )
}
fn id(nodes: &[c::Ref], c: Option<&c::Ref>) -> i32 {
    c.map_or(-1, |c| {
        nodes.iter().position(|v| Rc::ptr_eq(c, v)).unwrap() as i32
    })
}
fn snapshot(
    o: &mut Vec<u8>,
    store: &mut Store,
    state: &mut State,
    changes: &Changes,
    nodes: &[c::Ref],
) {
    for c in nodes {
        let c = c.borrow();
        crate::ui_components_oracle::component(o, &c);
        int(o, id(nodes, c.draggable.as_ref()));
        for pair in [&c.recolour, &c.retexture] {
            o.push(pair.is_some() as u8);
            if let Some((a, b)) = pair {
                for v in a.iter().chain(b) {
                    o.extend(v.to_be_bytes());
                }
            }
        }
    }
    int(o, store.updated.len() as i32);
    for c in store.updated.drain(..) {
        int(o, id(nodes, Some(&c)));
    }
    crate::ui_vars_oracle::changes_state(o, changes);
    int(o, id(nodes, state.layout.viewport.as_ref()));
    int(o, state.layout.hooks.len() as i32);
    for request in state.layout.hooks.drain(..) {
        let c = request.component.unwrap();
        let h = request.args.unwrap();
        int(o, id(nodes, Some(&c)));
        int(o, h.len() as i32);
        for a in h {
            match a {
                Arg::Int(v) => {
                    int(o, 0);
                    int(o, v)
                }
                _ => panic!("resize fixture"),
            }
        }
    }
    int(o, state.layout.actor_layers.len() as i32);
    for (d, h) in state.layout.actor_layers.drain(..) {
        for v in d {
            int(o, v);
        }
        o.push(h as u8);
    }
}
/// Commands whose handler reads the local player or the active clan channel;
/// the recorded harness has no such state, so they are not part of this corpus.
const NEEDS_CLIENT_STATE: [&str; 6] = [
    "cc_setlinkactiveclanchannel",
    "cc_setplayerhead_self",
    "cc_setplayermodel_self",
    "if_setlinkactiveclanchannel",
    "if_setplayerhead_self",
    "if_setplayermodel_self",
];
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-properties");
    let out = scratch.dir().to_path_buf();
    let root = rs910_core::test_support::repo_root();
    let pack = Pack::open(root.join("server/data/pack"));
    let book = OpcodeBook::embedded()?;
    let objects = Rc::new(crate::config::ObjStore::load(&pack)?);
    let mut definitions = State::default();
    definitions.load_params(&pack)?;
    let int_id = *definitions
        .params
        .iter()
        .find(|(_, p)| !p.string)
        .unwrap()
        .0;
    let string_id = *definitions.params.iter().find(|(_, p)| p.string).unwrap().0;
    let mut input = vec![];
    let mut output = vec![];
    let names = crate::ui_component_fields::FIELD_NAMES;
    int(&mut input, names.len() as i32);
    for n in names {
        string(&mut input, n);
    }
    let raw = pack.read_group("config", 11)?;
    int(&mut input, raw.len() as i32);
    for (id, bytes) in raw {
        int(&mut input, id as i32);
        int(&mut input, bytes.len() as i32);
        input.extend(bytes);
        let p = &definitions.params[&(id as i32)];
        output.push(p.string as u8);
        int(&mut output, p.integer);
        text(&mut output, p.text.as_deref());
    }
    let mut commands = vec![];
    for suffix in p::SETTERS
        .iter()
        .map(|x| x.0)
        .chain(p::GETTERS.iter().copied())
        .chain(p::HOOKS.iter().map(|x| x.0))
    {
        for prefix in ["cc_", "if_"] {
            let command = format!("{prefix}{suffix}");
            if !NEEDS_CLIENT_STATE.contains(&command.as_str())
                && book.opcode_for(&command).is_ok_and(|id| id < 1432)
            {
                commands.push(command);
            }
        }
    }
    commands.sort();
    commands.dedup();
    std::fs::write(out.join("commands.txt"), commands.join("\n") + "\n")?;
    let command_count = commands.len();
    // Query retained values again after the writes and hook mutations.
    commands.extend(
        commands
            .clone()
            .into_iter()
            .filter(|c| p::GETTERS.contains(&&c[3..])),
    );
    let count = commands.len() * 6 * 5;
    int(&mut input, count as i32);
    let mut engine = crate::iface::VmState::new();
    let mut case = 0;
    let mut offsets = String::new();
    for scenario in 0..6 {
        let (mut store, mut state, nodes, mut active) = fixture(scenario);
        state.params = definitions.params.clone();
        state.objs = Some(objects.clone());
        let mut changes = Changes::default();
        for command in &commands {
            for variant in 0..5 {
                let suffix = &command[3..];
                let secondary = variant & 1 != 0;
                let target = if variant % 3 == 0 { 65537 } else { 65536 };
                let values = [0, 1, -1, i32::MIN, i32::MAX];
                let mut i = vec![];
                let mut s = vec![];
                let mut l = vec![];
                if let Some((_, ni, ns)) = p::SETTERS.iter().find(|x| x.0 == suffix) {
                    i = vec![values[variant]; *ni];
                    s = vec![
                        if variant == 0 {
                            String::new()
                        } else {
                            "value😀".into()
                        };
                        *ns
                    ];
                    match suffix {
                        value if value.starts_with("setobject") => {
                            i = vec![[995, 1205, -1, 4152, 14484][variant], values[variant]];
                        }
                        "setopchar" | "setopkeyrate" | "setopkeyignoreheld" => {
                            i[0] = [0, 1, 10, 11, -1][variant];
                        }
                        "setopkey" => {
                            i = vec![[0, 1, 10, 11, -1][variant]];
                            if command.starts_with("if_") {
                                i.extend([values[variant], 37]);
                            } else {
                                for key in 0..5 {
                                    i.extend([
                                        if key < variant { 128 + key as i32 } else { -1 },
                                        37 + key as i32,
                                    ]);
                                }
                            }
                        }

                        "setposition" => {
                            i = vec![values[variant], 17, variant as i32 - 1, 6 - variant as i32]
                        }
                        "setsize" => {
                            i = vec![values[variant], 37, variant as i32, 4 - variant as i32]
                        }
                        "setaspect" => i = vec![values[variant], if variant == 0 { 0 } else { 3 }],
                        "setdraggable" => {
                            i = if variant == 0 {
                                vec![-1, -1]
                            } else if variant == 4 {
                                vec![65536, 0]
                            } else {
                                vec![target, -1]
                            }
                        }
                        "setparam" | "setparam_int" => {
                            i = vec![
                                int_id,
                                if variant == 0 {
                                    definitions.params[&int_id].integer
                                } else {
                                    values[variant]
                                },
                            ]
                        }
                        "setparam_string" => {
                            i = vec![string_id];
                            if variant == 0 {
                                s = vec![String::from_utf16(
                                    definitions.params[&string_id]
                                        .text
                                        .as_deref()
                                        .unwrap_or(&[]),
                                )?];
                            }
                        }
                        _ => {}
                    }
                } else if p::HOOKS.iter().any(|x| x.0 == suffix) {
                    let hook = if variant == 2 { -1 } else { 1234 };
                    match variant {
                        0 | 2 => {
                            i = vec![hook, 7, 11, 22, 2];
                            s = vec!["hook".into(), "islY".into()];
                            l = vec![i64::MIN + 42];
                        }
                        1 => {
                            i = vec![hook, 97, 0];
                            s = vec!["Y".into()];
                        }
                        3 => {
                            i = vec![hook, 55, -1];
                            s = vec!["arg".into(), "sY".into()];
                        }
                        _ => {
                            i = vec![hook, 8, 9, 10, 0];
                            s = vec!["😀Y".into()];
                        }
                    }
                } else if suffix == "getop" {
                    i = vec![values[variant]];
                } else if suffix == "param" {
                    i = vec![if variant & 1 == 0 { int_id } else { string_id }];
                }
                if command.starts_with("if_") {
                    i.push(target);
                }
                int(&mut input, scenario);
                string(&mut input, command);
                input.push(secondary as u8);
                input.extend((1000 + case as i64).to_be_bytes());
                ints(&mut input, &i);
                strings(&mut input, &s.iter().cloned().map(Some).collect::<Vec<_>>());
                longs(&mut input, &l);
                let mut code = vec![];
                for v in &i {
                    code.push(Instruction {
                        opcode: 0,
                        command: "push_constant_int".into(),
                        operand: Operand::Int(*v),
                    });
                }
                for v in &s {
                    code.push(Instruction {
                        opcode: 0,
                        command: "push_constant_string".into(),
                        operand: Operand::Str(v.clone()),
                    });
                }
                for v in &l {
                    code.push(Instruction {
                        opcode: 0,
                        command: "push_constant_string".into(),
                        operand: Operand::Long(*v),
                    });
                }
                code.push(Instruction {
                    opcode: 0,
                    command: command.clone(),
                    operand: Operand::Byte(secondary as u8),
                });
                code.push(Instruction {
                    opcode: 0,
                    command: "return".into(),
                    operand: Operand::Byte(0),
                });
                let script = CompiledScript {
                    name: Some("properties".into()),
                    locals: Counts::default(),
                    args: Counts::default(),
                    code,
                };
                let mut session = Session::new(&script, &[])?;
                let mut reads = 0;
                let result = {
                    let mut now = || {
                        reads += 1;
                        1000 + case as i64
                    };
                    let properties = p::Context {
                        nested_count: 0,
                        state: &mut state,
                        changes: &mut changes,
                        now: &mut now,
                    };
                    let mut host = c::ScriptHost {
                        engine: &mut engine,
                        store: &mut store,
                        active: &mut active,
                        properties: Some(properties),
                    };
                    let mut vm = Vm::new(&mut host, &());
                    loop {
                        match vm.step(&mut session) {
                            Ok(true) => break Ok(()),
                            Ok(false) => {}
                            Err(e) => break Err(e),
                        }
                    }
                };
                offsets.push_str(&format!(
                    "{} {case} {scenario} {command} {variant}\n",
                    output.len()
                ));
                output.push(result.is_ok() as u8);
                if result.is_ok() {
                    let snap = session.snapshot();
                    ints(&mut output, &snap.ints);
                    strings(&mut output, &snap.strings);
                    longs(&mut output, &snap.longs);
                }
                int(&mut output, reads);
                snapshot(&mut output, &mut store, &mut state, &changes, &nodes);
                case += 1;
            }
        }
    }
    std::fs::write(out.join("rust-offsets.txt"), offsets)?;
    std::fs::write(out.join("input.bin"), input)?;
    std::fs::write(out.join("rust.bin"), output)?;
    std::fs::write(
        out.join("counts.txt"),
        format!("{} {} {}\n", definitions.params.len(), command_count, case),
    )?;
    eprintln!(
        "Properties: {} commands, {case} cases, {} params",
        command_count,
        definitions.params.len()
    );
    scratch.finish("ui-properties", &[("rust.bin", "recording")]);
    Ok(())
}

/// The layout methods (positions, alignment, interface open, redraw) against
/// the frozen recording of the original client's layout code.
#[test]
fn layout() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-properties-layout");
    layout_replay(scratch.dir())?;
    scratch.finish("ui-properties", &[("rust-layout.bin", "layout")]);
    Ok(())
}

// The layout reference calls Client's actual methods directly. The property
// stream above separately proves invocation through the VM command boundary.
fn layout_replay(out: &std::path::Path) -> anyhow::Result<()> {
    let mut cases = vec![];
    let modes = [-1, 0, 1, 2, 3, 4, 5];
    let values = [0, 1, -1, i32::MIN, i32::MAX, 16384, 37];
    for scenario in 0..6 {
        for target in 0..5 {
            for (variant, v) in values.iter().copied().enumerate() {
                for op in 0..4 {
                    for wm in modes {
                        for hm in modes {
                            if op != 0 && (wm != modes[variant] || hm != modes[6 - variant]) {
                                continue;
                            }
                            cases.push([
                                op,
                                scenario,
                                target,
                                if variant < 3 { 800 } else { v },
                                if variant < 3 { 600 } else { v.wrapping_neg() },
                                wm,
                                hm,
                                modes[variant],
                                modes[6 - variant],
                                v,
                                v.wrapping_add(1),
                                v,
                                v.wrapping_neg(),
                                i32::MIN,
                                77,
                                if variant == 6 { -1 } else { v },
                                if variant == 6 { 1 } else { v.wrapping_add(1) },
                                v,
                                v.wrapping_add(1),
                                1,
                            ]);
                        }
                    }
                }
            }
        }
    }
    let mut input = vec![];
    let mut output = vec![];
    let mut offsets = String::new();
    let names = crate::ui_component_fields::FIELD_NAMES;
    int(&mut input, names.len() as i32);
    for n in names {
        string(&mut input, n);
    }
    int(&mut input, cases.len() as i32);
    for (index, a) in cases.iter().enumerate() {
        for v in a {
            int(&mut input, *v);
        }
        let (mut store, mut state, nodes, _) = fixture(a[1]);
        let changes = Changes::default();
        let c = &nodes[a[2] as usize];
        {
            let mut c = c.borrow_mut();
            let f = &mut c.f;
            f.widthSizeMode = a[5] as i8;
            f.heightSizeMode = a[6] as i8;
            f.xmode = a[7] as i8;
            f.ymode = a[8] as i8;
            f.wsize = a[9];
            f.hsize = a[10];
            f.xpos = a[11];
            f.ypos = a[12];
            f.width = a[13];
            f.height = a[14];
            f.aspectwidth = a[15];
            f.aspectheight = a[16];
            f.scrollwidth = a[17];
            f.scrollheight = a[18];
        }
        let main = store.interfaces[&1].clone();
        let itf = store.interfaces[&(c.borrow().f.parentlayer >> 16)].clone();
        let hooks = a[19] != 0;
        let result = match a[0] {
            0 => state
                .layout
                .size(c, [a[3], a[4]], hooks)
                .map(|()| state.layout.position(c, [a[3], a[4]])),
            1 => state.layout.align(&mut store, &itf, c),
            2 => state.layout.interface(&mut store, 1, [a[3], a[4]], hooks),
            3 => {
                state.layout.viewport = Some(nodes[0].clone());
                state.layout.redraw(&mut store, &main, &nodes[0], hooks)
            }
            _ => unreachable!(),
        };
        offsets.push_str(&format!("{} {index} {a:?}\n", output.len()));
        output.push(result.is_ok() as u8);
        snapshot(&mut output, &mut store, &mut state, &changes, &nodes);
    }
    std::fs::write(out.join("layout-input.bin"), input)?;
    std::fs::write(out.join("rust-layout.bin"), output)?;
    std::fs::write(out.join("rust-layout-offsets.txt"), offsets)?;
    std::fs::write(out.join("layout-count.txt"), cases.len().to_string())?;
    eprintln!("Layout: {} cases", cases.len());
    Ok(())
}
