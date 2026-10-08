use super::*;
use crate::entities910::{animation_state::Sequence, Npc, Player};

#[test]
fn reset_anims_clears_player_and_npc_movement_sequences() {
    let mut feed = Feed::default();
    let mut player = Player::default();
    player.animation.modes = Some(vec![42]);
    player.animation.main.sequence = Some(Sequence {
        id: 42,
        frames: Some(1),
        skeletal: false,
        restart: 0,
        priority: 0,
        stationary: 0,
        moving: 0,
    });
    feed.state.players.players[0] = Some(player);

    let mut npc = Npc::new([0; 4]);
    npc.path.animation.modes = Some(vec![42]);
    npc.path.animation.main.sequence = feed.state.players.players[0]
        .as_ref()
        .and_then(|p| p.animation.main.sequence.clone());
    feed.state.npcs.entities.insert(7, npc);

    assert!(feed.enqueue(crate::proto::server::RESET_ANIMS, &[]));
    feed.apply_next(&Contexts {
        rebuild: None,
        player: None,
        npc: None,
        zone: None,
    })
    .expect("RESET_ANIMS applies without scene context")
    .expect("RESET_ANIMS receipt");

    let player = feed.state.players.players[0].as_ref().unwrap();
    assert!(player.animation.modes.is_none());
    assert!(player.animation.main.sequence.is_none());
    let npc = feed.state.npcs.entities.get(&7).unwrap();
    assert!(npc.path.animation.modes.is_none());
    assert!(npc.path.animation.main.sequence.is_none());
}

#[test]
fn reset_anims_rejects_payload_bytes() {
    let mut feed = Feed::default();
    assert!(feed.enqueue(crate::proto::server::RESET_ANIMS, &[1]));
    let error = feed
        .apply_next(&Contexts {
            rebuild: None,
            player: None,
            npc: None,
            zone: None,
        })
        .expect_err("RESET_ANIMS payload must be empty");
    assert_eq!(error, Error::Invalid("RESET_ANIMS length"));
}

#[test]
fn region_rebuild_decodes_templates_into_a_map_transaction() {
    use std::collections::{BTreeMap, BTreeSet};
    let prior = rebuild_state::World {
        base_x: 0,
        base_z: 0,
        region_x: 0,
        region_z: 0,
        width: 104,
        height: 104,
        area: Some(0),
        last_kind: rebuild_state::Kind::Normal,
        npc_bits: 8,
        map_squares: vec![],
        groups: vec![],
        group_count: 0,
    };
    let old_map = crate::protocol910::Context {
        local: 0,
        base_x: 0,
        base_z: 0,
        width: 104,
        height: 104,
        bridges: vec![],
    };
    let source_x = 8 * 50;
    let source_z = 8 * 60;
    let source_group = 50 | (60 << 7);
    let template = (source_x << 14) | (source_z << 3) | (2 << 1);
    let target = (2 * 13 + 3) * 13 + 4;
    let mut payload = vec![5, 0, 1, 0, 64, 65, 0, 128];
    let mut bits = Vec::new();
    for index in 0..(4 * 13 * 13) {
        if index == target {
            bits.push(1);
            for shift in (0..26).rev() {
                bits.push(((template >> shift) & 1) as u8);
            }
        } else {
            bits.push(0);
        }
    }
    while bits.len() % 8 != 0 {
        bits.push(0);
    }
    payload.extend(
        bits.chunks(8)
            .map(|chunk| {
                chunk
                    .iter()
                    .enumerate()
                    .fold(0u8, |v, (i, b)| v | (*b << (7 - i)))
            })
            .collect::<Vec<_>>(),
    );

    let mut feed = Feed::default();
    feed.state.world = Some(prior.clone());
    assert!(feed.enqueue(crate::proto::server::REBUILD_REGION, &payload));
    let appearance = None;
    let land_groups = BTreeSet::from([source_group]);
    let config = rebuild_state::Config {
        prior: &prior,
        old_map: &old_map,
        appearance,
        login: false,
        land_groups: &land_groups,
        loc_sizes: &BTreeMap::new(),
        scene: None,
    };
    let applied = feed
        .apply_next(&Contexts {
            rebuild: Some(&config),
            player: None,
            npc: None,
            zone: None,
        })
        .unwrap()
        .unwrap();
    let (world, effects) = applied.rebuild.unwrap();
    assert_eq!(world.last_kind, rebuild_state::Kind::Region);
    assert_eq!(world.group_count, 1);
    assert_eq!(world.groups[0], source_group);
    let layout = effects.region.unwrap();
    assert_eq!(layout.templates[target], template);
    assert!(effects.rebased);
}

#[test]
fn npc_anim_specific_reaches_retained_npc_animation_state() {
    use crate::entities910::animation_state::{Config as AnimationConfig, Sequence};
    use std::collections::BTreeMap;

    let mut feed = Feed::default();
    let mut npc = Npc::new([0; 4]);
    npc.path.route_length = 2;
    feed.state.npcs.entities.insert(7, npc);
    let map = crate::protocol910::Context {
        local: 0,
        base_x: 0,
        base_z: 0,
        width: 104,
        height: 104,
        bridges: vec![],
    };
    let animation = AnimationConfig {
        sequences: BTreeMap::from([(
            42,
            Sequence {
                id: 42,
                frames: Some(1),
                skeletal: false,
                restart: 0,
                priority: 0,
                stationary: 0,
                moving: 0,
            },
        )]),
        effects: BTreeMap::new(),
        slots: 5,
    };
    let types = BTreeMap::new();
    let npc_context = npc::NpcContext {
        map: &map,
        local_x: 0,
        local_z: 0,
        view_bits: 8,
        loop_cycle: 0,
        textures: false,
        combat: None,
        animation: Some(&animation),
        variables: None,
        customisation: None,
        wear_slots: Some(0),
        chat_timeout: None,
        random: npc::Random::Samples(&[]),
        types: &types,
    };
    let mut payload = Vec::new();
    for mode in [42i32, -1, -1, -1] {
        payload.extend_from_slice(&mode.to_le_bytes());
    }
    payload.extend_from_slice(&7u16.to_be_bytes());
    payload.push(253); // g1_alt2 -> delay 3
    assert_eq!(payload.len(), 19);
    assert!(feed.enqueue(crate::proto::server::NPC_ANIM_SPECIFIC, &payload));
    feed.apply_next(&Contexts {
        rebuild: None,
        player: None,
        npc: Some(&npc_context),
        zone: None,
    })
    .expect("NPC_ANIM_SPECIFIC applies with NPC animation context")
    .expect("NPC_ANIM_SPECIFIC receipt");

    let npc = feed.state.npcs.entities.get(&7).unwrap();
    assert_eq!(
        npc.path.animation.modes.as_deref(),
        Some(&[42, -1, -1, -1][..])
    );
    assert_eq!(npc.path.animation.main.delay, 3);
    assert_eq!(npc.path.steps_remaining, 2);
}

#[test]
fn loc_anim_specific_reaches_retained_scene_animation_request() {
    use std::collections::BTreeMap;

    let mut feed = Feed::default();
    feed.state.initialized = true;
    let map = crate::protocol910::Context {
        local: 0,
        base_x: 3200,
        base_z: 3200,
        width: 104,
        height: 104,
        bridges: vec![],
    };
    let zone = zone_state::Config {
        map: &map,
        cycle: 0,
        cutscene: false,
        allow_outside: false,
        objects: &BTreeMap::new(),
        scene: None,
        transients: None,
    };
    let packed = (1_u32 << 28) | (3204_u32 << 14) | 3205_u32;
    let mut payload = packed.to_le_bytes().to_vec();
    payload.push(7);
    payload.extend_from_slice(&42_i32.to_be_bytes());
    payload.push((10 << 2) | 1);
    assert_eq!(payload.len(), 10);
    assert!(feed.enqueue(crate::proto::server::LOC_ANIM_SPECIFIC, &payload));
    feed.apply_next(&Contexts {
        rebuild: None,
        player: None,
        npc: None,
        zone: Some(&zone),
    })
    .expect("LOC_ANIM_SPECIFIC applies with zone context")
    .expect("LOC_ANIM_SPECIFIC receipt");

    assert_eq!(feed.state.zones.loc_animations.len(), 1);
    let request = &feed.state.zones.loc_animations[0];
    assert_eq!((request.level, request.x, request.z), (1, 4, 5));
    assert_eq!((request.layer, request.shape, request.angle), (2, 10, 1));
    assert_eq!((request.sequence, request.delay), (42, 7));
}

#[test]
fn projanim_specific_reaches_retained_projectile_state() {
    use crate::entities910::animation_state::{Config as AnimationConfig, Effect, Sequence};
    use std::collections::BTreeMap;

    let mut feed = Feed::default();
    feed.state.initialized = true;
    let map = crate::protocol910::Context {
        local: 0,
        base_x: 3200,
        base_z: 3200,
        width: 104,
        height: 104,
        bridges: vec![],
    };
    let animation = AnimationConfig {
        sequences: BTreeMap::from([(
            42,
            Sequence {
                id: 42,
                frames: Some(1),
                skeletal: false,
                restart: 0,
                priority: 0,
                stationary: 0,
                moving: 0,
            },
        )]),
        effects: BTreeMap::from([(
            7,
            Effect {
                sequence: 42,
                looping: false,
            },
        )]),
        slots: 5,
    };
    let attachment_y = BTreeMap::new();
    let height = |_x: i32, _z: i32, _level: i32| 100;
    let transient = crate::protocol910::transient::Config {
        cycle: 50,
        cutscene: false,
        animation: &animation,
        height: &height,
        attachment_y: Some(&attachment_y),
    };
    let zone = zone_state::Config {
        map: &map,
        cycle: 50,
        cutscene: false,
        allow_outside: false,
        objects: &BTreeMap::new(),
        scene: None,
        transients: Some(&transient),
    };
    let mut payload = Vec::new();
    payload.extend_from_slice(&(3200_u16 * 2 + 8).to_le_bytes());
    payload.push(254); // g1b_alt2 -> delta x 2
    payload.extend_from_slice(&[131, 0]); // g2_alt3 -> start delay 3
    payload.extend_from_slice(&8_u16.to_be_bytes());
    payload.extend_from_slice(&[0, 133]); // g2_alt2 -> arc 5
    payload.push(236); // g1_alt2 -> pitch 20
    payload.extend_from_slice(&(-1_i16).to_le_bytes());
    payload.push(127); // g1_alt3 -> follow flag 1
    payload.push(9); // targeted
    payload.extend_from_slice(&(-2_i16).to_le_bytes());
    payload.extend_from_slice(&(3200_u16 * 2 + 10).to_be_bytes());
    payload.push(129); // g1b_alt3 -> delta z -1
    payload.push(132); // g1_alt1 -> end offset 4
    payload.push(6); // start height
    payload.extend_from_slice(&7_u16.to_le_bytes());
    assert_eq!(payload.len(), 23);
    assert!(feed.enqueue(crate::proto::server::PROJANIM_SPECIFIC, &payload));
    feed.apply_next(&Contexts {
        rebuild: None,
        player: None,
        npc: None,
        zone: Some(&zone),
    })
    .expect("PROJANIM_SPECIFIC applies with transient context")
    .expect("PROJANIM_SPECIFIC receipt");

    assert_eq!(feed.state.zones.transients.projectiles.len(), 1);
    let projectile = &feed.state.zones.transients.projectiles[0];
    assert_eq!(
        (projectile.effect, projectile.start, projectile.end),
        (7, 53, 58)
    );
    assert_eq!(
        (projectile.source, projectile.target, projectile.targeted),
        (-1, -2, 9)
    );
    assert!(projectile.follow_ground);
    assert_eq!(projectile.position[1], 76.);
    assert!((projectile.position[0] - 2065.8884).abs() < 0.001);
    assert!((projectile.position[2] - 2551.0557).abs() < 0.001);
}
