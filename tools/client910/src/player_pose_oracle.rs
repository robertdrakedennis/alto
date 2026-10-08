//! Cache-backed complete player model body/overlay/dual/wear composition replay.
use crate::{
    animation_assets::AnimationAssets,
    cache::Pack,
    entities910::animation_state::Node,
    player_model::{Inputs, Models},
    player_pose,
    protocol910::{bas_types::Bas, sequence_types::Group},
};
use std::{collections::BTreeSet, io::Write};
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn composed_player_poses() -> anyhow::Result<()> {
    let root = rs910_core::test_support::repo_root();
    let scratch = rs910_core::test_support::frozen::Scratch::new("player-poses");
    let out = scratch.dir().to_path_buf();
    std::fs::create_dir_all(&out)?;
    let pack = Pack::open(root.join("server/data/pack"));
    let mut data = crate::entity_runtime::Inputs::load(&pack, true, false, 50)?;
    let mut b = Bas::default();
    let mut slots = vec![None; data.appearance.defaults.wear.positions.len()];
    for slot in [8, 9, 16] {
        slots[slot] = Some(vec![-37, 73, 129, 127, 511, 901]);
    }
    b.slot_transforms = Some(slots);
    data.bas.insert(900001, b);
    let tilted = Bas {
        tilt_x: 513,
        tilt_z: 731,
        tilt_scale_x: 125,
        tilt_scale_z: 250,
        roll_target: 77,
        pitch_target: 91,
        ..Default::default()
    };
    data.bas.insert(900002, tilted);
    let mut terrain = crate::protocol910::terrain::Terrain::new(104, 104).unwrap();
    for l in 0..4 {
        for x in 0..=104 {
            for z in 0..=104 {
                let i = terrain.point(l, x, z);
                terrain.heights[i] =
                    -(l as i32) * 960 + x as i32 * 7 - z as i32 * 11 + ((x * z) % 17) as i32 * 8;
            }
        }
    }
    for x in 0..104 {
        for z in 0..104 {
            let i = terrain.tile(1, x, z);
            terrain.tiles[i].flags = if (x + z) % 3 == 0 { 2 } else { 0 };
        }
    }
    let mut shadows = crate::player_shadow::Shadows::default();
    let mut shadow_result =
        std::io::BufWriter::new(std::fs::File::create(out.join("shadow-rust.bin"))?);
    let mut body_result =
        std::io::BufWriter::new(std::fs::File::create(out.join("body-rust.bin"))?);
    let mut cached = crate::entities910::appearance::CachedPacket {
        data: rs910_core::test_support::frozen::bytes("player-poses/server-appearance.bin"),
        consumed: 0,
    };
    let mut actor = crate::entities910::Player::default();
    crate::protocol910::appearance::apply(&mut cached, &mut actor, &data.appearance.appearance)
        .map_err(|e| anyhow::anyhow!("appearance: {e:?}"))?;
    let original = actor.appearance.model.unwrap();
    let defaults = &data.appearance.defaults;
    let c = Inputs {
        items: &data.appearance.types.items,
        bases: &data.bas,
        wear: &defaults.wear,
        recolour: defaults.graphics.recolour.as_ref().unwrap(),
        retexture: defaults.graphics.retexture.as_ref().unwrap(),
    };
    let materials = crate::texture::MaterialStore::load(&pack)?;
    let billboards = crate::billboard::BillboardStore::load(&pack)?;
    let emitters = crate::particle::EmitterStore::load(&pack)?;
    let shadow_texture = if defaults.graphics.scalars.spotshadowtexture >= 0 {
        defaults.graphics.scalars.spotshadowtexture as i16
    } else {
        materials.iter().find(|(_, m)| !m.low_detail).unwrap().0 as i16
    };
    std::fs::write(
        out.join("shadow-material.txt"),
        format!("{shadow_texture}\n"),
    )?;
    println!(
        "Shadow material {shadow_texture}, alpha {}",
        defaults.graphics.scalars.spotshadowtexture_alpha
    );
    let mut models = Models::load(&pack, 0x37)?;
    let mut assets = AnimationAssets::load(&pack)?;
    let candidates: Vec<_> = assets
        .sequences
        .values()
        .filter(|s| s.frame_ids.as_ref().is_some_and(|f| f.len() > 1) && s.skeletal == -1)
        .cloned()
        .collect();
    // Pick cache sequences which actually move this player's vertices. An
    // arbitrary early sequence can target labels absent from the player model.
    let unposed = models
        .body(
            &pack,
            &original,
            None,
            &c,
            &crate::gpumodel::ModelStores {
                materials: &materials,
                billboards: &billboards,
                emitters: &emitters,
            },
            crate::player_model::BodyBuild {
                flags: 0x820,
                cache: true,
                last_key: &mut 0,
            },
        )?
        .unwrap();
    let mut classic = vec![];
    let mut classic_base = None;
    for s in candidates {
        let mut node = Node::default();
        node.set(s.id, 0, 0, &data.selection).unwrap();
        node.time = s.frames.as_ref().unwrap()[0] / 3;
        let pose = assets.actor_pose(&pack, &mut node, Default::default())?;
        if classic_base.is_some() && pose.base_id != classic_base {
            continue;
        }
        let mut posed = unposed.clone();
        posed.apply_animation(&pose.transforms);
        if posed.vx == unposed.vx && posed.vy == unposed.vy && posed.vz == unposed.vz {
            continue;
        }
        classic_base = pose.base_id;
        classic.push(s.id);
        if classic.len() == 4 {
            break;
        }
    }
    let skeletal: Vec<_> = assets
        .sequences
        .values()
        .filter(|s| s.skeletal != -1 && s.frame_ids.is_none())
        .take(4)
        .map(|s| s.id)
        .collect();
    anyhow::ensure!(
        classic.len() == 4 && skeletal.len() == 4,
        "animation format coverage"
    );
    println!(
        "Pose sequences: classic {classic:?}, skeletal {skeletal:?}; server BAS {}",
        original.bas
    );
    // Oracle geometry starts from identical cold base caches in the recording and in Rust.
    models = Models::load(&pack, 0x37)?;
    assets = AnimationAssets::load(&pack)?;
    let server = &data.bas[&original.bas];
    let server_idle = if server.readyanim != -1 {
        server.readyanim
    } else {
        server.extra_seq_ids.as_ref().unwrap()[0]
    };
    let mut pairs = vec![
        (server_idle, -1),
        (-1, server_idle),
        (classic[0], classic[1]),
        (classic[0], skeletal[1]),
        (skeletal[0], classic[1]),
        (skeletal[0], skeletal[1]),
        (classic[0], classic[0]),
    ];
    pairs.push((-1, -1));
    let mut requests = vec![];
    let mut result = std::io::BufWriter::new(std::fs::File::create(out.join("rust.bin"))?);
    let mut last = 0;
    let mut used_seq = BTreeSet::new();
    let mut count = 0_i32;
    for (main_id, walk_id) in pairs {
        for mask_mode in 0..3 {
            for extras in 0..4 {
                for phase in 0..3 {
                    let wear = phase != 0;
                    let mut a = original.clone();
                    if phase == 2 {
                        a.bas = 900001;
                    }
                    a.update_hash();
                    let mut angles = vec![-1; a.kits.len()];
                    if wear {
                        for slot in [8, 9, 16] {
                            angles[slot] = (phase * 4301 + slot as i32 * 997) & 16383;
                        }
                    }
                    let angle_slice = if wear { Some(angles.as_slice()) } else { None };
                    let yaw = (phase * 3101 + mask_mode * 511) & 16383;
                    let mut row = vec![
                        main_id,
                        walk_id,
                        mask_mode,
                        extras,
                        phase,
                        a.bas,
                        yaw,
                        wear as i32,
                    ];
                    row.extend(&angles);
                    let mut make =
                        |id: i32, extra: bool, primary: bool| -> anyhow::Result<Option<Node>> {
                            if id == -1 {
                                return Ok(None);
                            }
                            used_seq.insert(id);
                            let mut seq = data.animation.sequences.sequences[&id].clone();
                            seq.extra = extra;
                            if primary {
                                match mask_mode {
                                    1 => seq.blend = -1,
                                    2 => {
                                        seq.blend = 900001;
                                        assets.groups.insert(
                                            900001,
                                            Group {
                                                mask: Some((0..1024).map(|i| i % 3 == 0).collect()),
                                                ..Default::default()
                                            },
                                        );
                                    }
                                    _ => {}
                                }
                            }
                            assets.sequences.insert(id, seq.clone());
                            let mut n = Node::default();
                            n.set(id, 0, 0, &data.selection)
                                .map_err(|e| anyhow::anyhow!("node: {e:?}"))?;
                            n.time = if seq.skeletal != -1 {
                                if phase == 2 {
                                    10000
                                } else {
                                    phase * 7
                                }
                            } else {
                                phase * seq.frames.as_ref().unwrap()[0] / 3
                            };
                            Ok(Some(n))
                        };
                    let mut main = make(main_id, extras & 1 != 0, true)?;
                    let mut walk = make(walk_id, extras & 2 != 0, false)?;
                    let mut overlays = vec![None; a.kits.len()];
                    if phase != 0 {
                        overlays[8] = make(classic[2], extras & 1 != 0, false)?;
                        overlays[9] = make(classic[3], extras & 2 != 0, false)?;
                        overlays[16] = make(skeletal[2], true, false)?;
                    }
                    // Same-sequence nodes share the sequence type. Restore the
                    // explicit mask after both fixture nodes have been built.
                    if main_id == walk_id && main_id != -1 && mask_mode == 2 {
                        assets.sequences.get_mut(&main_id).unwrap().blend = 900001;
                    }
                    requests.extend((row.len() as i32).to_be_bytes());
                    for v in row {
                        requests.extend(v.to_be_bytes());
                    }
                    if count == 97 {
                        let ops = player_pose::dual(
                            &mut assets,
                            &pack,
                            main.as_mut().unwrap(),
                            walk.as_mut().unwrap(),
                        )?;
                        std::fs::write(
                            out.join("ops-rust.txt"),
                            ops.iter()
                                .map(|o| {
                                    format!(
                                        "{} {} {} {} {} {} {:?} {:?}\n",
                                        o.kind,
                                        o.mask,
                                        o.angle,
                                        o.normals as i32,
                                        o.direct_pivot as i32,
                                        o.labels.len(),
                                        o.value,
                                        o.labels
                                    )
                                })
                                .collect::<String>(),
                        )?;
                    }
                    let prepared = player_pose::prepare(
                        &mut assets,
                        &pack,
                        &mut overlays,
                        main.as_mut(),
                        walk.as_mut(),
                    )?;
                    let flags = 0x820 | prepared.flags | if wear { 0x20 } else { 0 };
                    let main_seq = main
                        .as_ref()
                        .and_then(|n| assets.sequences.get(&n.id()))
                        .cloned();
                    let mut model = models
                        .body(
                            &pack,
                            &a,
                            main_seq.as_ref(),
                            &c,
                            &crate::gpumodel::ModelStores {
                                materials: &materials,
                                billboards: &billboards,
                                emitters: &emitters,
                            },
                            crate::player_model::BodyBuild {
                                flags,
                                cache: true,
                                last_key: &mut last,
                            },
                        )?
                        .unwrap();
                    player_pose::apply(
                        &mut model,
                        prepared,
                        player_pose::WearPose {
                            bas: c.bases.get(&a.bas),
                            angles: angle_slice,
                            yaw,
                        },
                        &mut assets,
                        &pack,
                        player_pose::ActiveNodes {
                            main: main.as_mut(),
                            walk: walk.as_mut(),
                        },
                    )?;
                    let words = crate::player_model_oracle::words(&model);
                    result.write_all(&(words.len() as i32).to_be_bytes())?;
                    for w in words {
                        result.write_all(&w.to_be_bytes())?;
                    }
                    result.write_all(&last.to_be_bytes())?;
                    let mut e = crate::entities910::Player::default();
                    e.appearance.model = Some(a.clone());
                    e.appearance.bas = if extras & 1 != 0 { 900002 } else { a.bas };
                    e.actor.model_key = last;
                    e.angle = yaw;
                    e.level = count % 4;
                    e.size = 1 + count % 3;
                    e.fine_x = [25600., 25856.25, 512., 52992.][(count % 4) as usize];
                    e.fine_z = [25856.5, 25600., 512., 52992.][(count % 4) as usize];
                    e.motion.y = -137.75;
                    e.motion.roll[0] = phase * 193 - 193;
                    e.motion.pitch[0] = phase * 271 - 271;
                    e.tint = [-1, 3, 99, if count % 2 == 0 { 137 } else { -1 }, 7, 11];
                    e.animation.main = main.clone().unwrap_or_default();
                    e.animation.main.delay = extras & 1;
                    e.actor.walk.node = walk.clone().unwrap_or_default();
                    e.actor.walk.idle = extras & 2 != 0;
                    e.actor.wear_angles = angle_slice.map(<[i32]>::to_vec);
                    e.animation.overlays = overlays.clone();
                    let resources = crate::player_body::Resources {
                        pack: &pack,
                        types: Inputs {
                            items: c.items,
                            bases: c.bases,
                            wear: c.wear,
                            recolour: c.recolour,
                            retexture: c.retexture,
                        },
                        materials: &materials,
                        billboards: &billboards,
                        emitters: &emitters,
                        npcs: None,
                    };
                    let mut body = crate::player_body::build(
                        &mut models,
                        &mut assets,
                        &resources,
                        &mut e,
                        Some(&terrain),
                        crate::player_body::BodyRequest {
                            cycle: 6 + count % 7,
                            flags: 0x820,
                            use_idle: phase == 1,
                        },
                    )?
                    .unwrap();
                    let mut words = crate::player_model_oracle::words(&body.model);
                    words.extend([body.min_y, body.height]);
                    words.extend(body.ground);
                    words.extend(crate::player_body::tile_bounds(&e));
                    body_result.write_all(&(words.len() as i32).to_be_bytes())?;
                    for w in words {
                        body_result.write_all(&w.to_be_bytes())?;
                    }
                    body_result.write_all(&e.actor.model_key.to_be_bytes())?;
                    for shape in [
                        crate::player_shadow::Shape::Rings {
                            size: 1,
                            colours: [0, 0],
                            alpha: [160, 240],
                        },
                        crate::player_shadow::Shape::Rings {
                            size: 1 + count % 5,
                            colours: [0x2143, 0xeb6f],
                            alpha: [count % 256, 255 - count % 256],
                        },
                        crate::player_shadow::Shape::Texture {
                            material: shadow_texture,
                            alpha: (count % 4 * 63) as i8,
                        },
                    ] {
                        let node = crate::player_shadow::animation(&mut e);
                        let id = node.as_ref().map_or(-1, |n| n.id());
                        let shadow = shadows.build(
                            (&resources).into(),
                            &mut assets,
                            &mut body.model,
                            shape,
                            body.ground,
                            node,
                        )?;
                        let mut words = crate::player_model_oracle::words(&shadow);
                        words.push(id);
                        shadow_result.write_all(&(words.len() as i32).to_be_bytes())?;
                        for w in words {
                            shadow_result.write_all(&w.to_be_bytes())?;
                        }
                        shadow_result.write_all(&0i64.to_be_bytes())?;
                    }
                    last = e.actor.model_key;
                    count += 1;
                }
            }
        }
    }
    let mut queries =
        std::io::BufWriter::new(std::fs::File::create(out.join("height-queries.bin"))?);
    let mut heights = std::io::BufWriter::new(std::fs::File::create(out.join("height-rust.bin"))?);
    for x in [-1, 0, 511, 512, 1024, 25600, 25856, 52992, 53248] {
        for z in [-1, 0, 511, 512, 1024, 25600, 25856, 52992, 53248] {
            for reference in [[-1, 50], [0, 0], [50, 50], [51, 50], [104, 50]] {
                for level in 0..4 {
                    for present in [false, true] {
                        for v in [x, z, reference[0], reference[1], level, present as i32] {
                            queries.write_all(&v.to_be_bytes())?;
                        }
                        let h = crate::protocol910::terrain::footprint_height(
                            if present { Some(&terrain) } else { None },
                            x,
                            z,
                            reference,
                            level,
                        )
                        .unwrap();
                        heights.write_all(&h.to_be_bytes())?;
                    }
                }
            }
        }
    }
    let mut file = std::io::BufWriter::new(std::fs::File::create(out.join("requests.bin"))?);
    file.write_all(&count.to_be_bytes())?;
    file.write_all(&requests)?;
    for writer in [
        &mut file,
        &mut queries,
        &mut heights,
        &mut result,
        &mut body_result,
        &mut shadow_result,
    ] {
        writer.flush()?;
    }
    std::fs::write(
        out.join("sequence-ids.txt"),
        format!(
            "{}\n",
            classic
                .iter()
                .chain(&skeletal)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        ),
    )?;
    println!("Rust composed bodies: {count}");
    scratch.finish(
        "player-poses",
        &[
            ("rust.bin", "recording"),
            ("body-rust.bin", "body"),
            ("height-rust.bin", "height"),
            ("shadow-rust.bin", "shadow"),
            ("ops-rust.txt", "ops"),
        ],
    );
    Ok(())
}
