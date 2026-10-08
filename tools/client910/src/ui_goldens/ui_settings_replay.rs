use crate::cache::Pack;
use crate::ui_runtime::*;
use rs910_symbols::component::{
    confirm_popup, controls_settings, game_window, graphics_settings_panel,
};
use rs910_symbols::{interface, script, varbit, varc, ComponentId, VarbitId};
use std::collections::BTreeMap;

fn components(ui: &Runtime) -> Vec<serde_json::Value> {
    fn walk(node: &crate::ui_components::Ref, out: &mut Vec<serde_json::Value>) {
        let c = node.borrow();
        let ops: Vec<String> = c
            .ops
            .as_ref()
            .map(|ops| {
                ops.iter()
                    .map(|s| {
                        s.as_ref()
                            .map(|s| String::from_utf16_lossy(s))
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !ops.is_empty()
            || c.hooks.contains_key("onkey")
            || c.hooks.contains_key("onclick")
            || [
                interface::GRAPHICS_SETTINGS_LOADER.id(),
                interface::GRAPHICS_SETTINGS.id(),
                interface::GRAPHICS_SETTINGS_PANEL.id(),
            ]
            .contains(&(c.f.parentlayer >> 16))
        {
            out.push(serde_json::json!({"parent":c.f.parentlayer,"child":c.f.id,"layer":c.f.layer,"hide":c.f.hide,"ops":ops,"text":c.f.text.as_ref().map(|s|String::from_utf16_lossy(s)),"keys":c.keys,"key_mods":c.key_mods,"hooks":c.hooks.iter().map(|(k,v)|((*k).to_string(),format!("{v:?}"))).collect::<BTreeMap<_,_>>(),"params":format!("{:?}",c.params)}));
        }
        if let Some(kids) = &c.children {
            for child in kids.borrow().iter().flatten() {
                walk(child, out);
            }
        }
    }
    let mut out = Vec::new();
    for interface in ui.store.interfaces.values() {
        for node in interface.borrow().components.borrow().iter().flatten() {
            walk(node, &mut out);
        }
    }
    out
}

fn apply(
    ui: &mut Runtime,
    game: &mut crate::client_game::ClientGame,
    clock: &mut crate::test_support::SimClock,
    frames: &serde_json::Value,
    pack: &Pack,
) -> anyhow::Result<()> {
    for raw in frames.as_array().unwrap() {
        let bytes: Vec<u8> = raw
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_u64().unwrap() as u8)
            .collect();
        let (frame, used) =
            crate::net::decode_frame(&bytes)?.ok_or_else(|| anyhow::anyhow!("frame"))?;
        anyhow::ensure!(used == bytes.len(), "frame length");
        if game.runtime.feed.enqueue(frame.opcode, &frame.payload) {
            game.apply_next(game.cycle as i64)
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            if game.runtime.map_request.is_some() {
                let map = game.runtime.prepare_map(pack)?;
                game.runtime
                    .install_map(map)
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            }
        } else if let Some(event) = crate::session::parse_ui_event(frame.opcode, &frame.payload)? {
            clock.with(game, |vars| ui.packet(vars, &event))?;
        }
    }
    game.poll_vars(|| clock.0)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    for _ in 0..40 {
        game.cycle += 1;
        clock.tick(game, ui)?;
    }
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn game_interface_survives_window_resize_burst() -> anyhow::Result<()> {
    let pack = crate::test_support::require_pack("client.config.js5");
    let rows: serde_json::Value = crate::test_support::replay_json("settings-tabs", "frames.json");
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    let mut clock = crate::test_support::SimClock::default();
    let mut ui = Runtime::new(pack.clone())?;
    ui.engine.account.logged_in_members = true;
    ui.diagnostics.capture = true;
    ui.resize([800, 600])?;
    apply(&mut ui, &mut game, &mut clock, &rows[0]["frames"], &pack)?;
    for canvas in [
        [1728, 1052],
        [800, 600],
        [1920, 1080],
        [960, 660],
        [1280, 720],
    ] {
        let old = ui.state.layout.canvas;
        let mut pending = crate::ui_window::PendingResize::default();
        // Cocoa maximize delivers an animated burst before the next update.
        // Preserve only its final native size before laying out the cache UI.
        for step in 1..=42 {
            pending.receive(std::array::from_fn(|axis| {
                ((old[axis] + (canvas[axis] - old[axis]) * step / 42) * 2) as u32
            }));
        }
        let window = &mut game.ui_variables.queries.preferences.window;
        window.observe_size(pending.take().unwrap(), 2.0);
        let size = window.canvas(0).unwrap().size;
        assert_eq!(size, canvas);
        assert!(pending.take().is_none());
        ui.diagnostics.executions.clear();
        ui.resize(size)?;
        let root_resizes = ui
            .state
            .layout
            .hooks
            .iter()
            .filter(|r| r.script_id().ok() == Some(script::GAME_WINDOW_RESIZE.id()))
            .count();
        assert_eq!(root_resizes, 1, "maximize must queue one root resize hook");
        game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
        let output = ui.paint(game.cycle, true, [0.05; 3])?;
        let rect = output.scene.unwrap().rect;
        assert!(
            rect[0] >= 0
                && rect[1] >= 0
                && rect[0] + rect[2] <= size[0]
                && rect[1] + rect[3] <= size[1],
            "scene {rect:?} outside {size:?}"
        );
        assert!(ui.state.layout.hooks.is_empty());
        assert!(
            ui.diagnostics.errors.is_empty(),
            "{:?}",
            ui.diagnostics.errors
        );
        eprintln!("[resize-replay] canvas={canvas:?} native_events=42 root_resizes={root_resizes} executions={}",ui.diagnostics.executions.len());
    }
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn settings_all_tabs_replay() -> anyhow::Result<()> {
    let pack = crate::test_support::require_pack("client.config.js5");
    let rows: serde_json::Value = crate::test_support::replay_json("settings-tabs", "frames.json");
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    let mut clock = crate::test_support::SimClock::default();
    let mut ui = Runtime::new(pack.clone())?;
    ui.diagnostics.capture = true;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    ui.engine.platform.fullscreen_modes = Some(vec![FullscreenMode {
        width: 1280,
        height: 720,
        bit_depth: 24,
        refresh: 60,
    }]);
    let mut proof = vec![];
    let mut capture = if std::env::var_os("CLIENT910_SETTINGS_CAPTURE").is_some() {
        Some(crate::ui_model_gpu::Capture::new([1280, 720])?)
    } else {
        None
    };
    for row in rows.as_array().unwrap() {
        ui.diagnostics.errors.clear();
        ui.diagnostics.executions.clear();
        apply(&mut ui, &mut game, &mut clock, &row["frames"], &pack)?;
        let tab = row["tab"].as_u64().unwrap();
        if tab > 0 {
            anyhow::ensure!(
                !ui.store
                    .get(game_window::MODAL_WINDOW.packed(), -1)?
                    .unwrap()
                    .borrow()
                    .f
                    .hide,
                "settings main frame hidden at {}",
                row["label"]
            );
            anyhow::ensure!(
                ui.store
                    .get(game_window::OPTIONS_MENU_LAYER.packed(), -1)?
                    .unwrap()
                    .borrow()
                    .f
                    .hide,
                "Options obscures Settings"
            );
            let ids = match tab {
                1 => vec![interface::GAMEPLAY_SETTINGS],
                2 => vec![interface::GRAPHICS_SETTINGS_LOADER],
                3 => vec![interface::CONTROLS_SETTINGS],
                4 => vec![interface::MUSIC_PLAYER, interface::AUDIO_SETTINGS],
                _ => unreachable!(),
            };
            if tab == 2 {
                anyhow::ensure!(
                    ui.state
                        .life
                        .subs
                        .get(interface::GRAPHICS_SETTINGS_LOADER.id() << 16)
                        .is_some_and(|n| n.borrow().id == interface::GRAPHICS_SETTINGS.id()),
                    "missing in-game graphics frame"
                );
                anyhow::ensure!(
                    ui.state
                        .life
                        .subs
                        .get(interface::GRAPHICS_SETTINGS.id() << 16)
                        .is_some_and(|n| n.borrow().id == interface::GRAPHICS_SETTINGS_PANEL.id()),
                    "missing graphics controls"
                );
            }
            for (slot, id) in ids.into_iter().enumerate() {
                let sub = ui
                    .state
                    .life
                    .subs
                    .get((interface::HERO_WINDOW.id() << 16) | (3 + slot as i32 * 2))
                    .ok_or_else(|| anyhow::anyhow!("tab attachment"))?;
                anyhow::ensure!(sub.borrow().id == id.id(), "active tab group");
            }
        }
        if let Some(capture) = &mut capture {
            let _ = ui.paint(game.cycle, true, [0.05; 3])?;
            let output = ui.paint(game.cycle, true, [0.05; 3])?;
            capture.render(
                output,
                &crate::test_support::proof_dir("settings")
                    .join(format!("settings-{}.png", row["label"].as_str().unwrap())),
            )?;
        }
        proof.push(serde_json::json!({"label":row["label"],"tab":tab,"main_hidden":ui.store.get(game_window::MODAL_WINDOW.packed(),-1)?.unwrap().borrow().f.hide,"attachments":ui.state.life.subs.ordered().map(|n|{let n=n.borrow();(n.parent,n.id,n.kind)}).collect::<Vec<_>>(),"components":components(&ui),"errors":ui.diagnostics.errors,"executions":ui.diagnostics.executions}));
    }
    std::fs::write(
        crate::test_support::proof_dir("settings").join("tabs-ui-proof.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    let failures: Vec<_> = proof
        .iter()
        .flat_map(|row| {
            row["errors"]
                .as_object()
                .unwrap()
                .keys()
                .map(|err| format!("{}: {err}", row["label"]))
        })
        .collect();
    anyhow::ensure!(failures.is_empty(), "settings hook failures: {failures:?}");
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn settings_normal_navigation_replay() -> anyhow::Result<()> {
    let pack = crate::test_support::require_pack("client.config.js5");
    let rows: serde_json::Value =
        crate::test_support::replay_json("settings-navigation", "frames.json");
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    let mut clock = crate::test_support::SimClock::default();
    let mut ui = Runtime::new(pack.clone())?;
    ui.diagnostics.capture = true;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    // app.rs sets this every frame from `login_state::is_game(state)`: the
    // replay is in game (state 18), where the world-list fetch command
    // may queue WORLDLIST_FETCH.
    ui.engine.login.world_list_game = true;
    ui.engine.platform.fullscreen_modes = Some(vec![FullscreenMode {
        width: 1280,
        height: 720,
        bit_depth: 24,
        refresh: 60,
    }]);
    let mut proof = vec![];
    for row in rows.as_array().unwrap() {
        ui.diagnostics.errors.clear();
        ui.diagnostics.executions.clear();
        ui.engine.outgoing.clear();
        if let Some(action) = row["action"].as_array() {
            let parent = action[0].as_i64().unwrap() as i32;
            let child = action[1].as_i64().unwrap() as i32;
            if let Some(key) = row["keyboard"].as_i64() {
                ui.keyboard.key(key as i32, 0, clock.0);
            } else if let Some(mouse) = row["mouse"].as_array() {
                let pos = [
                    mouse[0].as_i64().unwrap() as i32,
                    mouse[1].as_i64().unwrap() as i32,
                ];
                ui.engine.platform.pending_mouse = pos;
                ui.input.click = Some(pos);
                ui.input.left_held = true;
                ui.input.event = Some(crate::ui_defaults::MouseEvent {
                    pos,
                    action: 0,
                    count: 1,
                });
            } else {
                ui.state
                    .interaction
                    .actions
                    .push_back(crate::ui_interaction::Action::Op {
                        op: 1,
                        parent,
                        child,
                        base: None,
                    });
            }
            game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
            if let Some(key) = row["keyboard"].as_i64() {
                ui.keyboard.key(key as i32, 1, clock.0);
                game.cycle += 1;
                clock.tick(&mut game, &mut ui)?;
            }
            if row["mouse"].is_array() {
                ui.input.left_held = false;
                game.cycle += 1;
                clock.tick(&mut game, &mut ui)?;
            }
            let expected: Vec<u8> = row["wire"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            anyhow::ensure!(
                ui.engine
                    .outgoing
                    .windows(expected.len())
                    .any(|w| w == expected),
                "{} missing real button: {:?}",
                row["label"],
                ui.engine.outgoing
            );
            if row["label"] == "options" {
                anyhow::ensure!(
                    ui.engine.outgoing.windows(5).any(|w| w == [77, 0, 0, 0, 0]),
                    "Options did not fetch worlds"
                );
            }
        } else if row["label"] == "close-modal" {
            // Invoke the exact script command in a cache hook's lifecycle owner.
            let result =
                crate::ui_lifecycle::command(&mut ui.store, &mut ui.state, "if_close", &mut vec![]);
            result.unwrap()?;
        }
        let outgoing = ui.engine.outgoing.clone();
        apply(&mut ui, &mut game, &mut clock, &row["frames"], &pack)?;
        let tab = row["tab"].as_u64().unwrap();
        let hidden = ui
            .store
            .get(game_window::MODAL_WINDOW.packed(), -1)?
            .unwrap()
            .borrow()
            .f
            .hide;
        if tab > 0 {
            anyhow::ensure!(!hidden, "{} hid settings", row["label"]);
            anyhow::ensure!(
                ui.store
                    .get(game_window::OPTIONS_MENU_LAYER.packed(), -1)?
                    .unwrap()
                    .borrow()
                    .f
                    .hide,
                "Options obscures Settings"
            );
        }
        if row["label"] == "close" {
            anyhow::ensure!(hidden, "Escape did not hide Settings");
        }
        proof.push(serde_json::json!({"label":row["label"],"wire":outgoing,"main_hidden":hidden,"errors":ui.diagnostics.errors,"executions":ui.diagnostics.executions}));
    }
    std::fs::write(
        crate::test_support::proof_dir("settings").join("navigation-ui-proof.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    let errors: Vec<_> = proof
        .iter()
        .flat_map(|row| {
            row["errors"]
                .as_object()
                .unwrap()
                .keys()
                .map(|err| format!("{}: {err}", row["label"]))
        })
        .collect();
    anyhow::ensure!(
        errors.is_empty(),
        "settings navigation hook failures: {errors:?}"
    );
    Ok(())
}

/// One camera-family rebind through the real cache Controls tab
/// (1444:<child> onop 8208 -> key -> 8823 -> VarBit, packed in VarC 2868),
/// then held keys that must move the cam2 entity camera. `holds` are
/// `(keycode, yaw axis?, delta)`: the axis must grow by more than a positive
/// delta, or fall by more than a negative one, relative to the rotation
/// before that hold.
struct RebindCase {
    name: &'static str,
    /// `(Controls button, varbit, keycode, expected client key)`.
    binds: &'static [(ComponentId, VarbitId, i32, i32)],
    holds: &'static [(i32, bool, f32)],
    /// Check that the binding queued STORE_SERVERPERM_VARCS frames.
    persists: bool,
}

const REBIND_CASES: &[RebindCase] = &[
    // Camera-up (slot 0) from W to Q (client key 32), VarBit 18958.
    RebindCase {
        name: "up",
        binds: &[(
            controls_settings::CAMERA_UP_KEYBIND,
            varbit::CAMERA_UP_KEY,
            81,
            32,
        )],
        holds: &[(81, false, 0.1)],
        persists: true,
    },
    // Camera-down (slot 1) from S to E (34), VarBit 18959 (VarC bits 8..15).
    RebindCase {
        name: "down",
        binds: &[(
            controls_settings::CAMERA_DOWN_KEYBIND,
            varbit::CAMERA_DOWN_KEY,
            69,
            34,
        )],
        holds: &[(69, false, -0.1)],
        persists: true,
    },
    // Camera-left/right (slots 2/3) from A/D to Z/X (64/65), VarBits
    // 18960/18961 (display 1444:38/49 run 8203(4,2)/8203(4,3)).
    RebindCase {
        name: "yaw",
        binds: &[
            (
                controls_settings::CAMERA_LEFT_KEYBIND,
                varbit::CAMERA_LEFT_KEY,
                90,
                64,
            ),
            (
                controls_settings::CAMERA_RIGHT_KEYBIND,
                varbit::CAMERA_RIGHT_KEY,
                88,
                65,
            ),
        ],
        holds: &[(90, true, 0.4), (88, true, -0.4)],
        persists: true,
    },
    // Camera-up to T (36): no static 1477:32 slot holds T (fresh statics are
    // arrows/Q/E/Z/X/PgUp/PgDn), so a working T-hold proves the rebound key
    // is installed dynamically from the varbit, not shadowed by a static one.
    RebindCase {
        name: "unlisted",
        binds: &[(
            controls_settings::CAMERA_UP_KEYBIND,
            varbit::CAMERA_UP_KEY,
            84,
            36,
        )],
        holds: &[(84, false, 0.1)],
        persists: false,
    },
];

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn settings_controls_rebind_replay() -> anyhow::Result<()> {
    let pack = crate::test_support::require_pack("client.config.js5");
    let rows: serde_json::Value =
        crate::test_support::replay_json("settings-navigation", "frames.json");
    for case in REBIND_CASES {
        let name = case.name;
        let mut game = crate::client_game::ClientGame::login(
            &pack,
            1,
            crate::protocol910::live::Feed::default(),
            910,
            true,
        )?;
        let mut clock = crate::test_support::SimClock::default();
        let mut ui = Runtime::new(pack.clone())?;
        ui.diagnostics.capture = true;
        ui.resize([1280, 720])?;
        ui.engine.account.logged_in_members = true;
        ui.engine.platform.fullscreen_modes = Some(vec![FullscreenMode {
            width: 1280,
            height: 720,
            bit_depth: 24,
            refresh: 60,
        }]);
        apply(&mut ui, &mut game, &mut clock, &rows[0]["frames"], &pack)?;
        let controls = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["label"] == "controls-shortcut")
            .unwrap();
        apply(&mut ui, &mut game, &mut clock, &controls["frames"], &pack)?;
        let mut installed = vec![];
        for &(button, varbit, key, expected) in case.binds {
            ui.diagnostics.errors.clear();
            ui.diagnostics.executions.clear();
            let (child, varbit) = (button.child(), varbit.id() as u16);
            let before = clock.with(&mut game, |v| v.get_bit(varbit, false))?;
            ui.state
                .interaction
                .actions
                .push_back(crate::ui_interaction::Action::Op {
                    op: 1,
                    parent: button.packed(),
                    child: -1,
                    base: None,
                });
            clock.tick(&mut game, &mut ui)?;
            ui.keyboard.key(key, 0, clock.0);
            clock.tick(&mut game, &mut ui)?;
            ui.keyboard.key(key, 1, clock.0);
            for _ in 0..4 {
                clock.tick(&mut game, &mut ui)?;
            }
            let after = clock.with(&mut game, |v| v.get_bit(varbit, false))?;
            installed.push(
                serde_json::json!({"child":child,"varbit":varbit,"before":before,"after":after}),
            );
            anyhow::ensure!(
                ui.diagnostics.errors.is_empty(),
                "{name}: Controls edit failed: {:?}",
                ui.diagnostics.errors
            );
            anyhow::ensure!(
                after == expected,
                "{name}: key {key} was not installed on varbit {varbit}: {after}"
            );
        }
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: game_window::MODAL_CLOSE_BUTTON.packed(),
                child: 1,
                base: None,
            });
        game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
        let rotation = |ui: &Runtime| match &ui.engine.camera.cam2.position {
            Some(crate::ui_cam2::Position::Entity(p)) => Some(p.rotation),
            _ => None,
        };
        let mut before = rotation(&ui)
            .ok_or_else(|| anyhow::anyhow!("{name}: camera entity owner not enabled"))?;
        let mut rotations = vec![[before.x, before.y, before.z, before.w]];
        for &(key, yaw, delta) in case.holds {
            ui.keyboard.key(key, 0, clock.0);
            for _ in 0..10 {
                game.cycle += 1;
                clock.tick(&mut game, &mut ui)?;
            }
            ui.keyboard.key(key, 1, clock.0);
            game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
            let after = rotation(&ui).unwrap();
            let axis = |r| {
                if yaw {
                    crate::ui_cam2::yaw_of(r)
                } else {
                    crate::ui_cam2::pitch_of(r)
                }
            };
            let moved = if delta > 0.0 {
                axis(&after) > axis(&before) + delta
            } else {
                axis(&after) < axis(&before) + delta
            };
            anyhow::ensure!(
                moved,
                "{name}: key {key} did not move the camera: {before:?} -> {after:?}"
            );
            rotations.push([after.x, after.y, after.z, after.w]);
            before = after;
        }
        let mut frames = vec![];
        let now = clock.0 + 10000;
        for step in 0..20 {
            let state = &mut game.ui_variables;
            state.persistence.acknowledge();
            let frame = state
                .persistence
                .flush(&mut state.client, now + step * 1000, 0)?;
            if frame.is_empty() {
                break;
            }
            frames.push(frame);
        }
        anyhow::ensure!(
            !case.persists || !frames.is_empty(),
            "{name}: keybind did not queue persistent client values"
        );
        let value = match game
            .ui_variables
            .client
            .values
            .get(&varc::CAMERA_KEYBINDS.id())
        {
            Some(crate::ui_vars::Value::Int(v)) => Some(*v),
            _ if case.persists => anyhow::bail!("{name}: camera binding varc is not an int"),
            _ => None,
        };
        std::fs::write(
            crate::test_support::proof_dir("settings").join(format!("controls-rebind-{name}.json")),
            serde_json::to_vec_pretty(
                &serde_json::json!({"source":"real cache 1444 onop8208 -> key -> 8823 -> varbit","installed":installed,"varc":varc::CAMERA_KEYBINDS.id(),"value":value,"rotations":rotations,"frames":frames,"errors":ui.diagnostics.errors,"executions":ui.diagnostics.executions,"wire":ui.engine.outgoing}),
            )?,
        )?;
    }
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn settings_controls_reset_camera_replay() -> anyhow::Result<()> {
    // Reset-all through the real cache control chain, mirroring
    // settings_controls_rebind_camera_yaw_replay:
    // 1444:990 "Select" runs onop 8826, which opens the 1476
    // "Reset Keybinds?" modal (script 2018); its Reset button runs onop 1176
    // arg 7, and 1176 case 7 runs 8827, which arms the 9899 ontimer over the
    // camera range (category 4, slots 0-5) writing through the real 8823.
    // Defaults are the recorded camera bytes (the server settings
    // integration test): W=33 S=49 A=48 D=50,
    // VarC 2868 = 842019105. VarBits 18958/18959/18960/18961 are VarC 2868
    // bits 0..7/8..15/16..23/24..31 (cache varbit defs, read by 8825:10-38
    // and written slot-scoped by 8823:22-51).
    let pack = crate::test_support::require_pack("client.config.js5");
    let rows: serde_json::Value =
        crate::test_support::replay_json("settings-navigation", "frames.json");
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    let mut clock = crate::test_support::SimClock::default();
    let mut ui = Runtime::new(pack.clone())?;
    ui.diagnostics.capture = true;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    ui.engine.platform.fullscreen_modes = Some(vec![FullscreenMode {
        width: 1280,
        height: 720,
        bit_depth: 24,
        refresh: 60,
    }]);
    apply(&mut ui, &mut game, &mut clock, &rows[0]["frames"], &pack)?;
    let controls = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["label"] == "controls-shortcut")
        .unwrap();
    apply(&mut ui, &mut game, &mut clock, &controls["frames"], &pack)?;
    ui.diagnostics.errors.clear();
    ui.diagnostics.executions.clear();
    // Bind every camera slot away from defaults through the real capture path
    // (1444:15/26/37/48 onop 8208 -> key -> 8823): up Q=32, down E=34,
    // left Z=64, right X=65.
    for (button, awt) in [
        (controls_settings::CAMERA_UP_KEYBIND, 81),
        (controls_settings::CAMERA_DOWN_KEYBIND, 69),
        (controls_settings::CAMERA_LEFT_KEYBIND, 90),
        (controls_settings::CAMERA_RIGHT_KEYBIND, 88),
    ] {
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: button.packed(),
                child: -1,
                base: None,
            });
        clock.tick(&mut game, &mut ui)?;
        ui.keyboard.key(awt, 0, clock.0);
        clock.tick(&mut game, &mut ui)?;
        ui.keyboard.key(awt, 1, clock.0);
        for _ in 0..4 {
            clock.tick(&mut game, &mut ui)?;
        }
    }
    let bound = [
        clock.with(&mut game, |v| {
            v.get_bit(varbit::CAMERA_UP_KEY.id() as u16, false)
        })?,
        clock.with(&mut game, |v| {
            v.get_bit(varbit::CAMERA_DOWN_KEY.id() as u16, false)
        })?,
        clock.with(&mut game, |v| {
            v.get_bit(varbit::CAMERA_LEFT_KEY.id() as u16, false)
        })?,
        clock.with(&mut game, |v| {
            v.get_bit(varbit::CAMERA_RIGHT_KEY.id() as u16, false)
        })?,
    ];
    anyhow::ensure!(
        bound == [32, 34, 64, 65],
        "bind-away did not install Q/E/Z/X: {bound:?}"
    );
    anyhow::ensure!(
        ui.diagnostics.errors.is_empty(),
        "Controls bind-away failed: {:?}",
        ui.diagnostics.errors
    );
    // NOTE: no camera drive between bind-away and reset here. Driving needs
    // the settings-close op (1477:659), and closing/reopening Controls around
    // the reset perturbs the retained camera owner; the reset chain itself
    // runs with settings open (as the real UI does).
    // Drive the real reset control: 1444 child 990 ("Select", onop 8826).
    ui.diagnostics.errors.clear();
    ui.diagnostics.executions.clear();
    ui.state
        .interaction
        .actions
        .push_back(crate::ui_interaction::Action::Op {
            op: 1,
            parent: controls_settings::RESET_KEYBINDS_BUTTON.packed(),
            child: -1,
            base: None,
        });
    for _ in 0..6 {
        clock.tick(&mut game, &mut ui)?;
    }
    anyhow::ensure!(
        ui.diagnostics.errors.is_empty(),
        "Reset opener failed: {:?}",
        ui.diagnostics.errors
    );
    let modal_open = ui
        .state
        .life
        .subs
        .ordered()
        .any(|n| n.borrow().id == interface::CONFIRM_POPUP.id());
    anyhow::ensure!(
        modal_open,
        "8826 did not open the 1476 Reset Keybinds modal"
    );
    // Confirm Reset on the modal (child 10 "Select", onop 1176 arg 7), then
    // run the 9899 ontimer to completion like the 40-tick apply() drain.
    ui.state
        .interaction
        .actions
        .push_back(crate::ui_interaction::Action::Op {
            op: 1,
            parent: confirm_popup::CONFIRM_BUTTON.packed(),
            child: -1,
            base: None,
        });
    for _ in 0..40 {
        game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
    }
    anyhow::ensure!(
        ui.diagnostics.errors.is_empty(),
        "Reset confirm failed: {:?}",
        ui.diagnostics.errors
    );
    let modal_open = ui
        .state
        .life
        .subs
        .ordered()
        .any(|n| n.borrow().id == interface::CONFIRM_POPUP.id());
    anyhow::ensure!(!modal_open, "Reset modal did not close");
    let reset = [
        clock.with(&mut game, |v| {
            v.get_bit(varbit::CAMERA_UP_KEY.id() as u16, false)
        })?,
        clock.with(&mut game, |v| {
            v.get_bit(varbit::CAMERA_DOWN_KEY.id() as u16, false)
        })?,
        clock.with(&mut game, |v| {
            v.get_bit(varbit::CAMERA_LEFT_KEY.id() as u16, false)
        })?,
        clock.with(&mut game, |v| {
            v.get_bit(varbit::CAMERA_RIGHT_KEY.id() as u16, false)
        })?,
    ];
    anyhow::ensure!(
        reset == [33, 49, 48, 50],
        "reset did not restore W/S/A/D defaults: {reset:?}"
    );
    std::fs::write(
        crate::test_support::proof_dir("settings").join("controls-reset-proof.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"bound_away":bound,"reset":reset,"errors":ui.diagnostics.errors,"wire":ui.engine.outgoing}),
        )?,
    )?;
    // The reset dirtied VarC 2868 through the real 8823 path, so the shared
    // STORE_SERVERPERM_VARCS flush must carry the default value 842019105.
    let mut frames = vec![];
    let now = clock.0 + 10000;
    for step in 0..20 {
        let state = &mut game.ui_variables;
        state.persistence.acknowledge();
        let frame = state
            .persistence
            .flush(&mut state.client, now + step * 1000, 0)?;
        if frame.is_empty() {
            break;
        }
        frames.push(frame);
    }
    anyhow::ensure!(
        !frames.is_empty(),
        "reset did not queue persistent client values"
    );
    let mut flushed = None;
    for frame in &frames {
        if frame.first() != Some(&crate::proto::client::STORE_SERVERPERM_VARCS) {
            continue;
        }
        let mut at = 4;
        while at + 6 <= frame.len() {
            let id = u16::from_be_bytes([frame[at], frame[at + 1]]) as i32;
            let value =
                i32::from_be_bytes([frame[at + 2], frame[at + 3], frame[at + 4], frame[at + 5]]);
            if id == varc::CAMERA_KEYBINDS.id() {
                flushed = Some(value);
            }
            at += 6;
        }
    }
    anyhow::ensure!(
        flushed == Some(842019105),
        "STORE_SERVERPERM_VARCS flush did not carry camera defaults: {flushed:?}"
    );
    let value = match game
        .ui_variables
        .client
        .values
        .get(&varc::CAMERA_KEYBINDS.id())
    {
        Some(crate::ui_vars::Value::Int(v)) => *v,
        _ => anyhow::bail!("camera binding varc is not an int"),
    };
    // Post-reset liveness, as the cache actually behaves: the 9899 timer
    // restores varbit *values* through 8823 but does not refresh the
    // installed 1477:32 key operations (those install on the capture path,
    // proved by settings_controls_rebind_unlisted_key_replay with key T).
    // Post-reset 1477:32 slots 4-7 still hold the bind-away Q/E/Z/X, so Q
    // still drives camera-up here while the restored W/S/A/D take effect on
    // the next capture/relogin installation. Record the installs in proof.
    ui.state
        .interaction
        .actions
        .push_back(crate::ui_interaction::Action::Op {
            op: 1,
            parent: game_window::MODAL_CLOSE_BUTTON.packed(),
            child: 1,
            base: None,
        });
    game.cycle += 1;
    clock.tick(&mut game, &mut ui)?;
    let rotation = |ui: &Runtime| match &ui.engine.camera.cam2.position {
        Some(crate::ui_cam2::Position::Entity(p)) => Some(p.rotation),
        _ => None,
    };
    let before = rotation(&ui).ok_or_else(|| anyhow::anyhow!("camera entity owner not enabled"))?;
    let mut installed = vec![];
    if let Some(iface) = ui.store.interfaces.get(&interface::GAME_WINDOW.id()) {
        for node in iface.borrow().components.borrow().iter().flatten() {
            let c = node.borrow();
            if c.f.parentlayer == game_window::CAMERA_KEY_LAYER.packed() {
                if let Some(keys) = c.keys.as_ref() {
                    installed = keys
                        .iter()
                        .skip(4)
                        .take(4)
                        .map(|slot| {
                            slot.as_ref()
                                .and_then(|ks| ks.first().copied())
                                .map(i32::from)
                                .unwrap_or(-1)
                        })
                        .collect();
                }
            }
        }
    }
    ui.keyboard.key(81, 0, clock.0);
    for _ in 0..10 {
        game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
    }
    ui.keyboard.key(81, 1, clock.0);
    game.cycle += 1;
    clock.tick(&mut game, &mut ui)?;
    let after = rotation(&ui).unwrap();
    anyhow::ensure!(
        crate::ui_cam2::pitch_of(&after) > crate::ui_cam2::pitch_of(&before) + 0.1,
        "retained Q install did not reach camera-up post-reset: {before:?} -> {after:?}"
    );
    std::fs::write(
        crate::test_support::proof_dir("settings").join("controls-save-frames-reset.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"source":"real cache1444:990 onop8826 -> 1476 Reset onop1176(7) -> 8827 -> 9899 timer -> 8823 -> varbits18958-61; installs refresh on capture only","bound_away":bound,"reset":reset,"installed_1477_32_slots4_7":installed,"varc":2868,"value":value,"flushed":flushed,"rotation_before":[before.x,before.y,before.z,before.w],"rotation_after":[after.x,after.y,after.z,after.w],"frames":frames}),
        )?,
    )?;
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn settings_controls_conflict_camera_replay() -> anyhow::Result<()> {
    // Conflict through the real capture path: bind camera-down to E, then
    // bind camera-up to the same E. The binding layer itself resolves
    // nothing: it only matches (key code, char), decodes, and dispatches the
    // type byte. Resolution lives in the cache capture script: 8818 finds the conflicting slot
    // (via 8824 over the current 8825 bindings) and clears it through 8822
    // (key 255, modifier 0) before 8823 writes the winner slot-scoped
    // (8823:22-51). End state: winner takes the key, loser is CLEARED to
    // 255 (not swapped, not left duplicated).
    // State-only, no pixel.
    let pack = crate::test_support::require_pack("client.config.js5");
    let rows: serde_json::Value =
        crate::test_support::replay_json("settings-navigation", "frames.json");
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    let mut clock = crate::test_support::SimClock::default();
    let mut ui = Runtime::new(pack.clone())?;
    ui.diagnostics.capture = true;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    ui.engine.platform.fullscreen_modes = Some(vec![FullscreenMode {
        width: 1280,
        height: 720,
        bit_depth: 24,
        refresh: 60,
    }]);
    apply(&mut ui, &mut game, &mut clock, &rows[0]["frames"], &pack)?;
    let controls = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["label"] == "controls-shortcut")
        .unwrap();
    apply(&mut ui, &mut game, &mut clock, &controls["frames"], &pack)?;
    ui.diagnostics.errors.clear();
    ui.diagnostics.executions.clear();
    let bind = |ui: &mut Runtime,
                game: &mut crate::client_game::ClientGame,
                clock: &mut crate::test_support::SimClock,
                button: ComponentId,
                awt: i32|
     -> anyhow::Result<()> {
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: button.packed(),
                child: -1,
                base: None,
            });
        clock.tick(game, ui)?;
        ui.keyboard.key(awt, 0, clock.0);
        clock.tick(game, ui)?;
        ui.keyboard.key(awt, 1, clock.0);
        for _ in 0..4 {
            clock.tick(game, ui)?;
        }
        Ok(())
    };
    // Camera-down takes E (client key 34) first.
    bind(
        &mut ui,
        &mut game,
        &mut clock,
        controls_settings::CAMERA_DOWN_KEYBIND,
        69,
    )?;
    let down_first = clock.with(&mut game, |v| {
        v.get_bit(varbit::CAMERA_DOWN_KEY.id() as u16, false)
    })?;
    anyhow::ensure!(
        down_first == 34,
        "E was not installed on camera-down: {down_first}"
    );
    // Camera-up steals the same E: winner takes it, loser is cleared to 255.
    bind(
        &mut ui,
        &mut game,
        &mut clock,
        controls_settings::CAMERA_UP_KEYBIND,
        69,
    )?;
    let up = clock.with(&mut game, |v| {
        v.get_bit(varbit::CAMERA_UP_KEY.id() as u16, false)
    })?;
    let down = clock.with(&mut game, |v| {
        v.get_bit(varbit::CAMERA_DOWN_KEY.id() as u16, false)
    })?;
    anyhow::ensure!(
        ui.diagnostics.errors.is_empty(),
        "Controls conflict edit failed: {:?}",
        ui.diagnostics.errors
    );
    anyhow::ensure!(up == 34, "E was not installed on camera-up: {up}");
    anyhow::ensure!(
        down == 255,
        "conflict did not clear the losing slot (cache 8818/8822 clears to 255): up={up} down={down}"
    );
    // Consumption follows the cleared state: only camera-up holds E now, so
    // E pitches up. (Had the loser kept E, both opposed
    // interface keybinds would fire.)
    ui.state
        .interaction
        .actions
        .push_back(crate::ui_interaction::Action::Op {
            op: 1,
            parent: game_window::MODAL_CLOSE_BUTTON.packed(),
            child: 1,
            base: None,
        });
    game.cycle += 1;
    clock.tick(&mut game, &mut ui)?;
    let rotation = |ui: &Runtime| match &ui.engine.camera.cam2.position {
        Some(crate::ui_cam2::Position::Entity(p)) => Some(p.rotation),
        _ => None,
    };
    let hold_pitch = |ui: &mut Runtime,
                      game: &mut crate::client_game::ClientGame,
                      clock: &mut crate::test_support::SimClock|
     -> anyhow::Result<(f32, f32)> {
        let before =
            rotation(ui).ok_or_else(|| anyhow::anyhow!("camera entity owner not enabled"))?;
        ui.keyboard.key(69, 0, clock.0);
        for _ in 0..10 {
            game.cycle += 1;
            clock.tick(game, ui)?;
        }
        ui.keyboard.key(69, 1, clock.0);
        game.cycle += 1;
        clock.tick(game, ui)?;
        let after = rotation(ui).unwrap();
        Ok((
            crate::ui_cam2::pitch_of(&before),
            crate::ui_cam2::pitch_of(&after),
        ))
    };
    let (conf_before, conf_after) = hold_pitch(&mut ui, &mut game, &mut clock)?;
    let conflicted_delta = conf_after - conf_before;
    anyhow::ensure!(
        conflicted_delta > 0.1,
        "conflicted E did not reach camera-up (loser should be cleared): {conf_before} -> {conf_after}"
    );
    let mut frames = vec![];
    let now = clock.0 + 10000;
    for step in 0..20 {
        let state = &mut game.ui_variables;
        state.persistence.acknowledge();
        let frame = state
            .persistence
            .flush(&mut state.client, now + step * 1000, 0)?;
        if frame.is_empty() {
            break;
        }
        frames.push(frame);
    }
    anyhow::ensure!(
        !frames.is_empty(),
        "conflict did not queue persistent client values"
    );
    let value = match game
        .ui_variables
        .client
        .values
        .get(&varc::CAMERA_KEYBINDS.id())
    {
        Some(crate::ui_vars::Value::Int(v)) => *v,
        _ => anyhow::bail!("camera binding varc is not an int"),
    };
    // Conflict bytes [34,255,48,50]: winner E, cleared loser, untouched yaw.
    anyhow::ensure!(
        value == 842071842,
        "conflict varc bytes differ: {value} ({value:#x})"
    );
    std::fs::write(
        crate::test_support::proof_dir("settings").join("controls-conflict-proof.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"source":"real cache1444:26 onop8208 -> key E ->8823 -> varbit18959, then 1444:15 -> key E -> varbit18958 (8818/8822 clears loser 18959 to 255)","up":up,"down":down,"conflicted_delta":conflicted_delta,"varc":2868,"value":value,"frames":frames}),
        )?,
    )?;
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn settings_controls_modifier_guard_replay() -> anyhow::Result<()> {
    // Camera must not rotate while edit focus (the console/text dialog) is
    // open. Every interface keybind is gated on the console
    // (`hasKeybinds && !console_open`, in `ui_loop.rs`).
    // There is deliberately no *additional* edit-focus guard: key
    // capture is pure cache script state (client vars 2850/2851 via
    // 8208/8209/8817), and the keybind block has no capture check,
    // so this test records the console gate (present) and no other guard.
    // Modifier *binds* for camera are rejected cache-side instead:
    // 8823:10-18 returns "Modifier keys cannot be used for camera controls."
    // State-only, no pixel.
    let pack = crate::test_support::require_pack("client.config.js5");
    let rows: serde_json::Value =
        crate::test_support::replay_json("settings-navigation", "frames.json");
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    let mut clock = crate::test_support::SimClock::default();
    let mut ui = Runtime::new(pack.clone())?;
    ui.diagnostics.capture = true;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    ui.engine.platform.fullscreen_modes = Some(vec![FullscreenMode {
        width: 1280,
        height: 720,
        bit_depth: 24,
        refresh: 60,
    }]);
    apply(&mut ui, &mut game, &mut clock, &rows[0]["frames"], &pack)?;
    let controls = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["label"] == "controls-shortcut")
        .unwrap();
    apply(&mut ui, &mut game, &mut clock, &controls["frames"], &pack)?;
    ui.diagnostics.errors.clear();
    ui.diagnostics.executions.clear();
    // Bind camera-up to Q so the guarded key is a rebound (non-default) key.
    ui.state
        .interaction
        .actions
        .push_back(crate::ui_interaction::Action::Op {
            op: 1,
            parent: controls_settings::CAMERA_UP_KEYBIND.packed(),
            child: -1,
            base: None,
        });
    clock.tick(&mut game, &mut ui)?;
    ui.keyboard.key(81, 0, clock.0);
    clock.tick(&mut game, &mut ui)?;
    ui.keyboard.key(81, 1, clock.0);
    for _ in 0..4 {
        clock.tick(&mut game, &mut ui)?;
    }
    anyhow::ensure!(
        clock.with(&mut game, |v| v
            .get_bit(varbit::CAMERA_UP_KEY.id() as u16, false))?
            == 32,
        "Q was not installed as the camera binding"
    );
    ui.state
        .interaction
        .actions
        .push_back(crate::ui_interaction::Action::Op {
            op: 1,
            parent: game_window::MODAL_CLOSE_BUTTON.packed(),
            child: 1,
            base: None,
        });
    game.cycle += 1;
    clock.tick(&mut game, &mut ui)?;
    let rotation = |ui: &Runtime| match &ui.engine.camera.cam2.position {
        Some(crate::ui_cam2::Position::Entity(p)) => Some(p.rotation),
        _ => None,
    };
    let hold_pitch = |ui: &mut Runtime,
                      game: &mut crate::client_game::ClientGame,
                      clock: &mut crate::test_support::SimClock|
     -> anyhow::Result<(f32, f32)> {
        let before =
            rotation(ui).ok_or_else(|| anyhow::anyhow!("camera entity owner not enabled"))?;
        ui.keyboard.key(81, 0, clock.0);
        for _ in 0..10 {
            game.cycle += 1;
            clock.tick(game, ui)?;
        }
        ui.keyboard.key(81, 1, clock.0);
        game.cycle += 1;
        clock.tick(game, ui)?;
        let after = rotation(ui).unwrap();
        Ok((
            crate::ui_cam2::pitch_of(&before),
            crate::ui_cam2::pitch_of(&after),
        ))
    };
    // Baseline without edit focus: the rebound key reaches camera-up.
    let (open_before, open_after) = hold_pitch(&mut ui, &mut game, &mut clock)?;
    anyhow::ensure!(
        open_after > open_before + 0.1,
        "new key did not reach camera-up: {open_before} -> {open_after}"
    );
    // With the text dialog open, the same hold must not rotate the camera.
    ui.input.console_open = true;
    let (shut_before, shut_after) = hold_pitch(&mut ui, &mut game, &mut clock)?;
    ui.input.console_open = false;
    anyhow::ensure!(
        (shut_after - shut_before).abs() < 0.05,
        "camera rotated with edit focus open: {shut_before} -> {shut_after}"
    );
    anyhow::ensure!(
        ui.diagnostics.errors.is_empty(),
        "guard replay failed: {:?}",
        ui.diagnostics.errors
    );
    std::fs::write(
        crate::test_support::proof_dir("settings").join("controls-guard-proof.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"source":"rebound Q on camera-up, hold with console_open=false (rotates) vs true (guarded); gate in ui_loop.rs","open_before":open_before,"open_after":open_after,"shut_before":shut_before,"shut_after":shut_after,"errors":ui.diagnostics.errors}),
        )?,
    )?;
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn graphics_controls_change_saved_preferences_through_cache_hooks() -> anyhow::Result<()> {
    let pack = crate::test_support::require_pack("client.config.js5");
    let rows: serde_json::Value = crate::test_support::replay_json("settings-tabs", "frames.json");
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    let mut clock = crate::test_support::SimClock::default();
    let mut ui = Runtime::new(pack.clone())?;
    ui.diagnostics.capture = true;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    ui.engine.platform.fullscreen_modes = Some(vec![FullscreenMode {
        width: 1280,
        height: 720,
        bit_depth: 24,
        refresh: 60,
    }]);
    let path = &crate::test_support::proof_dir("graphics-settings").join("cache-preferences.dat");
    std::fs::create_dir_all(path.parent().unwrap())?;
    // An isolated canonical file lets this verify reload without touching the user's preferences.
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    game.ui_variables.queries.preferences.install(path.into());
    game.ui_variables.queries.preferences.anti_aliasing = true;
    game.ui_variables.queries.preferences.bloom = true;
    let varcs = crate::test_support::proof_dir("graphics-settings").join("cache-varcs.dat");
    if varcs.exists() {
        std::fs::remove_file(&varcs)?;
    }
    let definitions = game.game.inputs.bits.definitions.get(&2).unwrap();
    anyhow::ensure!(
        definitions[&varc::CUSTOM_CURSORS.id()].lifetime == Some(1),
        "cursor VarC987 must use its cache local lifetime"
    );
    game.ui_variables.persistence.install(
        varcs.clone(),
        &mut game.ui_variables.client,
        definitions,
    )?;
    clock.with(&mut game, |v| ui.initialize_cursors(v))?;
    anyhow::ensure!(
        ui.engine.menu.default_cursors == [36, 41],
        "cursor defaults did not come from cache1129"
    );

    for row in rows.as_array().unwrap() {
        apply(&mut ui, &mut game, &mut clock, &row["frames"], &pack)?;
        if row["tab"] == 2 {
            break;
        }
    }
    let mut proof = vec![];
    // Device capability is an explicit fixture here; the native/GPU checks
    // separately prove the real 2x/4x scene targets. Operate the cache dropdown.
    game.ui_variables.queries.preferences.anti_aliasing = true;
    for level in [2, 1, 0, 2] {
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: graphics_settings_panel::CONTROL_LIST.packed(),
                child: 7,
                base: None,
            });
        game.game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: game_window::DROPDOWN_LIST_OPTIONS.packed(),
                child: level * 2 + 1,
                base: None,
            });
        for _ in 0..4 {
            game.game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
        }
        if level > 0 {
            // Cache2700 opens the timed confirmation; only YES commits the
            // saved AA level. Use the actual UI hit route, not a preference write.
            let pos = [574, 467];
            ui.engine.platform.pending_mouse = pos;
            ui.input.click = Some(pos);
            ui.input.left_held = true;
            ui.input.event = Some(crate::ui_defaults::MouseEvent {
                pos,
                action: 0,
                count: 1,
            });
            game.game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
            ui.input.left_held = false;
            for _ in 0..3 {
                game.game.cycle += 1;
                clock.tick(&mut game, &mut ui)?;
            }
        }
        let p = &game.ui_variables.queries.preferences;
        anyhow::ensure!(
            p.options.get("antiAliasing2") == Some(level),
            "AA active dropdown level {level}: {:?}, errors {:?}",
            p.options.get("antiAliasing2"),
            ui.diagnostics.errors
        );
        anyhow::ensure!(
            p.options.get("antiAliasing") == Some(level),
            "AA saved dropdown level {level}, hook errors {:?}",
            ui.diagnostics.errors
        );
        anyhow::ensure!(
            crate::client_options::ClientOptions::load(path, p.options.profile)
                .get("antiAliasing2")
                == Some(level),
            "AA reload level {level}"
        );
        anyhow::ensure!(
            ui.diagnostics.errors.is_empty(),
            "AA hook errors {:?}",
            ui.diagnostics.errors
        );
        proof.push(serde_json::json!({"control":"antiAliasing","active":level,"saved":level,"reloadedActive":level}));
    }
    for timeout in [false, true] {
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: graphics_settings_panel::CONTROL_LIST.packed(),
                child: 7,
                base: None,
            });
        game.game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: game_window::DROPDOWN_LIST_OPTIONS.packed(),
                child: 3,
                base: None,
            });
        for _ in 0..4 {
            game.game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
        }
        anyhow::ensure!(
            game.ui_variables
                .queries
                .preferences
                .options
                .get("antiAliasing2")
                == Some(1),
            "AA preview not applied"
        );
        if timeout {
            game.game.cycle += 751;
        } else {
            let pos = [706, 467];
            ui.engine.platform.pending_mouse = pos;
            ui.input.left_held = true;
            ui.input.click = Some(pos);
            ui.input.event = Some(crate::ui_defaults::MouseEvent {
                pos,
                action: 0,
                count: 1,
            });
        }
        game.game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
        ui.input.left_held = false;
        for _ in 0..3 {
            game.game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
        }
        let p = &game.ui_variables.queries.preferences;
        anyhow::ensure!(
            p.options.get("antiAliasing2") == Some(2) && p.options.get("antiAliasing") == Some(2),
            "AA revert timeout={timeout} active {:?}, saved {:?}, errors {:?}",
            p.options.get("antiAliasing2"),
            p.options.get("antiAliasing"),
            ui.diagnostics.errors
        );
        anyhow::ensure!(
            ui.diagnostics.errors.is_empty(),
            "AA revert errors {:?}",
            ui.diagnostics.errors
        );
        if timeout {
            let line = ui
                .engine
                .messages
                .history
                .get_by_uid(ui.engine.messages.history.last_uid())
                .expect("timeout chat message");
            anyhow::ensure!(
                line.message == "The requested change has been cancelled.",
                "timeout message {}",
                line.message
            );
        }
        proof.push(serde_json::json!({"control":"antiAliasing","revert":if timeout{"timeout"}else{"reject"},"active":2,"saved":2}));
    }
    for (child, field) in [
        (18, "bloom"),
        (14, "customCursors"),
        (11, "flickeringEffects"),
        (10, "groundDecoration"),
        (12, "characterShadows"),
        (13, "fog"),
        (16, "textures"),
        (15, "groundBlending"),
    ] {
        ui.diagnostics.errors.clear();
        let before = game
            .ui_variables
            .queries
            .preferences
            .options
            .get(field)
            .unwrap();
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: graphics_settings_panel::CONTROL_LIST.packed(),
                child,
                base: None,
            });
        for _ in 0..4 {
            game.game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
        }
        let after = game
            .ui_variables
            .queries
            .preferences
            .options
            .get(field)
            .unwrap();
        proof.push(serde_json::json!({"field":field,"before":before,"after":after,"errors":ui.diagnostics.errors,"components":components(&ui)}));
        std::fs::write(
            crate::test_support::proof_dir("graphics-settings").join("cache-hooks.json"),
            serde_json::to_vec_pretty(&proof)?,
        )?;
        anyhow::ensure!(
            after == 1 - before,
            "{field} did not toggle: {before}->{after}, errors {:?}",
            ui.diagnostics.errors
        );
        anyhow::ensure!(
            ui.diagnostics.errors.is_empty(),
            "{field} hook errors {:?}",
            ui.diagnostics.errors
        );
        let loaded = crate::client_options::ClientOptions::load(
            path,
            game.ui_variables.queries.preferences.options.profile,
        );
        // The custom-cursors preference ignores the serialized value on
        // load. Preserve that startup reset.
        let reloaded = if field == "customCursors" { 1 } else { after };
        anyhow::ensure!(
            loaded.get(field) == Some(reloaded),
            "{field} reload differs from the recording"
        );
    }
    // CPU Usage is Enum201 row1. Select its second cache option, then verify
    // the canonical file and the exact precise-sleep request sequence.
    ui.state
        .interaction
        .actions
        .push_back(crate::ui_interaction::Action::Op {
            op: 1,
            parent: graphics_settings_panel::CONTROL_LIST.packed(),
            child: 1,
            base: None,
        });
    game.game.cycle += 1;
    clock.tick(&mut game, &mut ui)?;
    anyhow::ensure!(
        ui.store
            .get(game_window::DROPDOWN_LIST.packed(), 1)?
            .is_some(),
        "CPU option missing"
    );
    ui.state
        .interaction
        .actions
        .push_back(crate::ui_interaction::Action::Op {
            op: 1,
            parent: game_window::DROPDOWN_LIST_OPTIONS.packed(),
            child: 3,
            base: None,
        });
    for _ in 0..4 {
        game.game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
    }
    let cpu = game
        .ui_variables
        .queries
        .preferences
        .options
        .get("cpuUsage")
        .unwrap();
    anyhow::ensure!(cpu == 1, "CPU dropdown selected {cpu}");
    anyhow::ensure!(
        ui.diagnostics.errors.is_empty(),
        "CPU hook errors {:?}",
        ui.diagnostics.errors
    );
    anyhow::ensure!(
        crate::client_options::ClientOptions::load(
            path,
            game.ui_variables.queries.preferences.options.profile
        )
        .get("cpuUsage")
            == Some(1),
        "CPU setting not saved"
    );
    anyhow::ensure!(
        crate::graphics_runtime::cpu_sleeps(cpu) == [9, 1],
        "CPU render delay"
    );
    proof.push(serde_json::json!({"control":"cpuUsage","value":cpu,"sleep_millis":crate::graphics_runtime::cpu_sleeps(cpu)}));
    use native910::{vars::VarScope, vm::Value as VmValue};
    let layout = clock.with(&mut game, |v| {
        v.get(VarScope::Client, varc::CURRENT_LAYOUT.id() as u16, false)
    })?;
    // 2582 -> 709/8885 stores NIS layout sizing separately. Its canvas remains
    // Any; forcing screenSize here would contradict the unchanged cache scripts.
    for direct in [false, true] {
        if direct {
            // Explicit direct-canvas profile fixture (2257/2582 case5).
            // The selected preference still changes only through real onop hooks.
            clock.with(&mut game, |v| {
                v.set(
                    VarScope::Client,
                    varc::CURRENT_LAYOUT.id() as u16,
                    false,
                    VmValue::Int(5),
                )
            })?;
        }
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: graphics_settings_panel::CONTROL_LIST.packed(),
                child: 9,
                base: None,
            });
        game.game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
        anyhow::ensure!(
            ui.store
                .get(game_window::DROPDOWN_LIST.packed(), 2)?
                .is_some(),
            "dropdown selection missing"
        );
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: game_window::DROPDOWN_LIST_OPTIONS.packed(),
                child: 5,
                base: None,
            });
        for _ in 0..4 {
            game.game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
        }
        anyhow::ensure!(
            ui.diagnostics.errors.is_empty(),
            "dropdown errors {:?}",
            ui.diagnostics.errors
        );
        if direct {
            anyhow::ensure!(
                game.ui_variables
                    .queries
                    .preferences
                    .options
                    .get("screenSize")
                    == Some(2),
                "direct canvas size not applied"
            );
            anyhow::ensure!(
                crate::client_options::ClientOptions::load(
                    path,
                    game.ui_variables.queries.preferences.options.profile
                )
                .get("screenSize")
                    == Some(2),
                "screen size not saved"
            );
        } else {
            let selected = clock.with(&mut game, |v| match layout {
                VmValue::Int(6) => v.get_bit(varbit::SCREEN_SIZE_LAYOUT_6.id() as u16, false),
                VmValue::Int(7) => v.get_bit(varbit::SCREEN_SIZE_LAYOUT_7.id() as u16, false),
                VmValue::Int(8) => v.get_bit(varbit::SCREEN_SIZE_LAYOUT_8.id() as u16, false),
                VmValue::Int(12) => v.get_bit(varbit::SCREEN_SIZE_LAYOUT_12.id() as u16, false),
                VmValue::Int(13) => v.get_bit(varbit::SCREEN_SIZE_LAYOUT_13.id() as u16, false),
                VmValue::Int(9 | 10) => match v.get(
                    VarScope::Client,
                    varc::SCREEN_SIZE_LAYOUT_9_10.id() as u16,
                    false,
                )? {
                    VmValue::Int(n) => Ok(n),
                    _ => anyhow::bail!("layout sizing type"),
                },
                _ => anyhow::bail!("unexpected NIS fixture layout {layout:?}"),
            })?;
            anyhow::ensure!(
                selected == 2,
                "NIS layout did not save screen size: {selected}"
            );
            anyhow::ensure!(
                game.ui_variables
                    .queries
                    .preferences
                    .options
                    .get("screenSize")
                    == Some(0),
                "NIS canvas should remain Any"
            );
        }
        proof.push(serde_json::json!({"control":"maxscreen","direct_canvas_profile":direct,"initial_layout":format!("{layout:?}"),"screenSize":game.ui_variables.queries.preferences.options.get("screenSize")}));
    }
    // Re-enable and disable through the control, then reload both stores.
    // Cache1129 promotes an uninitialized VarC987 to1 at fresh startup.
    for expected in [1, 0] {
        ui.state
            .interaction
            .actions
            .push_back(crate::ui_interaction::Action::Op {
                op: 1,
                parent: graphics_settings_panel::CONTROL_LIST.packed(),
                child: 14,
                base: None,
            });
        for _ in 0..4 {
            game.game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
        }
        anyhow::ensure!(
            game.ui_variables
                .queries
                .preferences
                .options
                .get("customCursors")
                == Some(expected),
            "cursor toggle did not select {expected}"
        );
    }
    game.ui_variables
        .persistence
        .save_local(&mut game.ui_variables.client, clock.0, true)?;
    let definitions = game.game.inputs.bits.definitions.get(&2).unwrap();
    let mut restored = crate::ui_vars::ClientVars::default();
    game.ui_variables
        .persistence
        .install(varcs, &mut restored, definitions)?;
    anyhow::ensure!(
        restored.values.get(&varc::CUSTOM_CURSORS.id()) == Some(&crate::ui_vars::Value::Int(0)),
        "cursor VarC was not saved"
    );
    game.ui_variables.client = restored;
    game.ui_variables.queries.preferences.install(path.into());
    anyhow::ensure!(
        game.ui_variables
            .queries
            .preferences
            .options
            .get("customCursors")
            == Some(1),
        "constructor reset differs"
    );
    clock.with(&mut game, |v| ui.initialize_cursors(v))?;
    anyhow::ensure!(
        game.ui_variables
            .queries
            .preferences
            .options
            .get("customCursors")
            == Some(0),
        "cache startup did not restore disabled cursor choice"
    );
    anyhow::ensure!(
        ui.diagnostics.errors.is_empty(),
        "cursor lifecycle hook errors {:?}",
        ui.diagnostics.errors
    );
    proof.push(serde_json::json!({"control":"customCursors","disabled":true,"localVarc":varc::CUSTOM_CURSORS.id(),"loadedPreferenceBeforeBootstrap":1,"effectiveAfterCacheBootstrap":0,"defaultCursors":ui.engine.menu.default_cursors}));
    std::fs::write(
        crate::test_support::proof_dir("graphics-settings").join("cache-hooks.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    Ok(())
}
