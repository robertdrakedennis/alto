use crate::cutscene::*;
use anyhow::{Context, Result};
use std::collections::BTreeMap;

fn pack() -> crate::cache::Pack {
    crate::test_support::require_pack("client.cutscenes.js5")
}

/// Decode every file of the real `cutscenes` archive in load
/// order and report the inventory.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn decodes_every_cache_cutscene() -> Result<()> {
    let pack = pack();
    let index = pack.read_archive_index(ARCHIVE)?;
    let mut decoded = 0;
    let mut kinds = BTreeMap::<&'static str, usize>::new();
    let mut failures = Vec::new();
    let mut unsorted = 0;
    for &group in &index.group_id {
        let decoded_file = crate::anim::fetch_file(&pack, ARCHIVE, group)
            .and_then(|bytes| Definition::decode_counted(&bytes));
        match decoded_file {
            Ok((def, trailing)) => {
                assert_eq!(trailing, 0, "cutscene {group} trailing bytes");
                if std::env::var("CLIENT910_CUTSCENE_DUMP").ok() == Some(group.to_string()) {
                    println!("templates {:?}", def.templates);
                    println!("entities {:?}", def.entities);
                    println!("locations {:?}", def.locations);
                    for a in &def.actions {
                        println!("  {:5} {:?}", a.start_tick, a.kind);
                    }
                }
                if std::env::var_os("CLIENT910_CUTSCENE_LIST").is_some() {
                    println!(
                        "cutscene {group}: viewport {}x{} templates {} splines {} entities {} (players {}) locations {} routes {} actions {} subtitles {} last tick {}",
                        def.viewport_width,
                        def.viewport_height,
                        def.templates.len(),
                        def.splines.len(),
                        def.entities.len(),
                        def.entities.iter().filter(|e| e.npc_id < 0).count(),
                        def.locations.len(),
                        def.routes.len(),
                        def.actions.len(),
                        def.actions.iter().filter(|a| matches!(a.kind, ActionKind::Subtitle { .. })).count(),
                        def.actions.last().map_or(0, |a| a.start_tick)
                    );
                }
                decoded += 1;
                for action in &def.actions {
                    *kinds.entry(kind_name(&action.kind)).or_default() += 1;
                }
                if !def
                    .actions
                    .windows(2)
                    .all(|w| w[0].start_tick <= w[1].start_tick)
                {
                    unsorted += 1;
                }
            }
            Err(error) => failures.push(format!("{group}: {error:#}")),
        }
    }
    println!(
        "cutscenes: {decoded}/{} decoded ({unsorted} with non-monotonic start ticks); action kinds {kinds:?}",
        index.group_id.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(decoded > 0);
    Ok(())
}

/// The cutscene scene rebuild for a real cache cutscene: the
/// CPU region terrain and the renderer's scene build agree on every
/// height/flag (the same check `Game::verify_scene` applies before the
/// map is acknowledged).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn cutscene_map_terrain_matches_scene_build() -> Result<()> {
    let pack = pack();
    let def = Definition::load(&pack, 2)?;
    let land: std::collections::BTreeSet<i32> = pack
        .read_archive_index(crate::map::MAP_ARCHIVE)?
        .group_id
        .iter()
        .map(|&g| g as i32)
        .collect();
    let prior = crate::protocol910::rebuild_state::World {
        base_x: 3176,
        base_z: 3168,
        region_x: 3228 / 8,
        region_z: 3220 / 8,
        width: 104,
        height: 104,
        area: Some(0),
        last_kind: crate::protocol910::rebuild_state::Kind::Normal,
        npc_bits: 8,
        map_squares: vec![],
        groups: vec![],
        group_count: 0,
    };
    let (world, layout) = cutscene_world(&def, &prior, 64, &land)?;
    assert_eq!((world.base_x, world.base_z, world.width), (0, 0, 104));
    assert_eq!(world.group_count, 4, "{:?}", world.map_squares);
    let terrain = crate::entity_runtime::Runtime::load_region_terrain(&pack, &world, &layout)?;
    let flo = crate::flo::FloStore::load(&pack)?;
    let tables = crate::maploader::FloTables::from_store(&flo);
    let materials = crate::texture::MaterialStore::load(&pack)?;
    let locs = crate::config::LocStore::load(&pack)?;
    let built = crate::rebuild::rebuild_region_world(
        &pack,
        &tables,
        &materials,
        &locs,
        &world,
        &layout,
        &crate::rebuild::BuildPrefs::default(),
    )?;
    let mut compared = 0;
    let mut mismatches = Vec::new();
    for level in 0..4 {
        let floor = built.scene.normal[level].as_ref().context("scene floor")?;
        for x in 0..=104 {
            for z in 0..=104 {
                assert_eq!(
                    floor.heights.get_tile_height(x, z),
                    terrain.heights[terrain.point(level, x, z)],
                    "height {level}:{x}:{z}"
                );
                if x < 104 && z < 104 {
                    let (a, b) = (
                        built.flags.get(level, x, z),
                        terrain.tiles[terrain.tile(level, x, z)].flags,
                    );
                    if a != b {
                        mismatches.push((level, x, z, a, b));
                    }
                }
                compared += 1;
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} flag mismatches, first {:?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(40)]
    );
    println!(
        "cutscene 2 map: {compared} vertices agree, squares {:?}",
        world.map_squares
    );
    Ok(())
}

/// Region template copying on hand-checked rotations: a 2x1 source
/// strip lands at the destination cells the rotation loops write.
#[test]
fn region_templates_follow_the_rotation_loops() -> Result<()> {
    let template = |rotation| Template {
        src_level: 1,
        src_x: 3200,
        src_z: 3208,
        size_x: 2,
        size_z: 1,
        dest_level: 0,
        dest_x: 3,
        dest_z: 4,
        rotation,
    };
    let packed = |sx: i32, sz: i32, r: i32| (r << 1) + (sz << 3) + (1 << 24) + (sx << 14);
    let (x0, z0) = (3200 >> 3, 3208 >> 3);
    let chunks = 13;
    let at = |data: &[i32], x: usize, z: usize| data[x * chunks + z];
    for (rotation, cells) in [
        (0, [((3, 4), (x0, z0)), ((4, 4), (x0 + 1, z0))]),
        (1, [((3, 5), (x0, z0)), ((3, 4), (x0 + 1, z0))]),
        (2, [((4, 4), (x0, z0)), ((3, 4), (x0 + 1, z0))]),
        (3, [((3, 4), (x0, z0)), ((3, 5), (x0 + 1, z0))]),
    ] {
        let def = Definition {
            viewport_width: 0,
            viewport_height: 0,
            templates: vec![template(rotation)],
            splines: vec![],
            entities: vec![],
            locations: vec![],
            routes: vec![],
            actions: vec![],
        };
        let data = def.region_templates(104)?;
        for ((cx, cz), (sx, sz)) in cells {
            assert_eq!(
                at(&data, cx, cz),
                packed(sx, sz, rotation),
                "rotation {rotation} cell {cx},{cz}"
            );
        }
        assert_eq!(data.iter().filter(|&&v| v != -1).count(), 2);
    }
    Ok(())
}

/// The cutscene defaults (defaults group 8) decode its cancel binding: the
/// cache bytes are `1` (`cancelbinding`), a
/// type-1 key binding (key code then modifier mask) of key 13, no
/// modifiers, then `0`. Key 13 is Escape (the keyboard maps AWT key 27 to
/// 13).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn cutscene_defaults_decode_from_cache() -> Result<()> {
    let pack = pack();
    let bytes = crate::js5_fetch::fetch_file(&pack, "defaults", 8)?.expect("defaults group 8");
    assert_eq!(
        decode_defaults(&bytes)?,
        Some(crate::ui_defaults::Binding::Key {
            code: 13,
            modifiers: 0
        })
    );
    Ok(())
}

/// The whole game-update cutscene path for cache cutscene 2 on a
/// real `Game`: CUTSCENE packet state, load, the CUTSCENE rebuild request
/// and CPU install, state save, the action clock by `startTick`, actor
/// spawning/routes/animations, loc requests, camera/audio/fade requests,
/// then the finish action -> `finish(true)` -> `reset`.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn cutscene_clock_plays_cache_cutscene_to_finish() -> Result<()> {
    let pack = pack();
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        true,
    )?;
    game.runtime.feed.state.initialized = true;
    game.cycle = 100;
    game.begin_cutscene(2, 64, &[]);
    assert_eq!(game.cutscene.scene_state, 2);
    assert_eq!(game.cutscene.take_requests(), vec![UiRequest::CloseMenu]);
    // sceneState 2 -> load -> cutscene map rebuild -> sceneState 1.
    assert!(game.update_scene_state(&pack)?);
    assert_eq!(game.cutscene.scene_state, 1);
    assert!(game.cutscene.rebuild_type_cutscene);
    let request = game.runtime.map_request.as_ref().context("map request")?;
    assert_eq!((request.world.base_x, request.world.width), (0, 104));
    assert_eq!(
        request.world.last_kind,
        crate::protocol910::rebuild_state::Kind::Cutscene
    );
    let created = game.cutscene.take_requests();
    let sounds = created
        .iter()
        .filter(|r| matches!(r, UiRequest::SoundCreate { .. }))
        .count();
    assert_eq!(sounds, 13, "one Sound per SoundVorbis action");
    let prepared = game.runtime.prepare_map(&pack)?;
    game.runtime
        .install_map(prepared)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    // sceneState 1 -> 0 on the next update; tick-0 actions run at once.
    game.cycle = 101;
    assert!(!game.update_scene_state(&pack)?);
    assert_eq!(game.cutscene.scene_state, 0);
    assert_eq!(game.cutscene.start_cycle, 101);
    assert!(game.cutscene.camera_active);
    let requests = game.cutscene.take_requests();
    assert!(matches!(
        requests[0],
        UiRequest::SaveState {
            viewport: [640, 480]
        }
    ));
    let move_along = requests
        .iter()
        .find_map(|r| match r {
            UiRequest::CameraMoveAlong {
                splines,
                pos_keyframe,
                min_speed,
                ..
            } => Some((splines[0].len(), *pos_keyframe, *min_speed)),
            _ => None,
        })
        .context("CamMoveAlong request")?;
    let def = Definition::load(&pack, 2)?;
    assert_eq!(move_along, (def.splines[1].from.len() * 2, 1, 936));
    assert_eq!(
        game.cutscene.next_action, 18,
        "the 18 tick-0 actions executed"
    );
    assert_eq!(game.cutscene.fade.end, [0, 0, 0, 0]);
    assert_eq!(game.cutscene.npcs.slots, vec![0, 2, 3]);
    let benita = &game.cutscene.npcs.entities[&2];
    assert_eq!(
        (benita.type_id, benita.path.x[0], benita.path.z[0]),
        (rs910_symbols::npc::ABBESS_BENITA_BASEMENT.id(), 53, 26)
    );
    assert_eq!(benita.path.angle, 5803);
    assert_eq!(
        benita.path.animation.modes,
        Some(vec![rs910_symbols::seq::SISTER_SHOCKED.id(); 4])
    );
    // EntityRoute: teleport to the route's first waypoint, then the
    // reversed queue; this update's movement already consumed that
    // (reached) first target.
    let ripper = &game.cutscene.npcs.entities[&0];
    let route = &def.routes[1];
    assert_eq!(ripper.path.route_length, route.waypoints.len() - 1);
    assert_eq!(ripper.path.x[0], route.waypoints.last().unwrap() >> 16);
    assert_eq!(game.runtime.feed.state.zones.locations.len(), 9);
    // Advance the clock; startTick 60 deletes entity 0 and spawns 1.
    let mut cycle = 101;
    while game.cutscene.scene_state == 0 && cycle < 101 + 400 {
        cycle += 1;
        game.cycle = cycle;
        game.update_scene_state(&pack)?;
        if cycle == 101 + 60 {
            assert!(!game.cutscene.npcs.entities.contains_key(&0));
            assert!(game.cutscene.npcs.entities.contains_key(&1));
        }
    }
    // The finish action at startTick 330.
    assert_eq!(cycle, 101 + 330);
    assert_eq!(game.cutscene.scene_state, 4);
    assert!(!game.cutscene.camera_active);
    assert!(
        game.cutscene.definition.is_none(),
        "the cutscene manager reset"
    );
    let tail = game.cutscene.take_requests();
    assert!(tail.contains(&UiRequest::End { cutscene: 2 }));
    assert_eq!(tail.last(), Some(&UiRequest::Finished { completed: true }));
    assert!(tail.contains(&UiRequest::RestoreState));
    Ok(())
}

pub(super) fn kind_name(k: &ActionKind) -> &'static str {
    match k {
        ActionKind::EntityHitmark { .. } => "EntityHitmark",
        ActionKind::EntityMove { .. } => "EntityMove",
        ActionKind::Finish => "Finish",
        ActionKind::Sound31 { .. } => "Sound31",
        ActionKind::SetVar { .. } => "SetVar",
        ActionKind::SoundJingle { .. } => "SoundJingle",
        ActionKind::EntityRoute { .. } => "EntityRoute",
        ActionKind::TextCoord { .. } => "TextCoord",
        ActionKind::EntityLook { .. } => "EntityLook",
        ActionKind::ProjAnim { .. } => "ProjAnim",
        ActionKind::EntitySay { .. } => "EntitySay",
        ActionKind::EntityAnim { .. } => "EntityAnim",
        ActionKind::EntityDel { .. } => "EntityDel",
        ActionKind::LocCreate { .. } => "LocCreate",
        ActionKind::Fade { .. } => "Fade",
        ActionKind::LocDel { .. } => "LocDel",
        ActionKind::LocAnim { .. } => "LocAnim",
        ActionKind::SoundVorbis { .. } => "SoundVorbis",
        ActionKind::CamMove { .. } => "CamMove",
        ActionKind::EntitySpot { .. } => "EntitySpot",
        ActionKind::SoundSong { .. } => "SoundSong",
        ActionKind::MapAnim { .. } => "MapAnim",
        ActionKind::CamMoveAlong { .. } => "CamMoveAlong",
        ActionKind::Subtitle { .. } => "Subtitle",
    }
}
