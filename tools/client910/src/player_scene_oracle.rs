//! Entity push and scene add/clear comparison against the recording.
use crate::{
    entities910::{
        appearance::Model,
        combat::{Bar, Combat},
        Player,
    },
    player_scene::{self, Settings},
    protocol910::{terrain::Terrain, Players},
    scene::{PrimaryRef, Scene},
};
use std::io::Write;
fn words(out: &mut impl Write, values: impl IntoIterator<Item = i32>) -> anyhow::Result<()> {
    for v in values {
        out.write_all(&v.to_be_bytes())?;
    }
    Ok(())
}
#[test]
fn temporary_player_scene() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("player-scene");
    let out = scratch.dir().to_path_buf();
    std::fs::create_dir_all(&out)?;
    let mut input = std::fs::File::create(out.join("input.bin"))?;
    let mut output = std::fs::File::create(out.join("rust.bin"))?;
    words(&mut input, [18])?;
    let mut total = 0;
    for scenario in 0..18 {
        let count = if scenario % 3 == 0 {
            24
        } else if scenario % 3 == 1 {
            60
        } else {
            220
        };
        let frames = 8;
        words(&mut input, [scenario, count, frames])?;
        let mut players = Players {
            high_indices: (1..=count as usize).collect(),
            ..Default::default()
        };
        if scenario % 2 != 0 {
            players.high_indices.reverse();
        }
        let mut scene = Scene::new(9, 4, 32, 32);
        scene.reset();
        scene.set_bridge(8, 8);
        let mut terrain = Terrain::new(32, 32).unwrap();
        for l in 0..4 {
            for x in 0..=32 {
                for z in 0..=32 {
                    let i = terrain.point(l, x, z);
                    terrain.heights[i] =
                        -960 * l as i32 + 7 * x as i32 - 11 * z as i32 + ((x * z) % 17) as i32 * 8;
                }
            }
        }
        for x in 0..32 {
            for z in 0..32 {
                let i = terrain.tile(1, x, z);
                terrain.tiles[i].flags = if (x + z) % 3 == 0 { 2 } else { 0 };
            }
        }
        for &id in &players.high_indices {
            let n = id as i32;
            let mut e = Player::default();
            if n % 13 != 0 {
                e.appearance.model = Some(Model {
                    bas: 0,
                    kits: vec![],
                    custom: vec![],
                    colours: [0; 10],
                    textures: [0; 10],
                    female: false,
                    npc: -1,
                    hash: 0,
                });
            }
            e.appearance.visibility = Some(if id == 1 {
                0
            } else {
                ((n + scenario) % 3) as i8
            });
            e.size = 1 + (n + scenario) % 3;
            e.level = (n + scenario) % 4;
            e.occlude_level = e.level;
            e.fine_x = ((6 + n % 7) * 512 + if e.size & 1 != 0 { 256 } else { 0 }) as f32;
            e.fine_z = ((6 + n % 5) * 512 + if e.size & 1 != 0 { 256 } else { 0 }) as f32;
            if n % 11 == 0 {
                e.fine_x += 61.25;
                e.fine_z -= 27.5;
            }
            if n % 23 == 0 {
                e.fine_x = -129.;
            }
            e.motion.y = -131.75;
            e.partner = (n + scenario) % 3;
            e.suppress_partner = n % 5 == 0;
            e.actor.walk.idle = n % 2 == 0;
            if n % 7 == 0 {
                e.combat = Some(Combat {
                    cursor: 0,
                    hits: vec![],
                    bars: vec![Bar {
                        id: 0,
                        updates: vec![],
                    }],
                });
            }
            e.actor.scene.force_show = n % 17 == 0;
            e.actor.scene.min_y = -500 - n;
            e.actor.scene.transparent = n % 2 == 0;
            e.forced[4] = 0;
            e.forced[5] = if n % 4 == 0 { 1 } else { 0 };
            e.forced[6] = 3;
            e.forced[7] = 6;
            words(
                &mut input,
                [
                    n,
                    e.appearance.model.is_some() as i32,
                    e.appearance.visibility.unwrap() as i32,
                    e.size,
                    e.level,
                    e.occlude_level,
                    e.fine_x.to_bits() as i32,
                    e.motion.y.to_bits() as i32,
                    e.fine_z.to_bits() as i32,
                    e.partner,
                    e.suppress_partner as i32,
                    e.actor.walk.idle as i32,
                    e.combat.is_some() as i32,
                    e.actor.scene.force_show as i32,
                    e.actor.scene.min_y,
                    e.actor.scene.transparent as i32,
                    e.forced[4],
                    e.forced[5],
                    e.forced[6],
                    e.forced[7],
                ],
            )?;
            players.players[id] = Some(e);
        }
        let heights = (0..4)
            .map(|l| {
                crate::floor::FloorHeights::new(
                    32,
                    32,
                    512,
                    terrain.heights[l * 33 * 33..(l + 1) * 33 * 33].to_vec(),
                )
            })
            .collect::<Vec<_>>();
        let mut draw = crate::draw::DrawState::new(16);
        let mut occlusion = crate::occlusion::Occlusion::new(&scene, &heights);
        for tick in 0..frames {
            if tick == 3 {
                for &id in &players.high_indices {
                    let e = players.players[id].as_mut().unwrap();
                    if id % 11 == 0 {
                        e.fine_x += 450.75;
                        e.fine_z += 27.5;
                    }
                }
            }
            if tick == 5 {
                players.players[1] = Some(Player {
                    appearance: players.players[1].as_ref().unwrap().appearance.clone(),
                    fine_x: 4352.,
                    fine_z: 4352.,
                    ..Player::default()
                });
            }
            let c = Settings {
                local: 1,
                idle_detail: scenario % 3,
                draw_order: scenario % 2,
                active_target: -3,
                hints: &[2, 3, 1],
                cutscene: false,
            };
            player_scene::insert(&mut players, &mut scene, Some(&terrain), tick, &c)?;
            words(&mut output, [scenario, tick, scene.temporary.len() as i32])?;
            for e in &scene.temporary {
                words(&mut output, [e.player as i32, e.level, e.occlude_level])?;
                words(&mut output, e.position.map(|v| v.to_bits() as i32))?;
                words(&mut output, e.bounds)?;
                words(&mut output, [e.overlay_height, e.transparent as i32])?;
            }
            for &id in &players.high_indices {
                let e = players.players[id].as_ref().unwrap();
                let s = &e.actor.scene;
                words(
                    &mut output,
                    [
                        id as i32,
                        s.priority,
                        s.deferred as i32,
                        s.use_idle as i32,
                        e.motion.y.to_bits() as i32,
                    ],
                )?;
                words(&mut output, s.bounds)?;
            }
            for l in 0..4 {
                for x in 0..32 {
                    for z in 0..32 {
                        let tile = scene.tile(l, x, z);
                        words(
                            &mut output,
                            [
                                tile.map_or(-1, |t| t.level as i32),
                                tile.map_or(0, |t| t.entities.len() as i32),
                            ],
                        )?;
                        if let Some(t) = tile {
                            for &r in &t.entities {
                                let PrimaryRef::Temporary(id) = r else {
                                    panic!("unexpected scenery fixture")
                                };
                                words(&mut output, [scene.temporary[id].player as i32])?;
                            }
                        }
                    }
                }
            }
            let a = [
                tick,
                5120,
                -4000,
                2048,
                (tick * 1703 + scenario * 991) & 16383,
                1700,
                1280,
                720,
                0,
                0,
                1280,
                720,
                50,
                30000,
                7,
                1,
                1,
                2,
                0,
            ];
            words(&mut input, a)?;
            let roof = (0..4)
                .map(|l| {
                    (0..32)
                        .map(|x| {
                            (0..32)
                                .map(|z| if (l + x + z + tick) % 5 == 0 { 7i8 } else { 0 })
                                .collect()
                        })
                        .collect()
                })
                .collect::<Vec<Vec<Vec<i8>>>>();
            let mut entities = vec![];
            crate::draw::append_temporary(&scene, &mut entities);
            draw.live_frame(
                crate::draw::PlannerScene {
                    scene: &scene,
                    heights: &heights,
                    entities: &entities,
                },
                crate::draw::DrawFrame::from_words(a),
                if tick % 2 == 0 { Some(&roof) } else { None },
                &mut occlusion,
                crate::draw::LiveInputs {
                    boxes: &[],
                    cam2: None,
                    underwater: None,
                },
            );
            for list in [
                &draw.plan.dispatch_opaque,
                &draw.plan.dispatch_transparent,
                &draw.plan.culled_updates,
            ] {
                words(&mut output, [list.len() as i32])?;
                for &id in list {
                    words(&mut output, [entities[id].loc_id])?;
                }
            }
            scene.clear_temporary();
            words(&mut output, [scene.temporary.len() as i32])?;
            for l in 0..4 {
                for x in 0..32 {
                    for z in 0..32 {
                        words(
                            &mut output,
                            [scene.tile(l, x, z).map_or(-1, |t| t.entities.len() as i32)],
                        )?;
                    }
                }
            }
            total += 1;
        }
    }
    println!("Rust temporary-player frames: {total}");
    scratch.finish("player-scene", &[("rust.bin", "recording")]);
    Ok(())
}
