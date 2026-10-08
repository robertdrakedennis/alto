//! Full cache-backed player menu replay from server login and promotion frames.
use crate::client_game::with_game;
use crate::ui_runtime::*;
use crate::ui_vars::Variables;
use crate::{actor_matrix::Matrix, player_body, player_model, protocol910::live::Feed};

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn player_pick_to_follow_wire_replay() -> anyhow::Result<()> {
    let rows = crate::test_support::replay_json("follow-observers", "menu-frames.json");
    let path = crate::test_support::proof_dir("follow-observers");
    let pack = crate::test_support::require_pack("client.config.js5");
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    ui.target.quiet = true;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, true)?;
    let mut millis = 10_000;
    fn tick(
        ui: &mut Runtime,
        game: &mut crate::client_game::ClientGame,
        millis: i64,
    ) -> anyhow::Result<()> {
        let mut now = || millis;
        ui.tick(&mut Variables {
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
        for raw in row["frames"].as_array().unwrap() {
            let wire: Vec<u8> = raw
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let (frame, n) = crate::net::decode_frame(&wire)?.unwrap();
            assert_eq!(n, wire.len());
            if game.game.runtime.feed.enqueue(frame.opcode, &frame.payload) {
                game.apply_next(millis)
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                if game.game.runtime.map_request.is_some() {
                    let map = game.game.runtime.prepare_map(&pack)?;
                    game.game
                        .runtime
                        .install_map(map)
                        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                }
            } else if let Some(event) =
                crate::session::parse_ui_event(frame.opcode, &frame.payload)?
            {
                with_game(&mut game, |v| ui.packet(v, &event))?;
            } else {
                anyhow::bail!("unhandled Follow replay opcode {}", frame.opcode);
            }
        }
    }
    assert_eq!(
        ui.engine.menu.player_ops[2]
            .as_ref()
            .unwrap()
            .name
            .as_deref(),
        Some("Follow")
    );
    for _ in 0..60 {
        millis += 20;
        game.game.cycle += 1;
        game.poll_vars(|| millis)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        game.update_actors().map_err(|e| anyhow::anyhow!("{e:?}"))?;
        tick(&mut ui, &mut game, millis)?;
    }
    let mut models = player_model::Models::load(&pack, 0x37)?;
    let mut assets = crate::animation_assets::AnimationAssets::load(&pack)?;
    let materials = crate::texture::MaterialStore::load(&pack)?;
    let billboards = crate::billboard::BillboardStore::load(&pack)?;
    let emitters = crate::particle::EmitterStore::load(&pack)?;
    let mut proof = vec![];
    for canvas in [[1280, 720], [1600, 900]] {
        ui.resize(canvas)?;
        tick(&mut ui, &mut game, millis)?;
        ui.paint(game.game.cycle, true, [0.05; 3])?;
        ui.paint(game.game.cycle, true, [0.05; 3])?;
        let (viewport, _) = ui.state.viewport.unwrap();
        let mut camera = crate::camera::SceneCamera::new([0; 3]);
        camera.cam2 = ui.engine.camera.cam2.frame();
        camera.viewport = (viewport[2], viewport[3]);
        let base = [
            game.game.runtime.map.base_x << 9,
            game.game.runtime.map.base_z << 9,
        ];
        let mut frame = crate::player_picking::Frame::new(
            &camera,
            base,
            game.game.runtime.terrain_generation,
            viewport,
        );
        let defaults = &game.game.inputs.appearance.defaults;
        let r = player_body::Resources {
            pack: &pack,
            types: player_model::Inputs {
                items: &game.game.inputs.appearance.types.items,
                bases: &game.game.inputs.bas,
                wear: &defaults.wear,
                recolour: defaults.graphics.recolour.as_ref().unwrap(),
                retexture: defaults.graphics.retexture.as_ref().unwrap(),
            },
            materials: &materials,
            billboards: &billboards,
            emitters: &emitters,
            npcs: None,
        };
        let e = game.game.runtime.feed.state.players.players[2]
            .as_mut()
            .expect("real promoted player");
        let idle = e.actor.scene.use_idle;
        let mut body = player_body::build(
            &mut models,
            &mut assets,
            &r,
            e,
            game.game.runtime.terrain.as_ref(),
            player_body::BodyRequest {
                cycle: game.game.cycle,
                flags: 2048,
                use_idle: idle,
            },
        )?
        .expect("cache-backed body");
        assert!(body.model.unique_count > 0);
        let raw = crate::player_picking::bounds(&mut body.model);
        let matrix = Matrix::actor(e.actor.rotation, [e.fine_x, e.motion.y, e.fine_z], -5.);
        let capsule = crate::scene_player_pick::screen_bounds(
            raw,
            body.model.horizontal_radius(),
            crate::camera::multiply(&matrix.entries(), &frame.vp),
            frame.projection,
            frame.screen,
        );
        frame.picks.push(crate::scene_player_pick::PickablePlayer {
            id: crate::scene_player_pick::PlayerPickId {
                pid: 2,
                generation: game.game.runtime.feed.state.players.generations[2],
            },
            bounds: crate::scene_player_pick::pick_bounds(raw, 0),
            screen_bounds: Some(capsule),
            projected_depth: 0,
            matrix: crate::camera::multiply(
                &Matrix::actor(e.actor.rotation, [e.fine_x, e.motion.y, e.fine_z], 0.).entries(),
                &frame.vp,
            ),
            active: true,
        });
        // The scene pickable list (kept sorted on insert):
        // the menu walks this ordered list, not
        // the raw pick vectors. The promoted player is the only drawn pickable
        // here (no scene graph), so it is the whole list.
        frame.order.push(crate::player_picking::PickRef::Player(
            frame.picks.len() - 1,
        ));
        // Exercise the same current-model rebuild used immediately before the live UI tick.
        let mut renderer = crate::player_renderer::PlayersRenderer::new(&pack)?;
        let mouse = [
            (capsule.a[0] + capsule.b[0]) / 2,
            (capsule.a[1] + capsule.b[1]) / 2,
        ];
        renderer.refresh_picks(&mut game, &materials, &mut frame, mouse, None)?;
        assert!(
            !crate::scene_player_pick::pick_players(&frame.picks, frame.screen, mouse, [0, 0])
                .is_empty(),
            "body not picked: {capsule:?}"
        );
        ui.engine.scene.player_picks = Some(frame.clone());
        ui.engine.platform.mouse = mouse;
        ui.input.click = None;
        tick(&mut ui, &mut game, millis)?;
        let entries: Vec<_> = ui
            .state
            .minimenu
            .entries
            .iter()
            .map(|&id| ui.state.minimenu.entry(id))
            .filter(|e| e.op == "Follow")
            .cloned()
            .collect();
        assert_eq!(entries.len(), 1, "menu {:?}", ui.input.scene_options);
        let entry = &entries[0];
        assert_eq!(entry.action, 2046);
        assert_eq!(entry.entity_id, 2);
        for ctrl in [false, true] {
            for k in ui
                .engine
                .configs
                .minimenu
                .as_ref()
                .unwrap()
                .ctrlrunning
                .iter()
                .flatten()
            {
                ui.keyboard.held[*k as usize] = ctrl;
            }
            ui.engine.outgoing.clear();
            ui.use_menu_option(entry, mouse[0], mouse[1], false);
            assert_eq!(ui.engine.outgoing, vec![76, 0, 2, 128 + u8::from(ctrl)]);
            if ctrl {
                assert_eq!(
                    ui.engine.outgoing,
                    std::fs::read(crate::test_support::replay_fixture(
                        "follow-observers",
                        "click.bin"
                    ))?
                );
            }
            assert_eq!(
                ui.engine.menu.cross,
                Cross {
                    x: mouse[0],
                    y: mouse[1],
                    mode: 2,
                    cycle: 0
                }
            );
        }
        let p = game.game.runtime.feed.state.players.players[2]
            .as_ref()
            .unwrap();
        assert_eq!(ui.engine.minimap.flag, Some([p.x[0], p.z[0]]));
        proof.push(serde_json::json!({"canvas":canvas,"viewport":viewport,"mouse":mouse,"vertices":body.model.unique_count,"target":entry.target,"action":entry.action,"wire":ui.engine.outgoing}));
        // Old geometry may not attach to a new player at the same wire index.
        game.game.runtime.feed.state.players.generations[2] += 1;
        tick(&mut ui, &mut game, millis)?;
        assert!(!ui.input.scene_options.iter().any(|o| o.op == "Follow"));
        game.game.runtime.feed.state.players.generations[2] -= 1;
        game.game.runtime.terrain_generation += 1;
        tick(&mut ui, &mut game, millis)?;
        assert!(!ui.input.scene_options.iter().any(|o| o.op == "Follow"));
        game.game.runtime.terrain_generation -= 1;
        let old = game.game.runtime.feed.state.players.players[2].take();
        tick(&mut ui, &mut game, millis)?;
        ui.engine.outgoing.clear();
        ui.use_menu_option(entry, mouse[0], mouse[1], false);
        assert!(ui.engine.outgoing.is_empty());
        game.game.runtime.feed.state.players.players[2] = old;
        ui.resize([canvas[0] + 1, canvas[1]])?;
        assert!(ui.engine.scene.player_picks.is_none());
    }
    std::fs::write(
        path.join("player-pick-proof.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    Ok(())
}
