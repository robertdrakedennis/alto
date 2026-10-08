#[allow(unused_imports)]
use crate::{entities910, protocol910};
use protocol910::*;
#[test]
fn live_feed_retains_failed_front_and_successors() {
    let c = ctx();
    let packets = corpus();
    let pc = PlayerContext {
        world: &c,
        appearance: None,
        combat: None,
        animation: None,
        variables: None,
        chat_timeout: None,
        cycle: 0,
    };
    let mut feed = live::Feed::default();
    // The login PLAYER_INFO now arrives through REBUILD_NORMAL; install its
    // decoded state directly.
    feed.state.players = initialize(&packets[0].1, &c).unwrap().state;
    feed.state.initialized = true;
    let before = feed.state.clone();
    feed.enqueue(122, &packets[1].1);
    feed.enqueue(129, &[]);
    let original = feed.front().unwrap().clone();
    assert!(feed
        .apply_next(&live::Contexts {
            rebuild: None,
            player: None,
            npc: None,
            zone: None
        })
        .is_err());
    assert_eq!(feed.state, before);
    assert_eq!(feed.front(), Some(&original));
    assert_eq!(feed.pending_len(), 2);
    feed.apply_next(&live::Contexts {
        rebuild: None,
        player: Some(&pc),
        npc: None,
        zone: None,
    })
    .unwrap();
    assert_eq!(feed.pending_len(), 1);
    let after = feed.state.clone();
    let tick = feed
        .apply_next(&live::Contexts {
            rebuild: None,
            player: None,
            npc: None,
            zone: None,
        })
        .unwrap()
        .unwrap();
    assert!(tick.read_batch_end);
    assert_eq!(feed.state, after); // packet boundary must not advance movement/chat.
    assert!(feed.blocked.is_none());
}
#[test]
fn live_rebuild_and_unsupported_entity_packets_never_fall_through() {
    for op in [88, 4, 84, 124, 147, 113, 70] {
        let mut feed = live::Feed::default();
        assert!(feed.enqueue(op, &[1, 2, 3]));
        let before = feed.state.clone();
        assert!(feed
            .apply_next(&live::Contexts {
                rebuild: None,
                player: None,
                npc: None,
                zone: None
            })
            .is_err());
        assert_eq!(feed.state, before);
        assert_eq!(feed.front().unwrap().payload, [1, 2, 3]);
    }
    assert!(!live::Feed::default().enqueue(83, &[])); // keepalive belongs to transport
}
/// The initial player-positions packet (production `initialize_cached`).
fn initialize(bytes: &[u8], c: &Context) -> std::result::Result<Decoded, Error> {
    initialize_cached(bytes, &Players::default(), c, None)
}
struct Out {
    state: Players,
}
/// One PLAYER_INFO through the production in-place `apply_full`; on error
/// the undo log must have restored the prior state exactly.
fn decode(bytes: &[u8], prior: &Players, c: &Context) -> std::result::Result<Out, Error> {
    let mut state = prior.clone();
    let context = PlayerContext {
        world: c,
        appearance: None,
        combat: None,
        animation: None,
        variables: None,
        chat_timeout: None,
        cycle: 0,
    };
    match apply_full(bytes, &mut state, &context) {
        Ok(_) => Ok(Out { state }),
        Err(error) => {
            assert_eq!(&state, prior, "apply_full did not roll back");
            Err(error)
        }
    }
}
fn npc_decode(bytes: &[u8], cx: &npc::NpcContext) -> std::result::Result<(), Error> {
    let mut state = npc::Npcs::default();
    match npc::apply(bytes, &mut state, cx) {
        Ok(_) => Ok(()),
        Err(error) => {
            assert_eq!(state, npc::Npcs::default(), "npc::apply did not roll back");
            Err(error)
        }
    }
}
fn ctx() -> Context {
    Context {
        local: 1,
        base_x: 3200,
        base_z: 3200,
        width: 104,
        height: 104,
        bridges: vec![(51, 51)],
    }
}
/// Deterministic packet corpora
/// (packet construction only, no expected state), checked in under
/// fixtures/phase-g/.
fn fixture(name: &str) -> std::path::PathBuf {
    rs910_core::test_support::client_dir()
        .join("fixtures/phase-g")
        .join(name)
}
fn corpus() -> Vec<(String, Vec<u8>)> {
    std::fs::read_to_string(fixture("players.txt"))
        .unwrap()
        .lines()
        .map(|line| {
            let (k, h) = line.split_once(' ').unwrap();
            (
                k.into(),
                (0..h.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
                    .collect(),
            )
        })
        .collect()
}
#[test]
fn errors_leave_prior_state_unchanged() {
    let c = ctx();
    let packets = corpus();
    let mut state = initialize(&packets[0].1, &c).unwrap().state;
    for (_, b) in &packets[1..] {
        let before = state.clone();
        for end in [0, b.len() / 2, b.len() - 1] {
            assert!(decode(&b[..end], &state, &c).is_err());
            assert_eq!(state, before);
        }
        let mut extra = b.clone();
        extra.push(0);
        assert!(decode(&extra, &state, &c).is_err());
        state = decode(b, &state, &c).unwrap().state;
    }
}
#[test]
fn unsupported_masks_reject_atomically() {
    let c = ctx();
    let packets = corpus();
    let mut state = initialize(&packets[0].1, &c).unwrap().state;
    for (index, (_, b)) in packets.iter().enumerate().skip(1) {
        if index == 31 {
            // target/suppression fixture: add var update mask without supplying var payload
            let mut b = b.clone();
            let mask = b.len() - 6;
            b[mask + 2] |= 0x04;
            let before = state.clone();
            assert!(matches!(
                decode(&b, &state, &c),
                Err(Error::UnsupportedMask { .. } | Error::UnsupportedContext(_))
            ));
            assert_eq!(state, before);
            return;
        }
        state = decode(b, &state, &c).unwrap().state;
    }
    panic!("mask fixture missing")
}
#[test]
fn npc_missing_config_and_random_are_explicit() {
    use protocol910::npc::*;
    let c = ctx();
    let data = std::fs::read_to_string(fixture("npcs.txt")).unwrap();
    let hex = data.lines().nth(1).unwrap().split_once(' ').unwrap().1;
    let b: Vec<_> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let types = std::collections::BTreeMap::new();
    let mut cx = NpcContext {
        map: &c,
        local_x: 50,
        local_z: 50,
        view_bits: 5,
        loop_cycle: 0,
        textures: true,
        combat: None,
        animation: None,
        variables: None,
        customisation: None,
        wear_slots: None,
        chat_timeout: None,
        random: protocol910::npc::Random::Samples(&[]),
        types: &types,
    };
    assert!(matches!(
        npc_decode(&b, &cx),
        Err(Error::UnsupportedContext(_))
    ));
    cx.random = protocol910::npc::Random::Samples(&[[0.5; 4]; 4]);
    assert!(matches!(
        npc_decode(&b, &cx),
        Err(Error::UnsupportedContext(_))
    ));
}
#[test]
fn zone_rejects_missing_valuation_and_preserves_state() {
    use protocol910::zone::*;
    let c = ctx();
    let types = std::collections::BTreeMap::new();
    let cx = ZoneContext {
        map: &c,
        x: 48,
        z: 48,
        level: 0,
        allow_outside: false,
        types: &types,
    };
    let b = [128, 129, 0, 0, 0];
    let first = zone::decode(Op::Add, &b, &Objects::default(), &cx)
        .unwrap()
        .state;
    let prior = first.clone();
    assert!(matches!(
        zone::decode(Op::Add, &b, &first, &cx),
        Err(Error::UnsupportedContext(_))
    ));
    assert_eq!(first, prior);
}

#[test]
fn unsupported_enclosed_zone_rolls_back_preceding_updates() {
    let map = ctx();
    let types = [(
        1,
        zone::ObjectType {
            cost: 1,
            stackable: 1,
        },
    )]
    .into_iter()
    .collect();
    let config = zone_state::Config {
        map: &map,
        allow_outside: false,
        cycle: 0,
        cutscene: false,
        objects: &types,
        scene: None,
        transients: None,
    };
    let state = zone_state::State::default();
    let before = state.clone();
    // Object add followed by an opcode no zone prot owns. (Op 14, SOUND_AREA,
    // was the unsupported example before it was ported: zone_state.rs
    // `op == 14 || op == 6`.)
    let bytes = [131, 250, 250, 13, 128, 129, 0, 1, 0, 15];
    assert!(matches!(
        zone_state::decode(&bytes, &state, &config, zone_state::Input::Enclosed),
        Err(Error::UnsupportedZone(15))
    ));
    assert_eq!(state, before);
}
#[test]
fn appearance_cache_requires_explicit_config() {
    let c = ctx();
    let mut state = Players::default();
    state.appearances[1] = Some(entities910::appearance::CachedPacket {
        data: vec![0],
        consumed: 0,
    });
    let init = &corpus()[0].1;
    assert!(matches!(
        initialize_cached(init, &state, &c, None),
        Err(Error::UnsupportedContext(_))
    ));
    assert_eq!(state.appearances[1].as_ref().unwrap().consumed, 0);
}
#[test]
fn reserved_player_mask_is_explicitly_unsupported() {
    let c = ctx();
    let packets = corpus();
    let mut state = initialize(&packets[0].1, &c).unwrap().state;
    for (index, (_, b)) in packets.iter().enumerate().skip(1) {
        if index == 31 {
            let mut b = b.clone();
            let mask = b.len() - 6;
            b[mask + 1] |= 0x40;
            assert!(matches!(
                decode(&b, &state, &c),
                Err(Error::UnsupportedMask { .. })
            ));
            return;
        }
        state = decode(b, &state, &c).unwrap().state;
    }
    panic!("missing fixture");
}
