//! Actual server Run state through the unmodified cached orb scripts.
use crate::ui_runtime::*;
use crate::ui_vars::Variables;
use rs910_symbols::component::minimap;
use rs910_symbols::{script, varc};
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn run_toggle_and_resource_cache_replay() -> anyhow::Result<()> {
    let rows = crate::test_support::replay_json("run-toggle", "frames.json");
    let root = crate::test_support::proof_dir("run-toggle");
    let pack = crate::test_support::require_pack("client.config.js5");
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    ui.target.quiet = true;
    ui.diagnostics.capture = true;
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    let mut millis = 10_000i64;
    fn clock<T>(
        game: &mut crate::client_game::ClientGame,
        millis: i64,
        f: impl FnOnce(&mut Variables<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut now = || millis;
        f(&mut Variables {
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
    let component = minimap::RUN_BUTTON.packed();
    let mut proof = vec![];
    for row in rows.as_array().unwrap() {
        let label = row["label"].as_str().unwrap();
        if row["click"].as_bool() == Some(true) {
            let c = ui.store.get(component, -1)?.expect("real run control");
            assert!(c.borrow().ops.as_ref().is_some_and(|ops| ops[0].is_some()));
            ui.engine.outgoing.clear();
            ui.state
                .interaction
                .actions
                .push_back(crate::ui_interaction::Action::Op {
                    op: 1,
                    parent: component,
                    child: -1,
                    base: Some(vec![]),
                });
            clock(&mut game, millis, |v| ui.tick(v))?;
            assert_eq!(
                ui.engine.outgoing,
                std::fs::read(crate::test_support::replay_fixture(
                    "run-toggle",
                    "click.bin"
                ))?,
                "{label} actual onop/trailer"
            );
        }
        for raw in row["frames"].as_array().unwrap() {
            let wire: Vec<u8> = raw
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
                let stamp = ui.state.life.cycles.redraw;
                clock(&mut game, millis, |v| ui.packet(v, &event))?;
                if matches!(
                    event,
                    crate::session::UiEvent::RunEnergy { .. }
                        | crate::session::UiEvent::RunWeight { .. }
                ) {
                    assert_eq!(ui.state.life.cycles.misc, stamp);
                }
            } else {
                anyhow::bail!("unhandled Run replay opcode {}", frame.opcode);
            }
        }
        for _ in 0..40 {
            millis += 20;
            game.cycle += 1;
            game.poll_vars(|| millis)
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            clock(&mut game, millis, |v| ui.tick(v))?;
        }
        let enabled = row["expected"]["enabled"].as_bool().unwrap();
        let energy = row["expected"]["energy"].as_i64().unwrap() as i32;
        let control = ui.store.get(component, -1)?.unwrap();
        let tooltip = control.borrow().ops.as_ref().unwrap()[0]
            .as_ref()
            .map(|v| String::from_utf16_lossy(v))
            .unwrap();
        let energy_text = ui
            .store
            .get(minimap::RUN_ENERGY_TEXT.packed(), -1)?
            .unwrap()
            .borrow()
            .f
            .text
            .clone()
            .unwrap();
        let text = String::from_utf16_lossy(&energy_text);
        assert_eq!(
            tooltip,
            if enabled {
                "Turn run mode off"
            } else {
                "Turn run mode on"
            },
            "{label}"
        );
        assert_eq!(text, format!("{energy}%"), "{label}");
        assert_eq!(game.ui_variables.queries.run_energy, energy);
        assert_eq!(
            game.ui_variables.client.values.get(&varc::RUN_ENABLED.id()),
            Some(&crate::ui_vars::Value::Int(i32::from(enabled))),
            "{label}"
        );
        let graphic = ui
            .store
            .get(minimap::RUN_ORB.packed(), -1)?
            .unwrap()
            .borrow()
            .f
            .graphic;
        assert_eq!(graphic, if enabled { 18818 } else { 18819 }, "{label}");
        if label == "weight-wire" {
            assert_eq!(game.ui_variables.queries.run_weight, -12);
        }
        proof.push(serde_json::json!({"label":label,"energy":energy,"text":text,"tooltip":tooltip,"graphic":graphic}));
    }
    // Equal packets still stamp the real misc hook. Multiple packets in one
    // redraw cycle produce one callback, and quiet later cycles produce none.
    let count = |ui: &Runtime| {
        ui.diagnostics
            .executions
            .iter()
            .filter(|v| v["id"] == script::RUN_ORB_UPDATE.id())
            .count()
    };
    let before = count(&ui);
    for _ in 0..2 {
        clock(&mut game, millis, |v| {
            ui.packet(v, &crate::session::UiEvent::RunEnergy { value: 100 })
        })?;
    }
    clock(&mut game, millis, |v| ui.tick(v))?;
    assert_eq!(count(&ui), before + 1);
    clock(&mut game, millis, |v| ui.tick(v))?;
    assert_eq!(count(&ui), before + 1);
    let failures: Vec<_> = ui
        .diagnostics
        .executions
        .iter()
        .filter(|v| v["ok"] == false)
        .collect();
    assert!(failures.is_empty(), "cache failures {failures:?}");
    std::fs::write(
        root.join("run-ui-proof.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    Ok(())
}
