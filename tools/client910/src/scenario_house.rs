//! The player-owned house, client side: one recorded dev-server session at the
//! Taverley house portal in which the real client enters its owner's house in
//! building mode (an instanced region, REBUILD_REGION, installed mid-session),
//! builds a parlour through the garden's south doorway with the room menu (a
//! second region install), and builds a crude wooden chair in it with the
//! furniture menu; replayed through the production owners (`session_replay`).
//!
//! Fixture: `fixtures/session-replay/house/` (`record.sh`; the release client
//! against the dev lobby/world). The character is placed by a written save that
//! already owns a house (a garden with its exit portal and a parlour north of
//! it); the room menu's Parlour Build and the furniture menu's first option are
//! UI operations; loc options are the dev server's `oploc` command (the
//! headless replay picks no scene locs).
use super::scenario_tests::ui;
use super::scenario_woodcutting::{chat, if_opensub, message_game, update_stat};
use super::session_replay::{
    arrivals, mask_wall_clock, observe, server_trace_file, Arrival, Replay, Trace,
};
use super::*;
use rs910_symbols::{interface, inv, loc, obj};

const FIXTURE: &str = "fixtures/session-replay/house";
/// Construction's stat id (the cache's stat names, enum 680).
const CONSTRUCTION: i32 = 22;
const CHUNK: i32 = 8;
/// The house's ground floor is level 1 of its region (Construction's placeholder layout).
const GROUND_FLOOR: i32 = 1;

/// Server content texts (Construction module; unverified against 910).
const TEXTS: [&str; 1] = ["You build the parlour."];

/// The client packets of a cycle without the round-trip reports (the recording
/// machine's ping answered some cycles after the replay's fixed answer).
fn without_reports(bytes: &[u8]) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    Ok(mask_wall_clock(bytes)?
        .into_iter()
        .filter(|(op, _)| *op != crate::proto::client::PING_STATISTICS)
        .collect())
}

/// A region template's source: everything but its quarter turns.
const fn source(template: i32) -> i32 {
    template & !(0x3 << 1)
}

const fn turns(template: i32) -> i32 {
    (template >> 1) & 0x3
}

/// The installed region's template of the chunk `[dx, dz]` chunks from the one
/// holding `tile` (absolute), or None outside a region.
fn template_beside(
    game: &crate::client_game::ClientGame,
    tile: [i32; 3],
    step: [i32; 2],
) -> Option<i32> {
    let layout = game.runtime.installed_region.as_ref()?;
    let [base_x, base_z] = [game.runtime.map.base_x, game.runtime.map.base_z];
    let cx = usize::try_from((tile[0] - base_x) / CHUNK + step[0]).ok()?;
    let cz = usize::try_from((tile[1] - base_z) / CHUNK + step[1]).ok()?;
    let level = usize::try_from(tile[2]).ok()?;
    (cx < layout.chunks_x && cz < layout.chunks_z)
        .then(|| layout.templates[(level * layout.chunks_x + cx) * layout.chunks_z + cz])
}

/// Every loc change request of a loc on layer 2 the client holds, as absolute tiles.
fn loc_requests_of(game: &crate::client_game::ClientGame, id: i32) -> Vec<[i32; 3]> {
    let base = [game.runtime.map.base_x, game.runtime.map.base_z];
    game.runtime
        .feed
        .state
        .zones
        .locations
        .iter()
        .filter(|r| r.layer == 2 && r.id == id)
        .map(|r| [base[0] + r.x, base[1] + r.z, r.level])
        .collect()
}

/// The stat updates of one stat in the recording: `[xp, level]` in order.
fn stat_updates(arrivals: &[Arrival], stat: i32) -> Vec<[i32; 2]> {
    use crate::proto::server as sp;
    arrivals
        .iter()
        .filter(|a| a.opcode == sp::UPDATE_STAT)
        .map(|a| update_stat(&a.payload))
        .filter(|u| u[0] == stat)
        .map(|u| [u[1], u[2]])
        .collect()
}

/// The recorded session on the real client, cycle by cycle, with the exact
/// outgoing bytes of every cycle.
///
/// Expected values, none from the client under test:
/// - positions: the dev server's own trace (`server-trace.jsonl`), one entry per
///   PLAYER_INFO (in the house, the region's coordinates on level 1); chat: the
///   MESSAGE_GAME texts of the server content, in order;
/// - two region installs: the first copies the garden under the player, the
///   parlour north of it (unturned) and an empty plot south of it; the second
///   puts, south of the garden, a copy of the same parlour chunk turned one
///   quarter (its west doorway facing the garden), the server's room rule;
/// - the room menu (interface 402) and the furniture menu (1306) are each opened
///   once; the furniture menu's list (inventory 398) starts with the crude wooden
///   chair (the cache's build-menu obj);
/// - the chair: the client's loc-change snapshot holds the crude wooden chair's
///   loc (the loc drawn with the build-menu obj's model) on a tile of the new
///   parlour; Construction XP 66 (wiki Parlour).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_house_session_enters_the_house_builds_a_parlour_and_a_chair() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let arrivals = arrivals(&trace)?;
    let (server_players, _) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let mut replay = Replay::start(&trace)?;
    let mut done = 0;
    // The installs seen: the template under the player, north of it and south of it.
    let mut regions: Vec<[Option<i32>; 3]> = Vec::new();
    let mut garden_tile = None;
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            without_reports(&out.written)?,
            without_reports(&trace.bytes(b"OUT ", cycle))?,
            "cycle {cycle}: client packets differ from the recording"
        );
        let start = done;
        done = replay.processed(&arrivals);
        let local = observe(replay.game()).local.context("local player")?;
        if arrivals[start..done]
            .iter()
            .any(|a| a.opcode == sp::PLAYER_INFO)
        {
            let k = arrivals[..done]
                .iter()
                .filter(|a| a.opcode == sp::PLAYER_INFO)
                .count();
            let [x, z, level, _] = server_players[k - 1];
            assert_eq!(local.tile, [x, z, level], "cycle {cycle}: tile vs server");
        }
        let expected_chat: Vec<(i32, String)> = arrivals[..done]
            .iter()
            .filter(|a| a.opcode == sp::MESSAGE_GAME)
            .map(|a| message_game(&a.payload))
            .collect();
        assert_eq!(
            chat(ui(&mut replay)),
            expected_chat,
            "cycle {cycle}: chat history"
        );
        // The garden is the chunk the player arrives in.
        if garden_tile.is_none() && replay.game().runtime.installed_region.is_some() {
            garden_tile = Some(local.tile);
        }
        if let Some(garden) = garden_tile {
            let game = replay.game();
            let seen = [
                template_beside(game, garden, [0, 0]),
                template_beside(game, garden, [0, 1]),
                template_beside(game, garden, [0, -1]),
            ];
            if regions.last() != Some(&seen) {
                regions.push(seen);
            }
        }
    }

    let garden = garden_tile.context("the house was never installed")?;
    assert_eq!(garden[2], GROUND_FLOOR, "the house's ground floor");
    assert_eq!(regions.len(), 2, "two region installs: {regions:x?}");
    let [Some(garden_chunk), Some(parlour), Some(plot)] = regions[0] else {
        anyhow::bail!("the first region: {:x?}", regions[0]);
    };
    let [Some(garden_after), Some(parlour_after), Some(built)] = regions[1] else {
        anyhow::bail!("the second region: {:x?}", regions[1]);
    };
    // South of the garden: an empty plot (the template map's grass chunk), then the new parlour.
    assert!(
        source(plot) != source(garden_chunk) && source(plot) != source(parlour),
        "an empty plot south of the garden"
    );
    assert_eq!(
        [turns(garden_chunk), turns(parlour)],
        [0, 0],
        "the starter rooms are unturned"
    );
    assert_ne!(
        source(garden_chunk),
        source(parlour),
        "a garden and a parlour"
    );
    assert_eq!(
        [garden_after, parlour_after],
        [garden_chunk, parlour],
        "the starter rooms stay"
    );
    assert_eq!(
        [source(built), turns(built)],
        [source(parlour), 1],
        "the new room is the parlour turned a quarter"
    );

    let opened: Vec<i32> = arrivals
        .iter()
        .filter(|a| a.opcode == sp::IF_OPENSUB)
        .map(|a| if_opensub(&a.payload)[1])
        .filter(|id| {
            [
                interface::ROOM_CREATION.id(),
                interface::FURNITURE_CREATION.id(),
            ]
            .contains(id)
        })
        .collect();
    assert_eq!(
        opened,
        [
            interface::ROOM_CREATION.id(),
            interface::FURNITURE_CREATION.id()
        ],
        "the room menu, then the furniture menu"
    );
    let listed = arrivals
        .iter()
        .find(|a| {
            a.opcode == sp::UPDATE_INV_FULL
                && i32::from(u16::from_be_bytes([a.payload[0], a.payload[1]]))
                    == inv::FURNITURE_CREATION_OPTIONS.id()
        })
        .context("the furniture menu's list")?;
    let first = i32::from(u16::from_be_bytes([listed.payload[5], listed.payload[6]])) - 1;
    assert_eq!(
        first,
        obj::CRUDE_WOODEN_CHAIR.id(),
        "the menu's first option"
    );

    // The chair stands on a tile of the new parlour (the chunk south of the garden).
    let chairs = loc_requests_of(replay.game(), loc::CRUDE_WOODEN_CHAIR.id());
    assert_eq!(chairs.len(), 1, "one chair: {chairs:?}");
    let chair = chairs[0];
    assert_eq!(
        [chair[0] / CHUNK, chair[1] / CHUNK, chair[2]],
        [garden[0] / CHUNK, garden[1] / CHUNK - 1, GROUND_FLOOR],
        "the chair's chunk"
    );
    let texts: Vec<String> = arrivals
        .iter()
        .filter(|a| a.opcode == sp::MESSAGE_GAME)
        .map(|a| message_game(&a.payload).1)
        .collect();
    assert_eq!(texts, TEXTS);
    assert_eq!(
        stat_updates(&arrivals, CONSTRUCTION).last().map(|u| u[0]),
        Some(66),
        "Construction XP"
    );
    Ok(())
}
