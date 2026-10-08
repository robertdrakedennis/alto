//! Multi-observer replay for authoritative appearance, visibility and movement.
//! The fixture is produced by the server integration harness; this module is
//! replayed by the registered opt-in test with the production frame decoder.

#[cfg(test)]
mod tests {
    use crate::{client_game::ClientGame, protocol910::live::Feed};
    use std::collections::BTreeMap;

    struct Row {
        label: String,
        pid: usize,
        frames: Vec<Vec<u8>>,
        expected: Vec<Expected>,
    }
    struct Expected {
        pid: usize,
        name: Option<String>,
        weapon: i32,
        wear: Vec<i32>,
        bas: i32,
        x: i32,
        z: i32,
        face: Option<i32>,
    }
    fn rows(value: serde_json::Value) -> anyhow::Result<Vec<Row>> {
        let array = value
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("observer fixture must be an array"))?;
        array
            .iter()
            .map(|v| {
                let obj = v
                    .as_object()
                    .ok_or_else(|| anyhow::anyhow!("observer row must be an object"))?;
                let number = |key: &str| {
                    obj.get(key)
                        .and_then(serde_json::Value::as_i64)
                        .ok_or_else(|| anyhow::anyhow!("missing integer {key}"))
                };
                let frames = obj
                    .get("frames")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| anyhow::anyhow!("missing frames"))?
                    .iter()
                    .map(|f| {
                        f.as_array()
                            .ok_or_else(|| anyhow::anyhow!("frame must be array"))
                            .and_then(|a| {
                                a.iter()
                                    .map(|b| {
                                        b.as_u64()
                                            .and_then(|n| u8::try_from(n).ok())
                                            .ok_or_else(|| anyhow::anyhow!("invalid frame byte"))
                                    })
                                    .collect()
                            })
                    })
                    .collect::<anyhow::Result<Vec<Vec<u8>>>>()?;
                let expected = obj
                    .get("expected")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| anyhow::anyhow!("missing expected"))?
                    .iter()
                    .map(|e| {
                        let o = e
                            .as_object()
                            .ok_or_else(|| anyhow::anyhow!("expected must be object"))?;
                        let get = |k: &str| o.get(k).ok_or_else(|| anyhow::anyhow!("missing {k}"));
                        Ok(Expected {
                            pid: get("pid")?.as_u64().ok_or_else(|| anyhow::anyhow!("pid"))?
                                as usize,
                            name: get("name")?.as_str().map(str::to_string),
                            weapon: get("weapon")?
                                .as_i64()
                                .ok_or_else(|| anyhow::anyhow!("weapon"))?
                                as i32,
                            wear: get("wear")?
                                .as_array()
                                .ok_or_else(|| anyhow::anyhow!("wear"))?
                                .iter()
                                .map(|v| {
                                    v.as_i64()
                                        .map(|n| if n == -1 { 0 } else { n as i32 })
                                        .ok_or_else(|| anyhow::anyhow!("wear entry"))
                                })
                                .collect::<anyhow::Result<Vec<_>>>()?,
                            bas: get("bas")?.as_i64().ok_or_else(|| anyhow::anyhow!("bas"))? as i32,
                            x: get("x")?.as_i64().ok_or_else(|| anyhow::anyhow!("x"))? as i32,
                            z: get("z")?.as_i64().ok_or_else(|| anyhow::anyhow!("z"))? as i32,
                            face: o
                                .get("face")
                                .and_then(serde_json::Value::as_i64)
                                .map(|v| v as i32),
                        })
                    })
                    .collect::<anyhow::Result<Vec<Expected>>>()?;
                Ok(Row {
                    label: obj
                        .get("label")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("row")
                        .to_string(),
                    pid: number("pid")? as usize,
                    frames,
                    expected,
                })
            })
            .collect()
    }
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn equipment_observer_replay() -> anyhow::Result<()> {
        replay("equipment-observers")
    }

    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn navigation_observer_replay() -> anyhow::Result<()> {
        replay("facing-observers")
    }

    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn follow_observer_replay() -> anyhow::Result<()> {
        replay("follow-observers")
    }

    fn replay(scenario: &str) -> anyhow::Result<()> {
        let rows = rows(crate::test_support::replay_json(scenario, "frames.json"))?;
        let pack = crate::test_support::require_pack("client.config.js5");
        let mut games: BTreeMap<usize, ClientGame> = BTreeMap::new();
        for row in rows {
            let game = match games.entry(row.pid) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => entry.insert(
                    ClientGame::login(&pack, row.pid, Feed::default(), 910, true)?,
                ),
            };
            for raw in row.frames {
                let (frame, used) = crate::net::decode_frame(&raw)?
                    .ok_or_else(|| anyhow::anyhow!("incomplete observer frame"))?;
                anyhow::ensure!(used == raw.len(), "trailing observer frame bytes");
                let opcode = frame.opcode;
                let payload = &frame.payload;
                anyhow::ensure!(
                    game.runtime.feed.enqueue(opcode, payload),
                    "{}: unhandled opcode {opcode}",
                    row.label
                );
                game.apply_next(game.cycle as i64)
                    .map_err(|e| anyhow::anyhow!("{}: apply {e:?}", row.label))?
                    .ok_or_else(|| anyhow::anyhow!("{}: packet not applied", row.label))?;
                if game.runtime.map_request.is_some() {
                    let prepared = game.runtime.prepare_map(&pack)?;
                    game.runtime
                        .install_map(prepared)
                        .map_err(|e| anyhow::anyhow!("map: {e:?}"))?;
                }
            }
            // Exercise the production facing/animation owner after the packet
            // request, not just the pending direction in the protocol state.
            if row.expected.iter().any(|e| e.face.is_some()) {
                for _ in 0..100 {
                    game.cycle += 1;
                    game.update_actors()
                        .map_err(|e| anyhow::anyhow!("{}: actor tick {e:?}", row.label))?;
                }
            }
            let world = game
                .runtime
                .feed
                .state
                .world
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("{}: no world", row.label))?;
            let players = &game.runtime.feed.state.players.players;
            let visible: Vec<usize> = players
                .iter()
                .enumerate()
                .filter_map(|(pid, p)| p.as_ref().map(|_| pid))
                .collect();
            let mut expected: Vec<usize> = row.expected.iter().map(|e| e.pid).collect();
            expected.sort_unstable();
            anyhow::ensure!(
                visible == expected,
                "{}: visible pids {:?} != {:?}",
                row.label,
                visible,
                expected
            );
            for e in row.expected {
                let p = players
                    .get(e.pid)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| anyhow::anyhow!("{}: missing pid {}", row.label, e.pid))?;
                anyhow::ensure!(
                    p.appearance.name.as_deref() == e.name.as_deref(),
                    "{} pid {} name",
                    row.label,
                    e.pid
                );
                anyhow::ensure!(p.appearance.bas == e.bas, "{} pid {} bas", row.label, e.pid);
                if let Some(face) = e.face {
                    anyhow::ensure!(
                        p.angle & 16383 == face,
                        "{} pid {} actual facing {} != {}",
                        row.label,
                        e.pid,
                        p.angle & 16383,
                        face
                    );
                    anyhow::ensure!(
                        p.face_override == -1,
                        "{}: facing request was not consumed",
                        row.label
                    );
                }
                anyhow::ensure!(
                    p.appearance.model.as_ref().map(|m| m.kits.as_slice())
                        == Some(e.wear.as_slice()),
                    "{} pid {} full appearance",
                    row.label,
                    e.pid
                );
                anyhow::ensure!(
                    p.appearance
                        .model
                        .as_ref()
                        .and_then(|m| m.kits.get(3))
                        .copied()
                        .unwrap_or(-1)
                        == if e.weapon < 0 {
                            0
                        } else {
                            0x40000000 | e.weapon
                        },
                    "{} pid {} weapon",
                    row.label,
                    e.pid
                );
                anyhow::ensure!(
                    p.x[0] + world.base_x == e.x && p.z[0] + world.base_z == e.z,
                    "{} pid {} position",
                    row.label,
                    e.pid
                );
            }
        }
        Ok(())
    }
}
