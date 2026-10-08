use crate::ui_components_oracle::{int, text};
use crate::{
    cache::Pack,
    ui_configs::{self as c, Enum, Scalar, Storage, Struct},
    ui_properties::State,
};
use native910::{
    config::{self as wire, ConfigValue, EnumRow, EnumSlot, EnumValues, StructRow},
    opcode::OpcodeBook,
    script::{CompiledScript, Counts, Instruction, Operand},
    vm::{Session, Vm},
};
use std::collections::BTreeSet;
fn txt(o: &mut Vec<u8>, s: &str) {
    text(o, Some(&s.encode_utf16().collect::<Vec<_>>()));
}
fn ints(o: &mut Vec<u8>, a: &[i32]) {
    int(o, a.len() as i32);
    for v in a {
        int(o, *v);
    }
}
fn scalar(o: &mut Vec<u8>, s: &Scalar) {
    match s {
        Scalar::Int(v) => {
            int(o, 0);
            int(o, *v)
        }
        Scalar::String(v) => {
            int(o, 2);
            text(o, Some(v))
        }
    }
}
fn integer(o: &mut Vec<u8>, r: anyhow::Result<i32>) {
    o.push(r.is_ok() as u8);
    if let Ok(v) = r {
        int(o, v);
    }
}
fn string(o: &mut Vec<u8>, r: anyhow::Result<Option<&[u16]>>) {
    o.push(r.is_ok() as u8);
    if let Ok(v) = r {
        text(o, v);
    }
}
fn record(o: &mut Vec<u8>, id: i32, b: &[u8]) {
    int(o, id);
    int(o, b.len() as i32);
    o.extend(b);
}
fn enumeration(o: &mut Vec<u8>, e: &Enum) {
    int(o, e.input.unwrap_or(i32::MIN));
    int(o, e.output.unwrap_or(i32::MIN));
    int(o, e.default_int);
    text(o, Some(&e.default_string));
    int(o, e.count);
    match &e.storage {
        Storage::Empty => int(o, 0),
        Storage::Sparse(_) => int(o, 1),
        Storage::Dense(a) => {
            int(o, 2);
            int(o, a.len() as i32);
        }
    }
    let a = e.entries();
    int(o, a.len() as i32);
    for (k, v) in a {
        int(o, k);
        scalar(o, v);
    }
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-configs");
    let out = scratch.dir().to_path_buf();
    let root = rs910_core::test_support::repo_root();
    let pack = Pack::open(root.join("server/data/pack"));
    let mut state = State::default();
    state.load(&pack)?;
    let mut enums = c::records(&pack, "enum.config", 8)?.1;
    let (_, mut structs) = c::records(&pack, "struct.config", 5)?;
    let cache_counts = [enums.len(), structs.len(), state.params.len()];
    let int_param = *state.params.iter().find(|(_, p)| !p.string).unwrap().0;
    let str_param = *state.params.iter().find(|(_, p)| p.string).unwrap().0;
    let synth_enum = enums.keys().next_back().copied().unwrap() + 2;
    let fixtures = [
        wire::EnumConfig {
            input_type: Some(0),
            output_type: Some(0),
            values: Some(EnumValues::SparseInt(
                vec![(7, 5), (-3, 5), (7, 9), (i32::MAX, 5), (i32::MIN, -1)]
                    .into_iter()
                    .map(|(key, v)| EnumRow {
                        key,
                        value: ConfigValue::Int(v),
                    })
                    .collect(),
            )),
            default_int: Some(-17),
            ..Default::default()
        },
        wire::EnumConfig {
            input_type: Some(0),
            output_type: Some(36),
            values: Some(EnumValues::DenseString {
                capacity: 10,
                slots: vec![(5, "z"), (1, "a"), (5, "a")]
                    .into_iter()
                    .map(|(index, s)| EnumSlot {
                        index,
                        value: ConfigValue::Str(s.into()),
                    })
                    .collect(),
            }),
            ..Default::default()
        },
        wire::EnumConfig {
            input_type: Some(0),
            output_type: Some(0),
            values: Some(EnumValues::SparseString(vec![EnumRow {
                key: 0,
                value: ConfigValue::Str("mismatch".into()),
            }])),
            ..Default::default()
        },
        wire::EnumConfig {
            input_legacy: Some(b'O'),
            output_legacy: Some(b's'),
            ..Default::default()
        },
        wire::EnumConfig {
            input_type: Some(32767),
            output_type: Some(32767),
            ..Default::default()
        },
        wire::EnumConfig::default(),
    ];
    for (n, e) in fixtures.into_iter().enumerate() {
        let b = wire::encode_enum(&e)?;
        let id = synth_enum + n as i32;
        state.configs.enums.insert(id, Enum::decode(&b)?);
        enums.insert(id, b);
    }
    let synth_struct = state.configs.struct_count + 2;
    for (n, params) in [
        vec![
            (int_param, ConfigValue::Int(7)),
            (int_param, ConfigValue::Int(9)),
            (str_param, ConfigValue::Str("first".into())),
            (str_param, ConfigValue::Str("last".into())),
        ],
        vec![
            (str_param, ConfigValue::Int(7)),
            (int_param, ConfigValue::Str("wrong node".into())),
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let st = wire::StructConfig {
            params: params
                .into_iter()
                .map(|(param, value)| StructRow {
                    param: param as u32,
                    value,
                })
                .collect(),
        };
        let b = wire::encode_struct(&st)?;
        let id = synth_struct + n as i32;
        state.configs.structs.insert(id, Struct::decode(&b)?);
        structs.insert(id, b);
    }
    state.configs.struct_count = synth_struct + 2;
    let mut input = vec![];
    let mut output = vec![];
    for byte in 0..=255 {
        let r = crate::ui_legacy_types::legacy(byte);
        output.push(r.is_ok() as u8);
        if let Ok(v) = r {
            int(&mut output, v.unwrap_or(i32::MIN));
        }
    }
    for id in 0..32768 {
        int(&mut output, c::serial(id).unwrap_or(i32::MIN));
    }
    let params = pack.read_group("config", 11)?;
    int(&mut input, params.len() as i32);
    for (id, b) in params {
        record(&mut input, id as i32, &b);
        let p = &state.params[&(id as i32)];
        output.push(p.string as u8);
        int(&mut output, p.integer);
        text(&mut output, p.text.as_deref());
    }
    int(&mut input, enums.len() as i32);
    for (&id, b) in &enums {
        record(&mut input, id, b);
        let e = &state.configs.enums[&id];
        enumeration(&mut output, e);
        let keys: BTreeSet<i32> = e
            .entries()
            .iter()
            .map(|p| p.0)
            .chain([i32::MIN, -1, 0, 1, i32::MAX])
            .collect();
        int(&mut input, keys.len() as i32);
        for key in keys {
            int(&mut input, key);
            integer(&mut output, e.integer(key));
            string(&mut output, e.string(key).map(Some));
        }
        let values: BTreeSet<Scalar> = e
            .entries()
            .iter()
            .map(|p| p.1.clone())
            .chain([
                Scalar::Int(e.default_int),
                Scalar::String(e.default_string.clone()),
                Scalar::Int(i32::MIN),
                Scalar::String(vec![]),
            ])
            .collect();
        int(&mut input, values.len() as i32);
        for value in values {
            scalar(&mut input, &value);
            let reverse = e.reverse(&value);
            output.push(reverse.is_some() as u8);
            match reverse {
                None => int(&mut output, -1),
                Some(a) => ints(&mut output, a),
            };
            output.push(e.reverse.get().is_some() as u8);
        }
    }
    int(&mut input, state.configs.struct_count);
    int(&mut input, structs.len() as i32);
    for (&id, b) in &structs {
        record(&mut input, id, b);
        let st = &state.configs.structs[&id];
        let mut sorted = st.params.clone();
        sorted.sort_by_key(|p| p.0);
        int(&mut output, sorted.len() as i32);
        for (k, v) in sorted {
            int(&mut output, k);
            scalar(&mut output, &v);
        }
        let keys: BTreeSet<i32> = st
            .params
            .iter()
            .map(|p| p.0)
            .chain([i32::MIN, -1, i32::MAX])
            .collect();
        int(&mut input, keys.len() as i32);
        for key in keys {
            int(&mut input, key);
            integer(&mut output, st.integer(key, -999));
            string(&mut output, st.string(key, None));
        }
    }
    std::fs::write(out.join("input.bin"), input)?;
    std::fs::write(out.join("rust.bin"), output)?;
    let host_cases = host(&out, &mut state, int_param, str_param)?;
    std::fs::write(
        out.join("counts.txt"),
        format!(
            "{} {} {} {} {} {}\n",
            cache_counts[0],
            cache_counts[1],
            cache_counts[2],
            enums.len(),
            structs.len(),
            host_cases
        ),
    )?;
    eprintln!(
        "Configs: {:?} cache counts, {host_cases} real-VM cases",
        cache_counts
    );
    scratch.finish(
        "ui-configs",
        &[("rust.bin", "recording"), ("rust-host.bin", "host")],
    );
    Ok(())
}
fn host(
    out: &std::path::Path,
    state: &mut State,
    int_param: i32,
    str_param: i32,
) -> anyhow::Result<usize> {
    let mut cases: Vec<(&str, Vec<i32>, Vec<String>)> = vec![];
    for (&id, e) in &state.configs.enums {
        let it = e.input.unwrap_or(i32::MIN);
        let ot = e.output.unwrap_or(i32::MIN);
        let entries = e.entries();
        let first = entries.first().map_or(0, |p| p.0);
        let iv = entries
            .iter()
            .find_map(|p| {
                if let Scalar::Int(v) = p.1 {
                    Some(*v)
                } else {
                    None
                }
            })
            .unwrap_or(-999);
        let sv = entries
            .iter()
            .find_map(|p| {
                if let Scalar::String(v) = p.1 {
                    Some(String::from_utf16(v).unwrap())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "missing".into());
        cases.push(("enum_getoutputcount", vec![id], vec![]));
        for k in [first, -1] {
            cases.push(("enum_string", vec![id, k], vec![]));
            cases.push(("_enum", vec![it, ot, id, k], vec![]));
        }
        cases.push(("_enum", vec![it, ot.wrapping_add(1), id, first], vec![]));
        for command in ["enum_hasoutput", "enum_getreversecount"] {
            cases.push((command, vec![ot, id, iv], vec![]));
            cases.push((command, vec![ot.wrapping_add(1), id, iv], vec![]));
        }
        for command in ["enum_hasoutput_string", "enum_getreversecount_string"] {
            cases.push((command, vec![id], vec![sv.clone()]));
        }
        for ordinal in [-1, 0, 1, i32::MAX] {
            cases.push((
                "enum_getreverseindex",
                vec![ot, it, id, iv, ordinal],
                vec![],
            ));
            cases.push((
                "enum_getreverseindex_string",
                vec![it, id, ordinal],
                vec![sv.clone()],
            ));
        }
    }
    for (&id, st) in &state.configs.structs {
        let keys: BTreeSet<i32> = st
            .params
            .iter()
            .map(|p| p.0)
            .chain([int_param, str_param, -1])
            .collect();
        for k in keys {
            cases.push(("struct_param", vec![id, k], vec![]));
        }
    }
    for id in [
        -1,
        i32::MIN,
        state.configs.struct_count - 3,
        state.configs.struct_count,
        i32::MAX,
    ] {
        for p in [int_param, str_param, -1] {
            cases.push(("struct_param", vec![id, p], vec![]));
        }
        cases.push(("enum_getoutputcount", vec![id], vec![]));
        cases.push(("enum_string", vec![id, 0], vec![]));
        cases.push(("_enum", vec![0, 0, id, 0], vec![]));
        cases.push(("enum_hasoutput", vec![0, id, 0], vec![]));
    }
    let book = OpcodeBook::embedded()?;
    let mut input = vec![];
    let mut output = vec![];
    let mut offsets = String::new();
    int(&mut input, cases.len() as i32);
    let mut store = crate::ui_components::Store::default();
    let mut active = Default::default();
    let mut changes = crate::ui_changes::Changes::default();
    let mut engine = crate::iface::VmState::new();
    for (index, (command, i, s)) in cases.iter().enumerate() {
        let ins = |command: &str, operand| Instruction {
            opcode: 0,
            command: command.into(),
            operand,
        };
        let mut code = vec![
            ins("push_constant_int", Operand::Int(73)),
            ins("push_constant_string", Operand::Str("prefix".into())),
        ];
        for v in i {
            code.push(ins("push_constant_int", Operand::Int(*v)));
        }
        for v in s {
            code.push(ins("push_constant_string", Operand::Str(v.clone())));
        }
        code.push(ins(command, Operand::Byte(0)));
        code.push(ins("return", Operand::Byte(0)));
        let script = CompiledScript {
            name: Some(index.to_string()),
            locals: Counts::default(),
            args: Counts::default(),
            code,
        };
        let b = native910::script::encode_script(&script, &book)?;
        int(&mut input, b.len() as i32);
        input.extend(&b);
        let decoded = native910::script::decode_script(&b, &book)?;
        let mut session = Session::new(&decoded, &[])?;
        let result = {
            let mut now = || panic!("config read touched delayed clock");
            let mut h = crate::ui_components::ScriptHost {
                engine: &mut engine,
                store: &mut store,
                active: &mut active,
                properties: Some(crate::ui_properties::Context {
                    nested_count: 0,
                    state,
                    changes: &mut changes,
                    now: &mut now,
                }),
            };
            let mut vm = Vm::new(&mut h, &());
            loop {
                match vm.step(&mut session) {
                    Ok(true) => break Ok(()),
                    Ok(false) => {}
                    Err(e) => break Err(e),
                }
            }
        };
        offsets.push_str(&format!("{} {index} {command} {i:?}\n", output.len()));
        output.push(result.is_ok() as u8);
        if result.is_ok() {
            let snap = session.snapshot();
            ints(&mut output, &snap.ints);
            int(&mut output, snap.strings.len() as i32);
            for s in &snap.strings {
                if let Some(s) = s {
                    txt(&mut output, s);
                } else {
                    int(&mut output, -1);
                }
            }
            int(&mut output, snap.longs.len() as i32);
            for l in snap.longs {
                output.extend(l.to_be_bytes());
            }
        }
    }
    anyhow::ensure!(engine.trapped().is_empty(), "config op leaked to preview");
    std::fs::write(out.join("host-input.bin"), input)?;
    std::fs::write(out.join("rust-host.bin"), output)?;
    std::fs::write(out.join("rust-host-offsets.txt"), offsets)?;
    Ok(cases.len())
}
