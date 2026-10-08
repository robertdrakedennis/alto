use crate::ui_component_fields_snapshot::FieldsSnapshot;
use crate::{
    cache::Pack,
    ui_components::{self as c, Arg, Component},
};
use std::{cell::RefCell, rc::Rc};
pub fn int(o: &mut Vec<u8>, v: i32) {
    o.extend(v.to_be_bytes());
}
pub fn text(o: &mut Vec<u8>, v: Option<&[u16]>) {
    int(o, v.map_or(-1, |v| v.len() as i32));
    if let Some(v) = v {
        for c in v {
            o.extend(c.to_be_bytes());
        }
    }
}
fn string(o: &mut Vec<u8>, s: &str) {
    text(o, Some(&s.encode_utf16().collect::<Vec<_>>()));
}
fn arg(o: &mut Vec<u8>, v: &Arg) {
    match v {
        Arg::Int(v) => {
            int(o, 0);
            int(o, *v)
        }
        Arg::Long(v) => {
            int(o, 1);
            o.extend(v.to_be_bytes());
        }
        Arg::String(v) => {
            int(o, 2);
            text(o, Some(v));
        }
        Arg::Null => int(o, 3),
    }
}
fn ints(o: &mut Vec<u8>, v: Option<&[i32]>) {
    int(o, v.map_or(-1, |v| v.len() as i32));
    if let Some(v) = v {
        for n in v {
            int(o, *n);
        }
    }
}
fn keys(o: &mut Vec<u8>, v: Option<&[Option<Vec<i8>>]>) {
    int(o, v.map_or(-1, |v| v.len() as i32));
    if let Some(v) = v {
        for n in v {
            int(o, n.as_ref().map_or(-1, |v| v.len() as i32));
            if let Some(n) = n {
                o.extend(n.iter().map(|v| *v as u8));
            }
        }
    }
}
pub(crate) fn component(o: &mut Vec<u8>, c: &Component) {
    c.f.snapshot(o);
    for v in c.default_active {
        int(o, v);
    }
    int(o, c.ops.as_ref().map_or(-1, |v| v.len() as i32));
    if let Some(v) = &c.ops {
        for s in v {
            text(o, s.as_deref());
        }
    }
    ints(o, c.opname.as_deref());
    keys(o, c.keys.as_deref());
    keys(o, c.key_mods.as_deref());
    for v in [&c.key_delays, &c.key_rates, &c.key_chars, &c.key_next_fire] {
        ints(o, v.as_deref());
    }
    int(o, c.hooks.len() as i32);
    for (name, h) in &c.hooks {
        string(o, name);
        int(o, h.len() as i32);
        for a in h {
            arg(o, a);
        }
    }
    int(o, c.transmits.len() as i32);
    for (name, v) in &c.transmits {
        string(o, name);
        ints(o, Some(v));
    }
    o.push(c.params.is_some() as u8);
    let mut params = c.params.clone().unwrap_or_default();
    params.sort_by_key(|v| v.0);
    int(o, params.len() as i32);
    for (id, v) in params {
        o.extend(id.to_be_bytes());
        arg(o, &v);
    }
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn replay() -> anyhow::Result<()> {
    let root = rs910_core::test_support::repo_root();
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-components");
    let out = scratch.dir().to_path_buf();
    let pack = Pack::open(root.join("server/data/pack"));
    let mut input = vec![];
    let mut output = vec![];
    let names = crate::ui_component_fields::FIELD_NAMES;
    int(&mut input, names.len() as i32);
    for name in names {
        string(&mut input, name);
    }
    // Constructor defaults have a recorded snapshot too.
    component(&mut output, &Component::default());
    let mut count = 0_i32;
    let count_pos = input.len();
    int(&mut input, 0);
    for group in pack.read_archive_index("interfaces")?.group_id {
        for (file, bytes) in pack.read_group("interfaces", group)? {
            let packed = ((group << 16) | file) as i32;
            int(&mut input, packed);
            int(&mut input, bytes.len() as i32);
            input.extend(&bytes);
            let c = Component::decode(packed, &bytes)?;
            component(&mut output, &c);
            count += 1;
        }
    }
    input[count_pos..count_pos + 4].copy_from_slice(&count.to_be_bytes());
    let mut c = Component::default();
    let mut params = vec![
        (0, 1, Arg::Int(-99)),
        (1, 1, Arg::Null),
        (2, 1, Arg::Int(41)),
        (3, 1, Arg::String(vec![65])),
        (4, 1, Arg::Null),
        (3, 1, Arg::Null),
        (1, 1, Arg::Null),
        (4, 1, Arg::Null),
    ];
    let mut rng = 910u64;
    for i in 0..1000 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let op = ((rng >> 16) % 6) as i32;
        let key = (((rng >> 32) % 8) * 16) as i32 - 1;
        let value = if matches!(op, 1 | 3) {
            if i % 5 == 0 {
                Arg::Null
            } else {
                Arg::String(vec![i as u16, 0xd800])
            }
        } else {
            Arg::Int(rng as i32)
        };
        params.push((op, key, value));
    }
    int(&mut input, params.len() as i32);
    for (op, id, v) in params {
        int(&mut input, op);
        int(&mut input, id);
        arg(&mut input, &v);
        let result = match op {
            0 => c
                .param_int(
                    id,
                    match v {
                        Arg::Int(v) => v,
                        _ => 0,
                    },
                )
                .map(Arg::Int),
            1 => c
                .param_string(
                    id,
                    match &v {
                        Arg::String(v) => Some(v),
                        _ => None,
                    },
                )
                .map(|v| v.map_or(Arg::Null, Arg::String)),
            2 => c
                .set_param_int(
                    id,
                    match v {
                        Arg::Int(v) => v,
                        _ => 0,
                    },
                )
                .map(|_| Arg::Null),
            3 => c
                .set_param_string(
                    id,
                    match &v {
                        Arg::String(v) => Some(v.clone()),
                        _ => None,
                    },
                )
                .map(|_| Arg::Null),
            4 => {
                c.remove_param(id);
                Ok(Arg::Null)
            }
            5 => {
                c.params.get_or_insert_with(Vec::new).push((id as i64, v));
                Ok(Arg::Null)
            }
            _ => unreachable!(),
        };
        output.push(result.is_ok() as u8);
        if let Ok(v) = result {
            arg(&mut output, &v);
        }
        component(&mut output, &c);
    }
    std::fs::write(out.join("input.bin"), input)?;
    std::fs::write(out.join("rust.bin"), output)?;
    std::fs::write(out.join("count.txt"), format!("{count}\n"))?;
    graph_replay(&out)?;
    eprintln!("Decoded {count} cached components and replayed 1008 parameter actions");
    scratch.finish(
        "ui-components",
        &[("rust.bin", "recording"), ("rust-graph.bin", "graph")],
    );
    Ok(())
}
fn node_id(c: &Option<c::Ref>, ids: &mut Vec<c::Ref>) -> i32 {
    let Some(c) = c else { return -1 };
    if let Some(at) = ids.iter().position(|v| Rc::ptr_eq(v, c)) {
        return at as i32;
    }
    ids.push(c.clone());
    ids.len() as i32 - 1
}
fn array(o: &mut Vec<u8>, a: &Option<c::Array>, ids: &mut Vec<c::Ref>) {
    int(o, a.as_ref().map_or(-1, |a| a.borrow().len() as i32));
    if let Some(a) = a {
        for n in &*a.borrow() {
            int(o, node_id(n, ids));
        }
    }
}
fn graph_replay(out: &std::path::Path) -> anyhow::Result<()> {
    let mut store = c::Store::default();
    let mut ids = vec![];
    for group in [1, 2] {
        let mut nodes = vec![];
        for index in 0..4 {
            let mut c = Component::default();
            c.f.parentlayer = (group << 16) | index;
            let n = Rc::new(RefCell::new(c));
            ids.push(n.clone());
            nodes.push(Some(n));
        }
        store.interfaces.insert(group, c::Interface::new(nodes));
    }
    let mut active = [c::Active::default(), c::Active::default()];
    let mut engine = crate::iface::VmState::new();
    // op, secondary, packed, type/child, child. Deliberate failures test the
    // mutation prefix, stale references and aliased versus detached arrays.
    let mut steps = vec![
        [0, 0, 65536, 3, 0],
        [0, 0, 65536, 4, 1],
        [0, 1, 65536, 5, 2],
        [4, 0, 0, 0, 0],
        [1, 0, 65536, 0, 0],
        [2, 0, 0, 0, 0],
        [0, 0, 65536, 6, 1],
        [1, 1, 65536, -1, 0],
        [4, 1, 0, 1, 0],
        [3, 0, 65536, 0, 0],
        [4, 1, 0, 0, 0],
        [2, 1, 0, 0, 0],
        [0, 0, 65536, 3, 3],
        [0, 0, 65536, 3, 0],
        [0, 0, 65536, 3, 1],
        [5, 0, 65536, 0, 0],
        [4, 0, 0, 1, 0],
        [1, 0, 65536, 9, 0],
        [0, 0, 65536, 0, 2],
        [0, 0, 65536, 4, -1],
    ];
    let mut rng = 910u64;
    for n in 0..3000 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let op = (rng >> 32) % 7;
        steps.push([
            op as i32,
            ((rng >> 24) & 1) as i32,
            (if n % 7 == 0 { 2 } else { 1 }) << 16 | ((rng >> 12) & 3) as i32,
            if op == 0 {
                ((rng >> 5) % 10) as i32
            } else if op == 1 {
                ((rng >> 5) % 7) as i32 - 1
            } else {
                ((rng >> 5) & 1) as i32
            },
            ((rng >> 16) % 5) as i32,
        ]);
    }
    let mut input = vec![];
    let mut output = vec![];
    int(&mut input, steps.len() as i32);
    for step in &steps {
        for v in step {
            int(&mut input, *v);
        }
        let [op, secondary, packed, a, b] = *step;
        use native910::script::{CompiledScript, Counts, Instruction, Operand};
        let (command, args) = match op {
            0 => ("cc_create", vec![packed, a, b]),
            1 => ("cc_find", vec![packed, a]),
            2 => ("cc_delete", vec![]),
            3 => ("cc_deleteall", vec![packed]),
            4 => (
                if a != 0 {
                    "cc_sendtofront"
                } else {
                    "cc_sendtoback"
                },
                vec![],
            ),
            5 => ("if_find", vec![packed]),
            6 => (
                if a != 0 {
                    "if_sendtofront"
                } else {
                    "if_sendtoback"
                },
                vec![packed],
            ),
            _ => unreachable!(),
        };
        let mut code: Vec<_> = args
            .into_iter()
            .map(|v| Instruction {
                opcode: 0,
                command: "push_constant_int".into(),
                operand: Operand::Int(v),
            })
            .collect();
        code.push(Instruction {
            opcode: 0,
            command: command.into(),
            operand: Operand::Byte(secondary as u8),
        });
        code.push(Instruction {
            opcode: 0,
            command: "return".into(),
            operand: Operand::Byte(0),
        });
        let script = CompiledScript {
            name: Some("component-graph".into()),
            locals: Counts::default(),
            args: Counts::default(),
            code,
        };
        let result = {
            let mut host = c::ScriptHost {
                engine: &mut engine,
                store: &mut store,
                active: &mut active,
                properties: None,
            };
            native910::vm::Vm::new(&mut host, &())
                .execute(&script, &[])
                .map(|v| match v {
                    Some(native910::vm::Value::Int(v)) => v,
                    None => -1,
                    _ => panic!("unexpected component result"),
                })
        };
        output.push(result.is_ok() as u8);
        if let Ok(v) = result {
            int(&mut output, v);
        }
        for a in &active {
            int(&mut output, node_id(&a.component, &mut ids));
            int(
                &mut output,
                a.interface
                    .as_ref()
                    .and_then(|i| {
                        store
                            .interfaces
                            .iter()
                            .find(|(_, v)| Rc::ptr_eq(i, v))
                            .map(|(id, _)| *id)
                    })
                    .unwrap_or(-1),
            );
        }
        for i in store.interfaces.values() {
            let i = i.borrow();
            array(&mut output, &Some(i.components.clone()), &mut ids);
            array(&mut output, &i.sorted, &mut ids);
        }
        // Snapshot may discover a newly created child; reserve and backfill count.
        let count_at = output.len();
        int(&mut output, 0);
        let mut n = 0;
        while n < ids.len() {
            let node = ids[n].clone();
            let node = node.borrow();
            for v in [node.f.parentlayer, node.f.id, node.f.r#type, node.f.layer] {
                int(&mut output, v);
            }
            output.push(match (&node.children, &node.sorted) {
                (None, None) => true,
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            } as u8);
            array(&mut output, &node.children, &mut ids);
            array(&mut output, &node.sorted, &mut ids);
            n += 1;
        }
        output[count_at..count_at + 4].copy_from_slice(&(n as i32).to_be_bytes());
        int(&mut output, store.updated.len() as i32);
        for c in store.updated.drain(..) {
            int(&mut output, node_id(&Some(c), &mut ids));
        }
    }
    std::fs::write(out.join("graph-input.bin"), input)?;
    std::fs::write(out.join("rust-graph.bin"), output)?;
    std::fs::write(out.join("graph-count.txt"), format!("{}\n", steps.len()))?;
    Ok(())
}
