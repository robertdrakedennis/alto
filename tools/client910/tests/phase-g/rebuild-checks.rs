#[allow(unused_imports)]
use crate::{entities910, protocol910};
use protocol910::{live::*, rebuild_state::*};
fn world() -> World {
    World {
        base_x: 3200,
        base_z: 3200,
        region_x: 406,
        region_z: 406,
        width: 104,
        height: 104,
        area: Some(0),
        last_kind: Kind::Normal,
        npc_bits: 0,
        map_squares: vec![],
        groups: vec![],
        group_count: 0,
    }
}
fn packet() -> Vec<u8> {
    // First step of the recorded login REBUILD.
    let s = std::fs::read_to_string(
        rs910_core::test_support::client_dir().join("fixtures/phase-g/rebuild-login.txt"),
    )
    .unwrap();
    let hex = s.lines().next().unwrap().split_whitespace().nth(1).unwrap();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}
#[test]
fn rebuild_errors_retain_front_packet_and_prior_state() {
    let w = world();
    let map = protocol910::Context {
        local: 1,
        base_x: 3200,
        base_z: 3200,
        width: 104,
        height: 104,
        bridges: vec![],
    };
    let groups = (0..20000).filter(|n| n % 3 != 0).collect();
    let sizes = Default::default();
    let c = Config {
        prior: &w,
        old_map: &map,
        appearance: None,
        login: true,
        land_groups: &groups,
        loc_sizes: &sizes,
        scene: None,
    };
    let good = packet();
    let mut cases = vec![vec![], good[..30].to_vec(), good[..good.len() - 1].to_vec()];
    let mut extra = good.clone();
    extra.push(0);
    cases.push(extra);
    let mut capacity = good.clone();
    let n = capacity.len();
    capacity[n - 5] = 128;
    cases.push(capacity);
    let mut area = good.clone();
    area[n - 4] = 99;
    cases.push(area);
    for bytes in cases {
        let mut f = Feed::default();
        f.enqueue(88, &bytes);
        f.enqueue(122, &[0]);
        let before = f.state.clone();
        assert!(f
            .apply_next(&Contexts {
                rebuild: Some(&c),
                player: None,
                npc: None,
                zone: None
            })
            .is_err());
        assert_eq!(f.state, before);
        assert_eq!(f.front().unwrap().payload, bytes);
        assert_eq!(f.pending_len(), 2);
        assert_eq!(f.drain_completed().count(), 0);
    }
    let mut f = Feed::default();
    f.enqueue(88, &good);
    let applied = f
        .apply_next(&Contexts {
            rebuild: Some(&c),
            player: None,
            npc: None,
            zone: None,
        })
        .unwrap()
        .unwrap();
    assert_eq!(applied.bytes, good.len());
    assert_eq!(applied.bit_pos, Some(36858));
    assert!(f.state.initialized);
    assert_eq!(f.state.world.as_ref().unwrap().base_z, 3208);
    assert_eq!(f.drain_completed().count(), 1);
}
#[test]
fn rebase_missing_loc_config_rolls_back_actor_shifts() {
    let mut s = State::default();
    s.players.players[1] = Some(entities910::Player::default());
    s.zones.locations.push(protocol910::zone_state::Location {
        level: 0,
        layer: 0,
        x: 50,
        z: 50,
        old_id: 0,
        old_shape: 0,
        old_angle: 0,
        id: 999,
        shape: 0,
        angle: 0,
        transform: None,
        custom: None,
        pending: true,
        remove: false,
    });
    let before = s.clone();
    assert!(rebase(
        &mut s,
        &Rebase {
            old_x: 3200,
            old_z: 3200,
            base_x: 3208,
            base_z: 3216,
            width: 104,
            height: 104,
            mode: 3,
            preserve_outside: false,
            loc_sizes: &Default::default(),
            scene: None,
        },
        Effects::default()
    )
    .is_err());
    assert_eq!(s, before);
}
#[test]
fn reentry_requires_cached_appearance_context() {
    let w = world();
    let map = protocol910::Context {
        local: 1,
        base_x: 3200,
        base_z: 3200,
        width: 104,
        height: 104,
        bridges: vec![],
    };
    let groups = Default::default();
    let sizes = Default::default();
    let c = Config {
        prior: &w,
        old_map: &map,
        appearance: None,
        login: true,
        land_groups: &groups,
        loc_sizes: &sizes,
        scene: None,
    };
    let mut s = State::default();
    s.players.appearances[1] = Some(entities910::appearance::CachedPacket {
        data: vec![0],
        consumed: 0,
    });
    let before = s.clone();
    assert!(matches!(
        decode_normal(&packet(), &s, &c),
        Err(protocol910::Error::UnsupportedContext(_))
    ));
    assert_eq!(s, before);
}
/// A reconnect the server resumes in place restarts only the player list:
/// from the server's player-positions block (here the block a recorded login
/// opens its rebuild with), while the map, npcs, zones and variables stay and
/// no npc keeps a target.
#[test]
fn resume_restarts_the_player_list_and_keeps_the_world() {
    let w = world();
    let map = protocol910::Context {
        local: 1,
        base_x: 3200,
        base_z: 3200,
        width: 104,
        height: 104,
        bridges: vec![],
    };
    let groups = (0..20000).filter(|n| n % 3 != 0).collect();
    let sizes = Default::default();
    let c = Config {
        prior: &w,
        old_map: &map,
        appearance: None,
        login: true,
        land_groups: &groups,
        loc_sizes: &sizes,
        scene: None,
    };
    let login = packet();
    let mut f = Feed::default();
    f.enqueue(88, &login);
    f.apply_next(&Contexts {
        rebuild: Some(&c),
        player: None,
        npc: None,
        zone: None,
    })
    .unwrap();
    // A session in progress: another player in view, an npc targeting one.
    let mut state = f.state.clone();
    state.players.players[7] = Some(entities910::Player::default());
    let mut npc = entities910::Npc::new([0; 4]);
    npc.path.target = 7;
    state.npcs.entities.insert(3, npc);
    let world = state.world.clone();
    let block = &login[..(30 + 18 * 2046usize).div_ceil(8)];
    let resumed = resume_players(block, &state, &map, None).unwrap();
    assert!(
        resumed.players.players[7].is_none(),
        "the player list restarts"
    );
    // The block names the local player's absolute tile (3250, 3250); the tile
    // is placed against the map base the caller passes.
    let local = resumed.players.players[1]
        .as_ref()
        .expect("the local player");
    assert_eq!((local.x[0], local.z[0]), (50, 50));
    assert_eq!(resumed.npcs.entities[&3].path.target, -1);
    assert_eq!(resumed.world, world, "the map stays");
    assert!(resumed.initialized);
    // A block of the wrong size is refused and nothing changes.
    assert!(resume_players(&block[..block.len() - 1], &state, &map, None).is_err());
    assert!(resume_players(&[block, &[0]].concat(), &state, &map, None).is_err());
    assert!(state.players.players[7].is_some());
}
/// The NPC info packet that adds one NPC, `index`, of `type_id` (view of 5 bits): no
/// NPCs held yet, the new NPC, then the end marker.
fn npc_info_adding(index: usize, type_id: usize) -> Vec<u8> {
    let mut bits = String::new();
    let mut put = |value: usize, width: usize| bits.push_str(&format!("{value:0width$b}"));
    put(0, 8); // NPCs held
    put(index, 15);
    put(0, 2); // level
    put(0, 5); // z offset
    put(0, 1); // no mask
    put(type_id, 15);
    put(1, 1); // teleport
    put(0, 3); // facing
    put(0, 5); // x offset
    put(32767, 15); // end
    while !bits.len().is_multiple_of(8) {
        bits.push('0');
    }
    (0..bits.len())
        .step_by(8)
        .map(|i| u8::from_str_radix(&bits[i..i + 8], 2).unwrap())
        .collect()
}
/// The map NPCs the title world stands up never reach the world: the login's rebuild clears the
/// NPC list, and the server's NPC update then adds its NPCs as new arrivals, even at an index a
/// map NPC held.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn the_servers_npc_update_replaces_the_title_worlds_map_npcs() {
    use rs910_scene::title_world::{read_squares, NpcInputs, TitleWorld};
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let types = protocol910::pack_types::load(&pack, true)
        .unwrap()
        .npc_types();
    // The title world around the squares (39..=41, 51), whose lists hold 250 entries.
    let mut title = TitleWorld::default();
    let rebuild = title
        .rebuild([324 << 12, 412 << 12], 0, 4, &mut |_| true)
        .unwrap()
        .unwrap();
    let files = read_squares(&pack, &rebuild.squares);
    let mut feed = Feed::default();
    let mut random = crate::animation_playback::AnimationRandom::new(5);
    let placed = title
        .update_npcs(
            &rebuild,
            &files,
            &mut feed.state.npcs,
            NpcInputs {
                types: &types,
                cycle: 1,
                textures: true,
                random: &mut || random.next(),
            },
        )
        .unwrap()
        .placed;
    assert!(placed > 100);
    let held = *feed.state.npcs.entities.keys().next().unwrap();
    let held_type = feed.state.npcs.entities[&held].type_id;
    // The login's rebuild (the recorded one).
    let w = world();
    let map = protocol910::Context {
        local: 1,
        base_x: 3200,
        base_z: 3200,
        width: 104,
        height: 104,
        bridges: vec![],
    };
    let groups = (0..20000).filter(|n| n % 3 != 0).collect();
    let sizes = Default::default();
    let c = Config {
        prior: &w,
        old_map: &map,
        appearance: None,
        login: true,
        land_groups: &groups,
        loc_sizes: &sizes,
        scene: None,
    };
    feed.enqueue(88, &packet());
    feed.apply_next(&Contexts {
        rebuild: Some(&c),
        player: None,
        npc: None,
        zone: None,
    })
    .unwrap();
    assert!(feed.state.npcs.entities.is_empty());
    assert!(feed.state.npcs.slots.is_empty() && feed.state.npcs.snapshot.is_empty());
    // The server's first NPC update names the same index with another type.
    let other = (0..)
        .find(|id| *id != held_type && types.contains_key(id))
        .unwrap();
    let map = protocol910::Context {
        local: 1,
        base_x: 3208,
        base_z: 3208,
        width: 104,
        height: 104,
        bridges: vec![],
    };
    let cx = protocol910::npc::NpcContext {
        map: &map,
        local_x: 50,
        local_z: 50,
        view_bits: 5,
        loop_cycle: 9,
        textures: true,
        combat: None,
        animation: None,
        variables: None,
        customisation: None,
        wear_slots: None,
        chat_timeout: None,
        random: protocol910::npc::Random::Samples(&[[0.5; 4]]),
        types: &types,
    };
    protocol910::npc::apply(
        &npc_info_adding(held, other as usize),
        &mut feed.state.npcs,
        &cx,
    )
    .unwrap();
    let npcs = &feed.state.npcs;
    assert_eq!(npcs.entities.keys().copied().collect::<Vec<_>>(), [held]);
    assert_eq!(npcs.slots, [held]);
    let npc = &npcs.entities[&held];
    assert_eq!(npc.type_id, other);
    assert_eq!(npc.fade_alpha, 255, "a new arrival, not the map NPC");
    assert_eq!((npc.path.x[0], npc.path.z[0]), (50, 50));
}
