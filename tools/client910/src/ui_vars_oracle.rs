use crate::{
    cache::Pack,
    entity_runtime::bits_pack as pack_varbits,
    protocol910::{
        script_types::script_type,
        varbits::Binding,
        variable_types::{Domain, Variable},
    },
    ui_changes::{Change, Changes},
    ui_vars::{ClientVars, Value},
};
use std::{collections::BTreeMap, io::Write};
fn int(w: &mut impl Write, v: i32) {
    w.write_all(&v.to_be_bytes()).unwrap();
}
fn long(w: &mut impl Write, v: i64) {
    w.write_all(&v.to_be_bytes()).unwrap();
}
fn boolean(w: &mut impl Write, v: bool) {
    w.write_all(&[v as u8]).unwrap();
}
fn value(w: &mut impl Write, v: &Value) {
    match v {
        Value::Int(v) => {
            int(w, 0);
            int(w, *v)
        }
        Value::Long(v) => {
            int(w, 1);
            long(w, *v)
        }
        Value::String(v) => {
            int(w, 2);
            int(w, v.len() as i32);
            for c in v {
                w.write_all(&c.to_be_bytes()).unwrap();
            }
        }
        Value::Null => int(w, 3),
    }
}
fn change(w: &mut impl Write, v: &Change) {
    long(w, v.key);
    long(w, v.timing);
    for i in v.ints {
        int(w, i);
    }
    value(w, &v.string.clone().map_or(Value::Null, Value::String));
}
fn client_state(w: &mut impl Write, c: &ClientVars) {
    boolean(w, c.permanent_dirty);
    boolean(w, c.server_dirty);
    int(w, c.dirty_ids.len() as i32);
    for id in &c.dirty_ids {
        int(w, *id);
    }
    int(w, c.values.len() as i32);
    for (id, v) in &c.values {
        int(w, *id);
        value(w, v);
    }
}
pub(crate) fn changes_state(w: &mut impl Write, c: &Changes) {
    boolean(w, c.last_push_new);
    int(w, c.cache.len() as i32);
    for v in c.cache.values() {
        change(w, v);
    }
    for q in [&c.client, &c.server] {
        int(w, q.len() as i32);
        for key in q {
            long(w, *key);
        }
    }
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn replay() -> anyhow::Result<()> {
    let root = rs910_core::test_support::repo_root();
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-vars");
    let out = scratch.dir().to_path_buf();
    let pack = Pack::open(root.join("server/data/pack"));
    let mut definitions = pack_varbits::load(&pack)?.definitions.remove(&2).unwrap();
    let mut raw = pack.read_group("config", 62)?;
    // Exercise all setter lanes/lifetimes even if this cache does not use them.
    let first = definitions.len() as i32;
    for life in [0, 1, 2, 255] {
        for ty in [0, 35, 36] {
            let id = definitions.len() as i32;
            let b = vec![3, ty, 4, life, 0];
            let v = Variable::decode(Domain::Npc, id, &b).unwrap();
            definitions.insert(
                id,
                Binding {
                    domain: 2,
                    id,
                    data_type: v.data_type,
                    lifetime: v.lifetime,
                    legacy: v.legacy,
                    client_code: 0,
                },
            );
            raw.insert(id as u32, b);
        }
    }
    let mut input = std::fs::File::create(out.join("input.bin"))?;
    let mut output = std::io::BufWriter::new(std::fs::File::create(out.join("rust.bin"))?);
    int(&mut input, definitions.len() as i32);
    for id in 0..definitions.len() as u32 {
        let b = raw.get(&id).cloned().unwrap_or_else(|| vec![0]);
        int(&mut input, b.len() as i32);
        input.write_all(&b)?;
    }
    let mut actions = Vec::<(i32, i32, Value)>::new();
    for (&id, def) in &definitions {
        actions.push((0, id, Value::Null));
        actions.push((1, id, Value::Null));
        let Some((base, _)) = def.data_type.and_then(script_type) else {
            continue;
        };
        let values = match base {
            0 => vec![
                Value::Int(-1),
                Value::Int(-1),
                Value::Int(i32::MIN),
                Value::Int(0),
                Value::Int(i32::MAX),
            ],
            1 => vec![
                Value::Long(-1),
                Value::Long(-1),
                Value::Long(i64::MIN),
                Value::Long(i64::MAX),
            ],
            2 => {
                let mut boundary = vec![65u16; 79];
                boundary.extend([0xd83d, 0xde00, 0xd800]);
                vec![
                    Value::String(vec![]),
                    Value::String(vec![]),
                    Value::String(boundary),
                    Value::Null,
                    Value::Null,
                    Value::String(vec![0xdc00; 81]),
                ]
            }
            _ => continue,
        };
        for v in values {
            actions.push((2, id, v));
            actions.push((1, id, Value::Null));
        }
        actions.push((3, id, Value::Null));
        actions.push((1, id, Value::Null));
    }
    // Mixed lifetimes in one domain; reset must retain permanent/unknown only.
    actions.push((0, first, Value::Null));
    for offset in [0, 3, 6, 9] {
        actions.push((2, first + offset, Value::Int(42)));
    }
    actions.push((3, first, Value::Null));
    for offset in [0, 3, 6, 9] {
        actions.push((1, first + offset, Value::Null));
    }
    int(&mut input, actions.len() as i32);
    let mut client = ClientVars::default();
    for (op, id, v) in &actions {
        int(&mut input, *op);
        int(&mut input, *id);
        value(&mut input, v);
        let result = match op {
            0 => {
                client = ClientVars::default();
                Ok(Value::Null)
            }
            1 => client.get(&definitions[id]),
            2 => client.set(&definitions[id], v.clone()).map(|_| Value::Null),
            3 => {
                client.reset(&definitions);
                Ok(Value::Null)
            }
            _ => unreachable!(),
        };
        boolean(&mut output, result.is_ok());
        if let Ok(v) = result {
            value(&mut output, &v);
        }
        client_state(&mut output, &client);
    }
    let mut queue_steps = vec![
        (0, 1, 7, 1000),
        (1, 1, 7, 1499),
        (2, 0, 0, 1499),
        (2, 0, 0, 1500),
        (1, 3, 9, 2000),
        (0, 3, 9, 2001),
        (2, 0, 0, 2500),
        (2, 0, 0, 2501),
        (0, 1, 1, 3000),
        (0, 1, 2, 3001),
        (0, 1, 1, 3002),
        (1, 1, 2, 3003),
        (2, 0, 0, 3502),
    ];
    let mut rng = 910u64;
    let mut now = 4000;
    for step in 0..5000 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        now += ((rng >> 32) % 100) as i64;
        let target = match step % 41 {
            0 => -1,
            1 => i64::MIN,
            2 => 0x1122_3344_5566_7788,
            _ => ((rng >> 20) % 64) as i64,
        };
        let op = if step % 127 == 0 {
            3
        } else {
            ((rng >> 16) % 3) as i32
        };
        queue_steps.push((op, ((rng >> 40) % 24) as i32, target, now));
    }
    int(&mut input, queue_steps.len() as i32);
    let mut queue = Changes::default();
    for (step, (op, kind, target, now)) in queue_steps.iter().enumerate() {
        int(&mut input, *op);
        int(&mut input, *kind);
        long(&mut input, *target);
        long(&mut input, *now);
        let mut reads = 0;
        let result = match op {
            0 => {
                queue.push_client(*kind, *target, *now);
                reads = 1;
                None
            }
            1 => {
                let c = queue.push_server(*kind, *target);
                c.ints = [step as i32, *kind, *target as i32];
                c.string = Some(vec![step as u16]);
                None
            }
            2 => queue.poll(|| {
                reads += 1;
                *now
            }),
            3 => {
                queue.clear();
                None
            }
            _ => unreachable!(),
        };
        int(&mut output, reads);
        boolean(&mut output, result.is_some());
        if let Some(v) = result {
            change(&mut output, &v);
        }
        changes_state(&mut output, &queue);
    }
    std::fs::write(
        out.join("counts.txt"),
        format!(
            "{} {} {}\n",
            definitions.len(),
            actions.len(),
            queue_steps.len()
        ),
    )?;
    eprintln!(
        "{} client definitions, {} variable actions, {} queue actions",
        definitions.len(),
        actions.len(),
        queue_steps.len()
    );
    host_replay(&out, false)?;
    host_replay(&out, true)?;
    output.flush()?;
    scratch.finish(
        "ui-vars",
        &[
            ("rust.bin", "recording"),
            ("rust-host.bin", "host"),
            ("rust-hookhost.bin", "host"),
        ],
    );
    Ok(())
}

fn host_replay(out: &std::path::Path, hook_host: bool) -> anyhow::Result<()> {
    use crate::{
        entities910::varps::Varps,
        protocol910::variables::Value as Wire,
        ui_vars::{State, Variables, WithVariables},
    };
    use native910::{
        script::{CompiledScript, Counts, Instruction, Operand, VarRef},
        vars::VarScope as D,
        vm::Vm,
    };
    let mut inputs = pack_varbits::Inputs {
        definitions: BTreeMap::new(),
        raw: BTreeMap::new(),
        count: 4,
    };
    for domain in [0, 1, 2] {
        let defs = [0, 36, 35, 1]
            .into_iter()
            .enumerate()
            .map(|(id, ty)| {
                (
                    id as i32,
                    Binding {
                        domain,
                        id: id as i32,
                        data_type: Some(ty),
                        lifetime: Some(if domain == 2 { id as u8 % 3 } else { 0 }),
                        legacy: true,
                        client_code: 0,
                    },
                )
            })
            .collect();
        inputs.definitions.insert(domain, defs);
        inputs
            .raw
            .insert(domain as u32, vec![1, domain, 0, 0, 2, 0, 2, 0]);
    }
    inputs.raw.insert(3, vec![1, 2, 0, 0, 2, 1, 8, 0]);
    let book = native910::opcode::OpcodeBook::embedded()?;
    let instruction = |command: &str, operand| Instruction {
        opcode: 0,
        command: command.into(),
        operand,
    };
    let mut cases = Vec::new();
    for active in [true, false] {
        for domain in [D::Player, D::Npc, D::Client] {
            for secondary in [false, true] {
                for id in 0..4 {
                    let var = Operand::VarRef(VarRef {
                        domain,
                        id,
                        transmog: secondary,
                    });
                    for write in [false, true, false] {
                        let mut code = vec![];
                        if write {
                            code.push(match id {
                                1 => instruction(
                                    "push_constant_string",
                                    Operand::Str("x".repeat(81)),
                                ),
                                2 => instruction(
                                    "push_constant_string",
                                    Operand::Long(i64::MIN + 910),
                                ),
                                _ => instruction("push_constant_int", Operand::Int(910)),
                            });
                            code.push(instruction("pop_var", var.clone()));
                        }
                        code.push(instruction("push_var", var.clone()));
                        code.push(instruction("return", Operand::Byte(0)));
                        cases.push((
                            active,
                            CompiledScript {
                                name: Some("123".into()),
                                locals: Counts::default(),
                                args: Counts::default(),
                                code,
                            },
                        ));
                    }
                }
            }
        }
    }
    // Varbit operands reference each domain's actual integer definition.
    for active in [true, false] {
        for id in 0..3 {
            for secondary in [false, true] {
                for v in [0, 7] {
                    let bit = Operand::VarBitRef(native910::script::VarBitRef {
                        id,
                        transmog: secondary,
                    });
                    cases.push((
                        active,
                        CompiledScript {
                            name: Some("124".into()),
                            locals: Counts::default(),
                            args: Counts::default(),
                            code: vec![
                                instruction("push_constant_int", Operand::Int(v)),
                                instruction("pop_varbit", bit.clone()),
                                instruction("push_varbit", bit),
                                instruction("return", Operand::Byte(0)),
                            ],
                        },
                    ));
                }
            }
        }
    }
    for command in [
        "detailget_vsync",
        "detailcanmod_vsync",
        "has_nxt",
        "detailget_musicvol",
        "detailget_musicvol",
        "detailget_soundvol",
        "detailget_soundvol",
        "detailget_bgsoundvol",
        "detailget_bgsoundvol",
        "detailget_speechvol",
        "detailget_speechvol",
        "clienttype",
        "clienttype",
    ] {
        cases.push((
            false,
            CompiledScript {
                name: Some("125".into()),
                locals: Counts::default(),
                args: Counts::default(),
                code: vec![
                    instruction(command, Operand::Byte(0)),
                    instruction("return", Operand::Byte(0)),
                ],
            },
        ));
    }
    let bit = Operand::VarBitRef(native910::script::VarBitRef {
        id: 3,
        transmog: false,
    });
    cases.push((
        false,
        CompiledScript {
            name: Some("126".into()),
            locals: Counts::default(),
            args: Counts::default(),
            code: vec![
                instruction("detailget_musicvol", Operand::Byte(0)),
                instruction("pop_varbit", bit.clone()),
                instruction("push_varbit", bit),
                instruction("return", Operand::Byte(0)),
            ],
        },
    ));
    let mut input = std::fs::File::create(out.join("host-input.bin"))?;
    let mut output = std::io::BufWriter::new(std::fs::File::create(out.join(if hook_host {
        "rust-hookhost.bin"
    } else {
        "rust-host.bin"
    }))?);
    int(&mut input, cases.len() as i32);
    let mut state = State::default();
    let mut player = Varps::new(4);
    let mut active_player = BTreeMap::new();
    let mut active_npc = BTreeMap::new();
    let mut engine = crate::iface::VmState::new();
    let mut hook_context = crate::ui_hooks::ExecutionContext::default();
    let mut components = crate::ui_components::Store::default();
    let mut properties = crate::ui_properties::State::default();
    for (step, (active, script)) in cases.iter().enumerate() {
        let bytes = native910::script::encode_script(script, &book)?;
        boolean(&mut input, *active);
        // The recording sets the four ClientOptions volumes it reads; the
        // login volume (slot 4) stays at its default.
        let volumes = if step % 2 == 0 {
            [127; 5]
        } else {
            [11, 22, 33, 44, 127]
        };
        state.queries.client_type = if step % 2 == 0 { 0 } else { -1 };
        for (name, value) in crate::ui_preferences::VOLUMES.iter().zip(volumes) {
            state.queries.preferences.options.values
                [crate::client_options::ClientOptions::field_index(name).unwrap()] = value;
        }
        for v in &volumes[..4] {
            int(&mut input, *v);
        }
        int(&mut input, state.queries.client_type);
        long(&mut input, 1000 + step as i64);
        int(&mut input, bytes.len() as i32);
        input.write_all(&bytes)?;
        let mut reads = 0;
        let result = {
            let mut clock = || {
                reads += 1;
                1000 + step as i64
            };
            let mut variables = Variables {
                cycle: 0,
                definitions: &inputs,
                state: &mut state,
                player: Some(&mut player),
                active_player: active.then_some(&mut active_player),
                active_npc: active.then_some(&mut active_npc),
                now: &mut clock,
                probe: None,
                varp_transmit: Default::default(),
                scene: Default::default(),
            };
            if hook_host {
                let mut host = crate::ui_hook_host::HookHost {
                    engine: &mut engine,
                    store: &mut components,
                    properties: &mut properties,
                    context: &mut hook_context,
                    domains: crate::ui_hook_host::Domains::Game(&mut variables),
                };
                Vm::new(&mut host, &()).execute(script, &[])
            } else {
                let mut host = WithVariables {
                    engine: &mut engine,
                    variables,
                };
                Vm::new(&mut host, &()).execute(script, &[])
            }
        };
        boolean(&mut output, result.is_ok());
        if let Ok(v) = result {
            value(&mut output, &v.map_or(Value::Null, Value::from));
        }
        int(&mut output, reads);
        client_state(&mut output, &state.client);
        changes_state(&mut output, &state.delayed);
        for v in &player.current {
            int(&mut output, *v);
        }
        let pending = player.pending_order();
        int(&mut output, pending.len() as i32);
        for (id, t) in pending {
            int(&mut output, id);
            long(&mut output, t);
        }
        for values in [&active_player, &active_npc] {
            int(&mut output, values.len() as i32);
            for (id, v) in values {
                int(&mut output, *id);
                value(
                    &mut output,
                    &match v {
                        Wire::Int(v) => Value::Int(*v),
                        Wire::Long(v) => Value::Long(*v),
                        Wire::String(v) => Value::String(v.encode_utf16().collect()),
                        _ => unreachable!(),
                    },
                );
            }
        }
    }
    std::fs::write(out.join("host-count.txt"), format!("{}\n", cases.len()))?;
    Ok(())
}

#[test]
fn borrowed_vars_share_server_timing_and_actor_values() -> anyhow::Result<()> {
    use crate::{
        entities910::varps::Varps,
        protocol910::variables::Value as W,
        ui_vars::{State, Variables},
    };
    use native910::{vars::VarScope as D, vm::Value as V};
    let binding = |d, id, t| Binding {
        domain: d,
        id,
        data_type: Some(t),
        lifetime: Some(0),
        legacy: true,
        client_code: 0,
    };
    let inputs = pack_varbits::Inputs {
        definitions: BTreeMap::from([
            (0, BTreeMap::from([(0, binding(0, 0, 0))])),
            (1, BTreeMap::from([(0, binding(1, 0, 35))])),
            (
                2,
                BTreeMap::from([
                    (0, binding(2, 0, 0)),
                    (1, binding(2, 1, 36)),
                    (2, binding(2, 2, 35)),
                ]),
            ),
        ]),
        raw: BTreeMap::from([
            (0, vec![1, 0, 0, 0, 2, 0, 2, 0]),
            (1, vec![1, 2, 0, 0, 2, 0, 2, 0]),
        ]),
        count: 2,
    };
    let mut state = State::default();
    let mut varps = Varps::new(1);
    let mut actor = BTreeMap::from([(0, W::Int(91))]);
    let mut now = || 1000;
    {
        let mut host = Variables {
            cycle: 0,
            definitions: &inputs,
            state: &mut state,
            player: Some(&mut varps),
            active_player: Some(&mut actor),
            active_npc: None,
            now: &mut now,
            probe: None,
            varp_transmit: Default::default(),
            scene: Default::default(),
        };
        assert_eq!(host.get(D::Client, 0, false)?, V::Int(-1));
        assert_eq!(host.get(D::Client, 2, false)?, V::Long(-1));
        assert!(host.get(D::Client, 0, true).is_err());
        assert!(host.get(D::Npc, 0, false).is_err());
        host.set(D::Player, 0, false, V::Int(22))?;
        assert_eq!(host.get(D::Player, 0, true)?, V::Int(91));
        host.set(D::Player, 0, true, V::Int(93))?;
        host.set_bit(0, false, 8)?; // Overflow is logged and ignored only on local arrays.
        assert!(host.set_bit(0, true, 8).is_err());
        assert!(host.set_bit(1, false, 8).is_err());
        host.set_bit(1, false, 2)?;
        assert!(host.state.delayed.cache.is_empty());
        host.set(D::Client, 1, false, V::Str("shared".into()))?;
    }
    assert_eq!(actor[&0], W::Int(93));
    assert_eq!(varps.get(0).unwrap(), 22);
    assert_eq!(state.ignored_player_bit_overflows, vec![0]);
    assert_eq!(state.delayed.client.len(), 1);
    varps.set_server(0, 44, 1100).unwrap();
    assert_eq!(varps.poll(true, 1600).unwrap(), -1);
    assert_eq!(varps.poll(true, 1601).unwrap(), 0);
    assert_eq!(varps.get(0).unwrap(), 44);
    let host = Variables {
        cycle: 0,
        definitions: &inputs,
        state: &mut state,
        player: Some(&mut varps),
        active_player: None,
        active_npc: None,
        now: &mut now,
        probe: None,
        varp_transmit: Default::default(),
        scene: Default::default(),
    };
    assert_eq!(host.get(D::Player, 0, false)?, V::Int(44));
    assert_eq!(host.get(D::Client, 1, false)?, V::Str("shared".into()));
    Ok(())
}
