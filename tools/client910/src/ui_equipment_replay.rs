//! Real server login and equip/remove packets through the retained cache UI.
use rs910_symbols::component::{hero_loadout_stats as stats, hero_window, worn_equipment};
use rs910_symbols::{interface, inv, obj, script, ComponentId};

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn equipment_session_replay() -> anyhow::Result<()> {
    use crate::{protocol910::live::Feed, ui_runtime::Runtime};
    let bytes = std::fs::read(crate::test_support::replay_fixture(
        "equipment-session",
        "frames.bin",
    ))?;
    let pack = crate::test_support::require_pack("client.config.js5");
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([1280, 720])?;
    ui.diagnostics.capture = true;
    ui.engine.account.logged_in_members = true;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, true)?;
    let mut clock = crate::test_support::SimClock::default();
    let (mut at, mut frames, mut equipped) = (0usize, 0usize, false);
    let mut opened = false;
    while at < bytes.len() {
        let (frame, used) = crate::net::decode_frame(&bytes[at..])?
            .ok_or_else(|| anyhow::anyhow!("incomplete frame"))?;
        at += used;
        let opcode = frame.opcode;
        let payload = &frame.payload;
        if game.runtime.feed.enqueue(opcode, payload) {
            game.apply_next(frames as i64)
                .map_err(|e| anyhow::anyhow!("frame {frames} opcode {opcode}: {e:?}"))?;
            if game.runtime.map_request.is_some() {
                let prepared = game.runtime.prepare_map(&pack)?;
                game.runtime
                    .install_map(prepared)
                    .map_err(|e| anyhow::anyhow!("map installation: {e:?}"))?;
            }
        } else if let Some(event) = crate::session::parse_ui_event(opcode, payload)? {
            clock.with(&mut game, |vars| ui.packet(vars, &event))?;
        } else {
            anyhow::bail!("unhandled equipment session opcode {opcode}");
        }
        frames += 1;
        // Transmits run after the packet batch in production. Here each complete
        // inventory update is a batch boundary so the equipped intermediate state
        // is verified before the following Remove click clears it again.
        if opcode == crate::proto::server::UPDATE_INV_PARTIAL {
            game.poll_vars(|| clock.0)
                .map_err(|e| anyhow::anyhow!("varp poll {e:?}"))?;
            if !opened {
                // Let the ordinary login/transmit hooks establish layout mode
                // before the user requests a window (8785 rejects busy layout).
                for _ in 0..10 {
                    game.cycle += 1;
                    clock.tick(&mut game, &mut ui)?;
                }
                // The normal window toggle procedure, invoked as a user action.
                // Script8159 resolves window3 through enum7716/struct21286 and
                // opens it through 8162/8311, preserving the cached layout.
                let event = crate::session::UiEvent::RunScript(crate::session::RunClientScript {
                    script_id: script::TOGGLE_WINDOW.id(),
                    args: vec![
                        crate::session::ScriptArg::Int(3),
                        crate::session::ScriptArg::Int(0),
                    ],
                });
                clock.with(&mut game, |vars| ui.packet(vars, &event))?;
                opened = true;
            }
            game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;

            if ui
                .engine
                .inv_cache
                .slot_type(inv::WORN_EQUIPMENT.id(), 3, false)
                == obj::BRONZE_DAGGER.id()
            {
                let maybe_child = ui.store.get(worn_equipment::SLOTS.packed(), 3)?;
                let child = maybe_child.ok_or_else(|| {
                    anyhow::anyhow!(
                        "worn child 3 missing; hook errors {:?}",
                        ui.diagnostics.errors
                    )
                })?;
                let child = child.borrow();
                anyhow::ensure!(
                    child.f.invobject == obj::BRONZE_DAGGER.id(),
                    "worn UI object {}",
                    child.f.invobject
                );
                let op = child
                    .ops
                    .as_ref()
                    .and_then(|ops| ops.first())
                    .and_then(Option::as_ref);
                anyhow::ensure!(
                    op.map(|s| String::from_utf16_lossy(s)).as_deref() == Some("Remove"),
                    "worn Remove operation {op:?}"
                );
                drop(child);
                if !equipped {
                    ui.engine.outgoing.clear();
                    ui.state
                        .interaction
                        .actions
                        .push_back(crate::ui_interaction::Action::Op {
                            op: 1,
                            parent: worn_equipment::SLOTS.packed(),
                            child: 3,
                            base: None,
                        });
                    game.cycle += 1;
                    clock.tick(&mut game, &mut ui)?;
                    let expected = std::fs::read(crate::test_support::replay_fixture(
                        "equipment-session",
                        "remove-click.bin",
                    ))?;
                    anyhow::ensure!(
                        ui.engine
                            .outgoing
                            .windows(expected.len())
                            .any(|frame| frame == expected),
                        "Remove did not emit the exact server-accepted IF_BUTTON1 trailer: {:?}",
                        ui.engine.outgoing
                    );
                }
                equipped = true;
            }
        }
    }
    let model = ui
        .store
        .get(worn_equipment::PLAYER_MODEL.packed(), 0)?
        .ok_or_else(|| anyhow::anyhow!("local model component missing"))?;
    anyhow::ensure!(
        model.borrow().f.modelkind == 5 && model.borrow().f.model == 1,
        "model binding must use the real local PID"
    );
    anyhow::ensure!(
        equipped,
        "equipped intermediate state never rendered into retained UI"
    );
    anyhow::ensure!(
        ui.engine
            .inv_cache
            .slot_type(inv::WORN_EQUIPMENT.id(), 3, false)
            == -1,
        "removed worn slot"
    );
    anyhow::ensure!(
        ui.engine.inv_cache.slot_type(inv::BACKPACK.id(), 0, false) == obj::BRONZE_DAGGER.id(),
        "returned backpack item"
    );
    let child = ui.store.get(worn_equipment::SLOTS.packed(), 3)?.unwrap();
    anyhow::ensure!(child.borrow().f.invobject == -1, "cleared worn child");
    let failures: Vec<_> = ui
        .diagnostics
        .executions
        .iter()
        .filter(|e| e["ok"] == false)
        .collect();
    anyhow::ensure!(failures.is_empty(), "unexpected hook failures {failures:?}");
    anyhow::ensure!(
        ui.diagnostics.missing.is_empty(),
        "missing hooks {:?}",
        ui.diagnostics.missing
    );
    println!("PASS {frames} real server frames: cached worn UI equip/remove and backpack mirrors");
    Ok(())
}

#[test]
#[ignore = "gpu: renders through ui_model_gpu::Capture (desktop GPU); run with --ignored"]
fn equipment_preview_renders_and_animates() -> anyhow::Result<()> {
    use crate::{protocol910::live::Feed, ui_runtime::Runtime};
    let rows = crate::test_support::replay_json("equipment-preview", "frames.json");
    let path = crate::test_support::proof_dir("equipment-preview");
    let pack = crate::test_support::require_pack("client.config.js5");
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([1280, 720])?;
    ui.diagnostics.capture = true;
    ui.engine.account.logged_in_members = true;
    ui.target.quiet = true;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, true)?;
    let mut clock = crate::test_support::SimClock::default();
    let mut capture = crate::ui_model_gpu::Capture::new([1280, 720])?;
    let mut aa_capture = crate::ui_model_gpu::Capture::new([1280, 720])?;
    aa_capture.set_samples(4);
    let mut aa_proof = vec![];
    let mut signatures = std::collections::BTreeMap::new();
    let mut proof = vec![];
    for row in rows.as_array().unwrap() {
        let label = row["label"].as_str().unwrap();
        for wire in row["frames"].as_array().unwrap() {
            let wire: Vec<_> = wire
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let (frame, n) = crate::net::decode_frame(&wire)?.unwrap();
            assert_eq!(n, wire.len());
            if game.runtime.feed.enqueue(frame.opcode, &frame.payload) {
                game.apply_next(i64::from(game.cycle))
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                if game.runtime.map_request.is_some() {
                    let map = game.runtime.prepare_map(&pack)?;
                    game.runtime
                        .install_map(map)
                        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                }
            } else if let Some(event) =
                crate::session::parse_ui_event(frame.opcode, &frame.payload)?
            {
                clock.with(&mut game, |v| ui.packet(v, &event))?;
            } else {
                anyhow::bail!("unexpected opcode {}", frame.opcode)
            }
        }
        game.poll_vars(|| clock.0)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        for _ in 0..10 {
            game.cycle += 1;
            clock.tick(&mut game, &mut ui)?;
        }
        if label == "login" {
            let event = crate::session::UiEvent::RunScript(crate::session::RunClientScript {
                script_id: script::TOGGLE_WINDOW.id(),
                args: vec![
                    crate::session::ScriptArg::Int(3),
                    crate::session::ScriptArg::Int(0),
                ],
            });
            clock.with(&mut game, |v| ui.packet(v, &event))?;
            // The cache intentionally hides the player in the compact 199px
            // equipment panel. Use its ordinary window-resize procedure; its
            // onresize hook decides when the preview fits.
            let resize = crate::session::UiEvent::RunScript(crate::session::RunClientScript {
                script_id: script::RESIZE_WINDOW.id(),
                args: vec![800, 160, 470, 500, 3]
                    .into_iter()
                    .map(crate::session::ScriptArg::Int)
                    .collect(),
            });
            clock.with(&mut game, |v| ui.packet(v, &resize))?;
            for _ in 0..3 {
                game.cycle += 1;
                clock.tick(&mut game, &mut ui)?;
            }
        }
        game.cycle += 1;
        clock.tick(&mut game, &mut ui)?;
        if label == "login" {
            let _ = ui.paint(game.cycle, true, [0.05; 3])?;
        }
        let output = ui.paint(game.cycle, true, [0.05; 3])?;
        if output.models.is_empty() {
            for group in [interface::WORN_EQUIPMENT.id(), interface::GAME_WINDOW.id()] {
                if let Some(iface) = ui.store.interfaces.get(&group) {
                    for c in iface.borrow().components.borrow().iter().flatten() {
                        let c = c.borrow();
                        if group == interface::WORN_EQUIPMENT.id()
                            || [184, 185, 186].contains(&(c.f.parentlayer & 65535))
                        {
                            eprintln!(
                                "preview-debug group={group} fields={:?} children={:?} sorted={:?}",
                                c.f,
                                c.children.as_ref().map(|a| a.borrow().len()),
                                c.sorted.as_ref().map(|a| a.borrow().len())
                            );
                        }
                    }
                }
            }
            std::fs::write(
                path.join("preview-hooks.json"),
                serde_json::to_vec_pretty(&ui.diagnostics.executions)?,
            )?;
        }
        anyhow::ensure!(
            output.models.len() == 1,
            "{label}: {} model draws; calls {:?}; errors {:?}; gaps {:?}",
            output.models.len(),
            ui.target.calls,
            ui.diagnostics.errors,
            ui.target.missing
        );
        let model = &output.models[0];
        let model_clip = model.clip;
        let signature = model.model.position_stream();
        let vertices = signature.len();
        signatures.insert(label.to_string(), signature);
        let component = ui
            .store
            .get(worn_equipment::PLAYER_MODEL.packed(), 0)?
            .unwrap();
        let node = component
            .borrow()
            .model_animator
            .as_ref()
            .unwrap()
            .node
            .clone();
        let image = capture.render(output, &path.join(format!("preview-{label}.png")))?;
        let antialiased = aa_capture.render(
            ui.paint(game.cycle, true, [0.05; 3])?,
            &path.join(format!("preview-{label}-aa4.png")),
        )?;
        let mut changed = 0;
        for (index, (one, four)) in image
            .chunks_exact(4)
            .zip(antialiased.chunks_exact(4))
            .enumerate()
        {
            let x = (index % 1280) as i32;
            let y = (index / 1280) as i32;
            if x < model_clip[0] || x >= model_clip[2] || y < model_clip[1] || y >= model_clip[3] {
                anyhow::ensure!(
                    one == four,
                    "AA changed interface pixel outside model clip at {x},{y}"
                );
            } else if one[..3] != four[..3] {
                changed += 1;
            }
        }
        anyhow::ensure!(changed > 0, "4x AA did not change model edge pixels");
        aa_proof.push(serde_json::json!({"label":label,"modelClip":model_clip,"changedModelPixels":changed,"outsideModelClipChanges":0}));
        std::fs::write(
            path.join("ui-model-aa.json"),
            serde_json::to_vec_pretty(&aa_proof)?,
        )?;
        proof.push(serde_json::json!({"label":label,"vertices":vertices,"sequence":node.id(),"frame":node.frame,"time":node.time,"weapon":row["weapon"]}));
        if label == "dagger" {
            for _ in 0..75 {
                game.cycle += 1;
                clock.tick(&mut game, &mut ui)?;
            }
            let output = ui.paint(game.cycle, true, [0.05; 3])?;
            assert_ne!(
                output.models[0].model.position_stream(),
                signatures["dagger"],
                "animation must deform real vertices"
            );
            let animated = capture.render(output, &path.join("preview-dagger-animated.png"))?;
            let changed = image
                .chunks_exact(4)
                .zip(animated.chunks_exact(4))
                .filter(|(a, b)| a[..3] != b[..3])
                .count();
            anyhow::ensure!(
                changed > 10,
                "preview animation did not reach pixels ({changed})"
            );
            proof.push(serde_json::json!({"label":"dagger-animated","changed_pixels":changed}));
        }
    }
    // Cache ondrag callback: first sample captures the starting mouse X,
    // second sample rotates through the same property commands used in-game.
    ui.engine.platform.mouse = [1050, 430];
    let drag = |args: Vec<i32>| {
        crate::session::UiEvent::RunScript(crate::session::RunClientScript {
            script_id: script::MODEL_PREVIEW_DRAG.id(),
            args: args
                .into_iter()
                .map(crate::session::ScriptArg::Int)
                .collect(),
        })
    };
    clock.with(&mut game, |v| {
        ui.packet(
            v,
            &drag(vec![
                worn_equipment::PLAYER_MODEL_DRAG.packed(),
                worn_equipment::PLAYER_MODEL.packed(),
                0,
                -1,
                1,
                0,
                0,
                0,
                0,
            ]),
        )
    })?;
    ui.engine.platform.mouse = [1010, 430];
    clock.with(&mut game, |v| {
        ui.packet(
            v,
            &drag(vec![
                worn_equipment::PLAYER_MODEL_DRAG.packed(),
                worn_equipment::PLAYER_MODEL.packed(),
                0,
                -1,
                0,
                0,
                0,
                380,
                1050,
            ]),
        )
    })?;
    let component = ui
        .store
        .get(worn_equipment::PLAYER_MODEL.packed(), 0)?
        .unwrap();
    assert_eq!(component.borrow().f.modelangle_y, 200);
    let output = ui.paint(game.cycle, true, [0.05; 3])?;
    let mut hidpi = crate::ui_model_gpu::Capture::new([2560, 1440])?;
    hidpi.set_samples(4);
    hidpi.render(output, &path.join("preview-rotated-2x.png"))?;
    proof.push(serde_json::json!({"label":"rotated-2x","angle_y":200,"canvas":[1280,720],"target":[2560,1440]}));
    // The server's real two-handed equip packet must drive cache8483/8484.
    // No synthetic varc injection: this verifies the production stance feed.
    assert_eq!(component.borrow().f.modelanim, 7047);
    proof.push(serde_json::json!({"label":"server-preview-stance","bas":2574,"sequence":7047}));
    assert_ne!(
        signatures["backpack"].len(),
        signatures["dagger"].len(),
        "equipped geometry must change"
    );
    assert_ne!(
        signatures["dagger"].len(),
        signatures["two-handed"].len(),
        "two-handed geometry must change"
    );
    let failures: Vec<_> = ui
        .diagnostics
        .executions
        .iter()
        .filter(|e| e["ok"] == false)
        .collect();
    anyhow::ensure!(failures.is_empty(), "cache hook failures {failures:?}");
    anyhow::ensure!(
        !ui.target.missing.keys().any(|k| k.starts_with("Model")),
        "model renderer gaps {:?}",
        ui.target.missing
    );
    std::fs::write(
        path.join("preview-proof.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    eprintln!(
        "actual server equipment previews: {}",
        serde_json::to_string(&proof)?
    );
    Ok(())
}

#[test]
#[ignore = "gpu: renders through ui_model_gpu::Capture (desktop GPU); run with --ignored"]
fn equipment_stats_cache_replay() -> anyhow::Result<()> {
    loadout_replay("loadout-stats", "Stats")
}

#[test]
#[ignore = "gpu: renders through ui_model_gpu::Capture (desktop GPU); run with --ignored"]
fn equipment_armour_cache_replay() -> anyhow::Result<()> {
    loadout_replay("loadout-armour", "Armour")
}

fn loadout_replay(scenario: &str, character: &str) -> anyhow::Result<()> {
    use crate::{protocol910::live::Feed, ui_runtime::Runtime};
    let rows = crate::test_support::replay_json(scenario, "frames.json");
    let path = crate::test_support::proof_dir(scenario);
    let pack = crate::test_support::require_pack("client.config.js5");
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([1280, 720])?;
    ui.diagnostics.capture = true;
    ui.engine.account.logged_in_members = true;
    ui.target.quiet = true;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, true)?;
    let mut capture = crate::ui_model_gpu::Capture::new([1280, 720])?;
    let mut proof = vec![];
    let mut vertices = std::collections::BTreeMap::new();
    let mut millis = 10_000i64;
    fn with_clock<T>(
        game: &mut crate::client_game::ClientGame,
        millis: i64,
        f: impl FnOnce(&mut crate::ui_vars::Variables<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut now = || millis;
        f(&mut crate::ui_vars::Variables {
            cycle: game.game.cycle,
            definitions: &game.game.inputs.bits,
            state: &mut game.ui_variables,
            player: game.game.runtime.feed.state.varps.as_mut(),
            active_player: None,
            active_npc: None,
            now: &mut now,
            probe: None,
            varp_transmit: crate::ui_loop::Counter {
                num: game.game.runtime.varp_transmit_num,
                ids: game.game.runtime.varp_transmitted,
            },
            scene: crate::ui_cam2::SceneInput::new(
                &game.game.runtime.map,
                &game.game.runtime.feed.state.players,
                game.game.runtime.terrain.as_ref(),
                game.game.runtime.terrain_generation,
            ),
        })
    }
    for row in rows.as_array().unwrap() {
        let label = row["label"].as_str().unwrap();
        for wire in row["frames"].as_array().unwrap() {
            let wire: Vec<u8> = wire
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let (frame, n) = crate::net::decode_frame(&wire)?.unwrap();
            assert_eq!(n, wire.len());
            if game.runtime.feed.enqueue(frame.opcode, &frame.payload) {
                game.apply_next(millis)
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                if game.runtime.map_request.is_some() {
                    let map = game.runtime.prepare_map(&pack)?;
                    game.runtime
                        .install_map(map)
                        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                }
            } else if let Some(event) =
                crate::session::parse_ui_event(frame.opcode, &frame.payload)?
            {
                with_clock(&mut game, millis, |v| ui.packet(v, &event))?;
            } else {
                anyhow::bail!("unhandled opcode {}", frame.opcode);
            }
        }
        game.poll_vars(|| millis)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        for _ in 0..40 {
            millis += 20;
            game.cycle += 1;
            game.poll_vars(|| millis)
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            with_clock(&mut game, millis, |v| ui.tick(v))?;
        }
        if label == "login" {
            let c = ui
                .store
                .get(worn_equipment::STATS_BUTTON.packed(), 0)?
                .expect("stats control");
            assert_eq!(
                c.borrow().ops.as_ref().unwrap()[0]
                    .as_ref()
                    .map(|s| String::from_utf16_lossy(s))
                    .as_deref(),
                Some("Worn Equipment Stats")
            );
            let event = crate::session::UiEvent::RunScript(crate::session::RunClientScript {
                script_id: script::TOGGLE_WINDOW.id(),
                args: vec![
                    crate::session::ScriptArg::Int(3),
                    crate::session::ScriptArg::Int(0),
                ],
            });
            with_clock(&mut game, millis, |v| ui.packet(v, &event))?;
            ui.engine.outgoing.clear();
            ui.state
                .interaction
                .actions
                .push_back(crate::ui_interaction::Action::Op {
                    op: 1,
                    parent: worn_equipment::STATS_BUTTON.packed(),
                    child: 0,
                    base: Some(vec![]),
                });
            with_clock(&mut game, millis, |v| ui.tick(v))?;
            assert_eq!(
                ui.engine.outgoing,
                std::fs::read(crate::test_support::replay_fixture(scenario, "click.bin"))?,
                "real Stats control wire request"
            );
            continue;
        }
        let failures: Vec<_> = ui
            .diagnostics
            .executions
            .iter()
            .filter(|e| e["ok"] == false)
            .collect();
        anyhow::ensure!(
            failures.is_empty(),
            "{label}: cache hook failures {failures:?}"
        );
        let mut texts = serde_json::Map::new();
        // The stat fields the proof records; the unnamed ones are only recorded.
        let other = |child: u16| ComponentId::new(interface::HERO_LOADOUT_STATS.id() as u16, child);
        for field in [
            stats::MAIN_HAND_DAMAGE,
            stats::MAIN_HAND_ACCURACY,
            other(55),
            other(59),
            stats::ABILITY_DAMAGE,
            stats::ARMOUR,
            other(180),
            stats::PVM_REDUCTION,
            stats::PVP_REDUCTION,
            other(8),
            other(6),
            other(3),
            stats::PLAYER_NAME,
            stats::HEALTH,
            stats::PRAYER,
            stats::OFF_HAND_DAMAGE,
            stats::OFF_HAND_ACCURACY,
        ] {
            let id = field.packed();
            let c = ui.store.get(id, -1)?.expect("stat field");
            texts.insert(
                id.to_string(),
                serde_json::json!(String::from_utf16_lossy(
                    c.borrow().f.text.as_deref().unwrap_or(&[])
                )),
            );
        }
        eprintln!("{label} {texts:?}");
        let expected = match label {
            "open" | "returned-naked" => "2",
            "dagger" | "shield" | "dual" | "armoured-dual" => "40",
            "two-handed" | "armoured-two-handed" | "removed-armour" => "93",
            _ => unreachable!(),
        };
        let text = |field: ComponentId| texts[field.packed().to_string().as_str()].as_str();
        assert_eq!(text(stats::MAIN_HAND_DAMAGE), Some(expected));
        let ability = match label {
            "open" | "returned-naked" => "2",
            "dagger" | "shield" => "50",
            "two-handed" | "dual" | "armoured-dual" | "armoured-two-handed" | "removed-armour" => {
                "75"
            }
            _ => unreachable!(),
        };
        assert_eq!(text(stats::ABILITY_DAMAGE), Some(ability));
        assert_eq!(
            text(stats::MAIN_HAND_ACCURACY),
            Some(if matches!(label, "open" | "returned-naked") {
                "44"
            } else {
                "194"
            })
        );
        assert_eq!(
            text(stats::ARMOUR),
            Some(if label.starts_with("armoured-") {
                "156"
            } else if label == "shield" {
                "66"
            } else {
                "44"
            })
        );
        assert_eq!(
            text(stats::PVM_REDUCTION),
            Some(if label == "shield" || label.starts_with("armoured-") {
                "0.20%"
            } else {
                "0.10%"
            })
        );
        assert_eq!(
            text(stats::PVP_REDUCTION),
            Some(if label.starts_with("armoured-") {
                "0.25%"
            } else {
                "0.10%"
            })
        );
        let has_offhand = matches!(label, "dual" | "armoured-dual");
        // The cache retains text in hidden fields after removal; its disabled
        // off-hand overlay is the presentation contract, not clearing the text.
        assert_eq!(
            ui.store
                .get(stats::OFF_HAND_UNAVAILABLE.packed(), -1)?
                .unwrap()
                .borrow()
                .f
                .hide,
            has_offhand
        );
        if has_offhand {
            assert_eq!(text(stats::OFF_HAND_DAMAGE), Some("20"));
            assert_eq!(text(stats::OFF_HAND_ACCURACY), Some("194"));
        }
        assert_eq!(text(stats::PLAYER_NAME), Some(character));
        assert_eq!(text(stats::HEALTH), Some("1000/1000"));
        assert_eq!(text(stats::PRAYER), Some("10/10"));
        for (pane, width) in [
            (hero_window::BACKPACK_PANE, 228),
            (hero_window::STATS_PANE, 276),
            (hero_window::WORN_PANE, 228),
        ] {
            let c = ui.store.get(pane.packed(), -1)?.unwrap();
            let c = c.borrow();
            assert!(!c.f.hide);
            assert_eq!(c.f.width, width);
        }
        let _ = ui.paint(game.cycle, true, [0.05; 3])?;
        let output = ui.paint(game.cycle, true, [0.05; 3])?;
        anyhow::ensure!(output.models.len() == 1, "{label}: missing player preview");
        let count = output.models[0].model.position_stream().len();
        vertices.insert(label.to_string(), count);
        capture.render(output, &path.join(format!("loadout-{label}.png")))?;
        proof.push(serde_json::json!({"label":label,"fields":texts,"vertices":count,"offhand_available":has_offhand}));
        if label == "armoured-dual" {
            // Actual retained Off-hand tab operation, then restore Main-hand
            // before applying the next server equipment transaction.
            // The off-hand tab installs 8466(root,1); the main-hand tab (root,0).
            for (tab, offhand_tab) in [(stats::OFF_HAND_TAB, true), (stats::MAIN_HAND_TAB, false)] {
                ui.state
                    .interaction
                    .actions
                    .push_back(crate::ui_interaction::Action::Op {
                        op: 1,
                        parent: tab.packed(),
                        child: -1,
                        base: Some(vec![]),
                    });
                with_clock(&mut game, millis, |v| ui.tick(v))?;
                assert_eq!(
                    ui.store
                        .get(stats::OFF_HAND_DETAILS.packed(), -1)?
                        .unwrap()
                        .borrow()
                        .f
                        .hide,
                    !offhand_tab
                );
                assert_eq!(
                    ui.store
                        .get(stats::MAIN_HAND_DETAILS.packed(), -1)?
                        .unwrap()
                        .borrow()
                        .f
                        .hide,
                    offhand_tab
                );
                if offhand_tab {
                    let output = ui.paint(game.cycle, true, [0.05; 3])?;
                    capture.render(output, &path.join("loadout-armoured-offhand.png"))?;
                }
            }
        }
    }
    if character == "Armour" {
        assert_eq!(
            vertices["open"], vertices["returned-naked"],
            "removing armour restores the body"
        );
        assert_ne!(
            vertices["dual"], vertices["armoured-dual"],
            "armour changes rendered geometry"
        );
        assert_ne!(
            vertices["dagger"], vertices["dual"],
            "offhand reaches rendered geometry"
        );
    }
    ui.state
        .interaction
        .actions
        .push_back(crate::ui_interaction::Action::Op {
            op: 1,
            parent: stats::ABILITIES_TAB.packed(),
            child: -1,
            base: Some(vec![]),
        });
    with_clock(&mut game, millis, |v| ui.tick(v))?;
    let output = ui.paint(game.cycle, true, [0.05; 3])?;
    capture.render(output, &path.join("loadout-abilities.png"))?;
    std::fs::write(
        path.join("stats-ui-proof.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    Ok(())
}
