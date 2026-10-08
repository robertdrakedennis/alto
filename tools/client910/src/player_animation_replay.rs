//! Wave11 server animation-mask replay through the real 910 Game/actor path.

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use std::collections::HashMap;

    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn server_animation_frames_replay_into_player_state() -> anyhow::Result<()> {
        let rows: Value = crate::test_support::replay_json("animation-observers", "frames.json");
        let root = crate::test_support::proof_dir("animation-observers");
        let pack = crate::test_support::require_pack("client.config.js5");
        let mut games = HashMap::new();
        let mut proof = Vec::new();
        let mut models = crate::player_model::Models::load(&pack, 0x37)?;
        let mut assets = crate::animation_assets::AnimationAssets::load(&pack)?;
        let materials = crate::texture::MaterialStore::load(&pack)?;
        let billboards = crate::billboard::BillboardStore::load(&pack)?;
        let emitters = crate::particle::EmitterStore::load(&pack)?;
        let mut prior_geometry = HashMap::new();
        for row in rows
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("animation rows must be an array"))?
        {
            let viewer = row["pid"].as_u64().unwrap_or(1) as usize;
            let game = games.entry(viewer).or_insert_with(|| {
                crate::client_game::ClientGame::login(
                    &pack,
                    viewer,
                    crate::protocol910::live::Feed::default(),
                    910,
                    true,
                )
                .unwrap()
            });
            for raw in row["frames"].as_array().map_or(&[][..], |v| v.as_slice()) {
                let wire: Vec<u8> = raw
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap() as u8)
                    .collect();
                let (frame, used) = crate::net::decode_frame(&wire)?
                    .ok_or_else(|| anyhow::anyhow!("incomplete animation frame"))?;
                anyhow::ensure!(used == wire.len(), "trailing animation frame bytes");
                anyhow::ensure!(
                    game.game.runtime.feed.enqueue(frame.opcode, &frame.payload),
                    "animation frame was not a game packet"
                );
                game.apply_next(game.game.cycle as i64)
                    .map_err(|e| anyhow::anyhow!("animation apply: {e:?}"))?;
                if game.game.runtime.map_request.is_some() {
                    let map = game.game.runtime.prepare_map(&pack)?;
                    game.game
                        .runtime
                        .install_map(map)
                        .map_err(|e| anyhow::anyhow!("map install: {e:?}"))?;
                }
            }
            let expected = row["expected"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("animation expected"))?;
            let expected_ids: Vec<usize> = expected
                .iter()
                .map(|item| item["pid"].as_u64().unwrap_or(0) as usize)
                .collect();
            let visible_ids: Vec<usize> = game
                .game
                .runtime
                .feed
                .state
                .players
                .players
                .iter()
                .enumerate()
                .filter_map(|(pid, actor)| actor.as_ref().map(|_| pid))
                .collect();
            anyhow::ensure!(
                visible_ids == expected_ids,
                "{} visible players {:?} != {:?}",
                row["label"],
                visible_ids,
                expected_ids
            );
            for item in expected {
                let pid = item["pid"].as_u64().unwrap_or(0) as usize;
                let actor = game
                    .game
                    .runtime
                    .feed
                    .state
                    .players
                    .players
                    .get(pid)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| anyhow::anyhow!("expected player {pid}"))?;
                let modes = if item["modes"].is_null() {
                    None
                } else {
                    Some(
                        item["modes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_i64().unwrap() as i32)
                            .collect::<Vec<_>>(),
                    )
                };
                anyhow::ensure!(
                    actor.animation.modes == modes,
                    "{} modes before advance",
                    row["label"]
                );
                anyhow::ensure!(
                    actor.animation.main.delay == item["delay"].as_i64().unwrap_or(0) as i32,
                    "{} delay before advance actual={} expected={}",
                    row["label"],
                    actor.animation.main.delay,
                    item["delay"]
                );
            }
            for _ in 0..row["advance"].as_u64().unwrap_or(0) {
                game.game.cycle += 1;
                game.update_actors()
                    .map_err(|e| anyhow::anyhow!("actor tick: {e:?}"))?;
            }
            if matches!(row["label"].as_str(), Some("play" | "duplicate-no-resend")) {
                let defaults = &game.game.inputs.appearance.defaults;
                let resources = crate::player_body::Resources {
                    pack: &pack,
                    types: crate::player_model::Inputs {
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
                let actor = game.game.runtime.feed.state.players.players[1]
                    .as_mut()
                    .unwrap();
                let mut control = actor.clone();
                control.animation.modes = None;
                control.animation.main.sequence = None;
                let idle = actor.actor.scene.use_idle;
                let body = crate::player_body::build(
                    &mut models,
                    &mut assets,
                    &resources,
                    actor,
                    game.game.runtime.terrain.as_ref(),
                    crate::player_body::BodyRequest {
                        cycle: game.game.cycle,
                        flags: 2048,
                        use_idle: idle,
                    },
                )?
                .ok_or_else(|| anyhow::anyhow!("animated player body missing"))?;
                let still = crate::player_body::build(
                    &mut models,
                    &mut assets,
                    &resources,
                    &mut control,
                    game.game.runtime.terrain.as_ref(),
                    crate::player_body::BodyRequest {
                        cycle: game.game.cycle,
                        flags: 2048,
                        use_idle: idle,
                    },
                )?
                .ok_or_else(|| anyhow::anyhow!("control player body missing"))?;
                let coordinates = |m: &crate::gpumodel::GpuModel| {
                    m.vx.iter()
                        .chain(&m.vy)
                        .chain(&m.vz)
                        .flat_map(|v| v.to_be_bytes())
                        .collect::<Vec<_>>()
                };
                let actual = coordinates(&body.model);
                let without_main = coordinates(&still.model);
                anyhow::ensure!(
                    body.model.vertex_count > 0 && actual.len() == without_main.len(),
                    "body geometry shape"
                );
                anyhow::ensure!(
                    actual != without_main,
                    "main sequence did not deform the rendered body"
                );
                if let Some(prior) = prior_geometry.insert(viewer, actual.clone()) {
                    anyhow::ensure!(
                        actual != prior,
                        "animation did not change rendered vertices across actor cycles"
                    );
                }
                let label = row["label"].as_str().unwrap();
                std::fs::write(root.join(format!("body-{viewer}-{label}.bin")), actual)?;
                std::fs::write(
                    root.join(format!("body-{viewer}-{label}-control.bin")),
                    without_main,
                )?;
            }
            for item in expected {
                let pid = item["pid"].as_u64().unwrap_or(0) as usize;
                let actor = game
                    .game
                    .runtime
                    .feed
                    .state
                    .players
                    .players
                    .get(pid)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| anyhow::anyhow!("expected player {pid}"))?;
                if let Some(sequence) = item.get("afterSequence").and_then(|v| v.as_i64()) {
                    anyhow::ensure!(
                        actor.animation.main.id() == sequence as i32,
                        "{} post-advance sequence {} != {}",
                        row["label"],
                        actor.animation.main.id(),
                        sequence
                    );
                }
                proof.push(serde_json::json!({"label":row["label"],"viewerPid":viewer,"actorPid":pid,"modes":actor.animation.modes,"sequence":actor.animation.main.id(),"delay":actor.animation.main.delay,"frame":actor.animation.main.frame,"time":actor.animation.main.time}));
            }
        }
        std::fs::write(
            root.join("animation-ui-proof.json"),
            serde_json::to_vec_pretty(&proof)?,
        )?;
        Ok(())
    }
}
