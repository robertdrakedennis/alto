use crate::{
    ui_components::{Arg, Component, Store},
    ui_components_oracle::{int, text},
    ui_hook_host::{Domains, ScriptRun},
    ui_hooks::{Pool, Request},
    ui_properties::State,
    ui_text_compare::{self as compare, Language},
    ui_vars::{self, Variables},
};
use native910::{
    opcode::OpcodeBook,
    script::{CompiledScript, Counts, Instruction, Operand},
};
use std::{cell::RefCell, rc::Rc};
fn ins(name: &str, operand: Operand) -> Instruction {
    Instruction {
        opcode: 0,
        command: name.into(),
        operand,
    }
}
fn txt(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
fn pair(input: &mut Vec<u8>, output: &mut Vec<u8>, a: &[u16], b: &[u16], language: i32) {
    int(input, language);
    text(input, Some(a));
    text(input, Some(b));
    int(output, compare::compare(a, b, Language::from_id(language)));
}
#[test]
fn replay() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("ui-client-state");
    let out = scratch.dir().to_path_buf();
    let mut input = vec![];
    let mut output = vec![];
    // Full reference-JVM char casing, normalization, ligatures and sorting keys.
    for c in 0..=u16::MAX {
        let (up, lo, upper) = compare::casing(c);
        int(&mut output, up as i32);
        int(&mut output, lo as i32);
        output.push(upper as u8);
        int(&mut output, compare::expansion(c) as i32);
        for lang in -1..=6 {
            int(
                &mut output,
                compare::normalize(c, Language::from_id(lang)) as i32,
            );
            int(&mut output, compare::sort_key(c, Language::from_id(lang)));
        }
    }
    let pos = input.len();
    int(&mut input, 0);
    let mut count = 0_i32;
    let cases = [
        "", "a", "A", "Æ", "AE", "æ", "ae", "ß", "ss", "Œ", "OE", "œ", "oe", "Ñ", "ñ", "n", "N",
        "cote", "côte", "coté", "côté", "İ", "i", "ı", "ǅ", "ǆ", "Σ", "σ", "ς", "😀", "\0", "a\0b",
        "𐐀", "𐐨",
    ]
    .map(txt);
    for lang in -1..=6 {
        for a in &cases {
            for b in &cases {
                pair(&mut input, &mut output, a, b, lang);
                count += 1;
            }
        }
        for a in 0u16..256 {
            for b in 0u16..256 {
                pair(&mut input, &mut output, &[a], &[b], lang);
                pair(
                    &mut input,
                    &mut output,
                    &[97, a, 0, 339],
                    &[97, b, 0, 111, 101],
                    lang,
                );
                count += 2;
            }
        }
        let mut rng = 910u64;
        for _ in 0..4096 {
            let mut a = vec![];
            let mut b = vec![];
            for _ in 0..16 {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                let c = (rng >> 24) as u16;
                a.push(c);
                b.push(if rng & 3 == 0 {
                    compare::casing(c).1
                } else {
                    c
                });
            }
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            a.truncate((rng & 15) as usize);
            b.truncate(((rng >> 4) & 15) as usize);
            pair(&mut input, &mut output, &a, &b, lang);
            count += 1;
        }
    }
    input[pos..pos + 4].copy_from_slice(&count.to_be_bytes());
    let book = OpcodeBook::embedded()?;
    let defs = crate::entity_runtime::bits_pack::Inputs {
        definitions: Default::default(),
        raw: Default::default(),
        count: 0,
    };
    let commands = [
        "clientclock",
        "map_lang",
        "runenergy_visible",
        "runweight_visible",
        "clienttype",
        "compare",
        "if_get_gamescreen",
        "if_set_gamescreen_enabled",
        "detailget_soundvol",
        "detailget_bgsoundvol",
        "detailget_speechvol",
        "detailget_musicvol",
        "detailget_vsync",
        "detailcanmod_vsync",
        "has_nxt",
        "cam2_getcontrolmode",
        "cam2_setfieldofviewscreen",
        "get_mousex",
        "get_mousey",
        "chat_getfilter_public",
        "chat_getfilter_private",
        "chat_getfilter_trade",
        "chat_setfilter",
        "detailget_toolkit",
        "detailget_maxscreensize",
        "getwindowmode",
        "viewport_geteffectivesize",
        "viewport_getfov",
        "viewport_getzoom",
        "viewport_clampfov",
        "viewport_setfov",
        "viewport_setzoom",
        "discardInterfaceTargetArg",
        "discardThreeInterfaceArgs",
        "discardInterfaceArg",
        "interface_setpickingradius",
        "discardLogoutArg",
    ];
    int(&mut input, commands.len() as i32 * 8 * 8);
    let mut pool = Pool::default();
    let mut vars = ui_vars::State::default();
    let mut store = Store::default();
    let mut state = State::default();
    let c = Rc::new(RefCell::new(Component::default()));
    let mut offsets = String::new();
    for command in commands {
        for language in -1..=6 {
            for n in 0..8 {
                let mut engine = crate::ui_runtime::Engine::default();
                let value = [i32::MIN, -1, 0, 1, 2, 127, 255, i32::MAX][n];
                let cycle = value.wrapping_mul(1234);
                let energy = value;
                let weight = value.wrapping_neg();
                let client_type = value;
                let packed = if n % 3 == 0 { -1 } else { value };
                let level = value;
                let enabled = n & 1 == 1;
                let volumes = [
                    value,
                    value.wrapping_add(1),
                    value.wrapping_add(2),
                    value.wrapping_add(3),
                ];
                for v in [
                    language,
                    cycle,
                    energy,
                    weight,
                    client_type,
                    packed,
                    level,
                    enabled as i32,
                ] {
                    int(&mut input, v);
                }
                for v in volumes {
                    int(&mut input, v);
                }
                vars.queries.language = Language::from_id(language);
                vars.queries.run_energy = energy;
                vars.queries.run_weight = weight;
                vars.queries.client_type = client_type;
                for (name, value) in crate::ui_preferences::VOLUMES.iter().zip(volumes) {
                    vars.queries.preferences.options.values
                        [crate::client_options::ClientOptions::field_index(name).unwrap()] = value;
                }
                c.borrow_mut().f.parentlayer = packed;
                let size = [
                    (1, 1),
                    (100, 100),
                    (1024, 768),
                    (800, 600),
                    (1600, 1200),
                    (3000, 2000),
                    (0, 0),
                    (-1, -1),
                ][n];
                c.borrow_mut().f.width = size.0;
                c.borrow_mut().f.height = size.1;
                engine.platform.pending_mouse = [value, value.wrapping_neg()];
                engine.platform.mouse = engine.platform.pending_mouse;
                state.viewport = None;
                state.layout.viewport = if packed == -1 { None } else { Some(c.clone()) };
                state.life.cached_minimap_level = level;
                state.life.game_screen_enabled = enabled;
                let mut code = vec![
                    ins("push_constant_int", Operand::Int(321)),
                    ins("push_constant_string", Operand::Str("kept".into())),
                ];
                let mut hook_args = vec![Arg::Int(1)];
                let pairs = if command == "compare" {
                    Some((
                        ["côté", "ñ", "Æ", "İ", "A", "ǅ", "\0", "😀"][n],
                        ["côte", "n", "AE", "i", "a", "ǆ", "", "😀"][n],
                    ))
                } else {
                    None
                };
                for value in [pairs.map(|p| p.0), pairs.map(|p| p.1)] {
                    let units = value.map(txt);
                    text(&mut input, units.as_deref());
                    if let Some(units) = units {
                        hook_args.push(Arg::String(units));
                    }
                }
                if pairs.is_some() {
                    code.push(ins("push_string_local", Operand::Local(0)));
                    code.push(ins("push_string_local", Operand::Local(1)));
                }
                if command == "if_set_gamescreen_enabled" {
                    code.push(ins("push_constant_int", Operand::Int(value)));
                }
                if matches!(command, "cam2_setfieldofviewscreen" | "chat_setfilter") {
                    for v in [value, value.wrapping_add(1), value.wrapping_add(2)] {
                        code.push(ins("push_constant_int", Operand::Int(v)));
                    }
                }
                if matches!(
                    command,
                    "viewport_clampfov" | "viewport_setfov" | "viewport_setzoom"
                ) {
                    for _ in 0..if command == "viewport_clampfov" { 4 } else { 2 } {
                        code.push(ins(
                            "push_constant_int",
                            Operand::Int([0, -1, 65536, 32768][n % 4]),
                        ));
                    }
                }
                if let Some((_, count)) = crate::ui_properties::DISCARDS
                    .iter()
                    .find(|(name, _)| *name == command)
                {
                    for _ in 0..*count {
                        code.push(ins("push_constant_int", Operand::Int(value)));
                    }
                }
                code.push(ins(command, Operand::Byte(0)));
                code.push(ins("return", Operand::Byte(0)));
                let script = CompiledScript {
                    name: None,
                    locals: Counts {
                        obj: if pairs.is_some() { 2 } else { 0 },
                        ..Counts::default()
                    },
                    args: Counts::default(),
                    code,
                };
                let raw = native910::script::encode_script(&script, &book)?;
                int(&mut input, raw.len() as i32);
                input.extend(&raw);
                let script = native910::script::decode_script(&raw, &book)?;
                let mut now = || panic!("read-only client query touched delayed clock");
                let mut variables = Variables {
                    cycle,
                    definitions: &defs,
                    state: &mut vars,
                    player: None,
                    active_player: None,
                    active_npc: None,
                    now: &mut now,
                    probe: None,
                    varp_transmit: Default::default(),
                    scene: Default::default(),
                };
                let r = pool.execute(
                    &mut store,
                    &mut state,
                    ScriptRun {
                        id: 1,
                        script: &script,
                        request: &Request {
                            args: Some(hook_args),
                            ..Default::default()
                        },
                        limit: 100,
                    },
                    Domains::Game(&mut variables),
                    &mut engine,
                    &(),
                )?;
                offsets.push_str(&format!("{} {command} {language} {n}\n", output.len()));
                output.push(r.result.is_ok() as u8);
                if r.result.is_ok() {
                    int(&mut output, r.snapshot.ints.len() as i32);
                    for v in r.snapshot.ints {
                        int(&mut output, v);
                    }
                    int(&mut output, r.snapshot.strings.len() as i32);
                    for v in r.snapshot.strings {
                        text(&mut output, v.as_deref().map(txt).as_deref());
                    }
                }
                output.push(state.life.game_screen_enabled as u8);
                int(&mut output, state.life.cached_minimap_level);
                for v in engine.camera.cam2.fov {
                    int(
                        &mut output,
                        if v.is_nan() {
                            0x7fc00000
                        } else {
                            v.to_bits() as i32
                        },
                    );
                }
                for v in engine.messages.filters {
                    int(&mut output, v.unwrap_or(-1));
                }
                int(&mut output, engine.outgoing.len() as i32);
                output.extend(engine.outgoing);
                let (rect, zoom) = state.viewport.unwrap_or(([0; 4], 0));
                for v in rect {
                    int(&mut output, v);
                }
                int(&mut output, zoom);
            }
        }
    }
    std::fs::write(out.join("input.bin"), input)?;
    std::fs::write(out.join("rust.bin"), output)?;
    std::fs::write(out.join("rust-offsets.txt"), offsets)?;
    std::fs::write(
        out.join("counts.txt"),
        format!("65536 {count} {}", commands.len() * 8 * 8),
    )?;
    scratch.finish("ui-client-state", &[("rust.bin", "recording")]);
    Ok(())
}
