//! Agility and thieving, client side: a recorded dev-server session in which the
//! real client crosses the last four obstacles of the Gnome Stronghold course
//! (the balancing rope, the branch down, the net and the pipe: each a forced
//! movement the client slides the player along), then picks a Menaphite
//! marketeer's pocket, is caught and stunned at the second try, and steals from
//! a Baker's stall, replayed through the production owners (`session_replay`).
//!
//! Fixture: `fixtures/session-replay/support/` (`record.sh`; the release client
//! with `CLIENT910_RECORD` against the dev lobby/world, the TEST-ONLY
//! `ALTO_THIEVING_ROLLS` scripting the thefts, one developer command per 100
//! cycles: the Thieving level, `warp`s to the start of each obstacle after the
//! first, the marketeer and the stall; left clicks at the cycles below).
use super::scenario_woodcutting::{loc_request, message_game, update_inv_partial, update_stat};
use super::session_replay::{arrivals, client_frames, mask_wall_clock, Replay, Trace};
use super::*;
use rs910_symbols::{inv, loc, npc, obj, seq, spot};

const FIXTURE: &str = "fixtures/session-replay/support";
/// `record.sh`: the presses; each click's packet is flushed the cycle after.
const ROPE_CYCLE: i32 = 500;
const BRANCH_CYCLE: i32 = 900;
const NET_CYCLE: i32 = 1200;
const PIPE_CYCLE: i32 = 1500;
const MARKETEER_CYCLE: i32 = 2100;
const CAUGHT_CYCLE: i32 = 2300;
const STALL_CYCLE: i32 = 2650;
const PRESSES: [i32; 7] = [
    ROPE_CYCLE,
    BRANCH_CYCLE,
    NET_CYCLE,
    PIPE_CYCLE,
    MARKETEER_CYCLE,
    CAUGHT_CYCLE,
    STALL_CYCLE,
];
const AGILITY: i32 = 16;
const THIEVING: i32 = 17;
/// `record.sh`: `setxp 17 67983`, Thieving 46.
const THIEVING_SET: i32 = 67983;
const BACKPACK_INV: i32 = inv::BACKPACK.id();
/// Where the stall was put (`locadd 34384 10 0 -3 0` with the player at (2487, 3430)).
const STALL_TILE: [i32; 3] = [2484, 3430, 0];

/// `OPLOC1`/`OPLOC2`: `p1_alt2(ctrl) p2(z) p4(loc) p2_alt3(x)` -> `(loc, x, z)`.
fn oploc(payload: &[u8]) -> (i32, i32, i32) {
    let z = i32::from(payload[1]) << 8 | i32::from(payload[2]);
    let id = i32::from(payload[3]) << 24
        | i32::from(payload[4]) << 16
        | i32::from(payload[5]) << 8
        | i32::from(payload[6]);
    let x = i32::from(payload[7].wrapping_sub(128)) | i32::from(payload[8]) << 8;
    (id, x, z)
}

/// `OPNPC3`: `p1_alt3(ctrl)`, `p2_alt2(index)` -> the NPC slot.
fn opnpc_index(payload: &[u8]) -> usize {
    usize::from(payload[1]) << 8 | usize::from(payload[2].wrapping_sub(128))
}

/// One forced movement as the client holds it: absolute start and end tiles
/// (`[x, z, level]`) and the cycles it starts and ends at.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Slide {
    start: [i32; 3],
    end: [i32; 3],
    cycles: i32,
}

/// The recorded agility and thieving session on the real client.
///
/// Expected values, none from the client under test:
/// - the presses: OPLOC1 on the rope (a piece of it), the branch, the net and
///   the pipe of the course (cache locs, the server's crossing table), OPNPC3 on
///   the marketeer's slot twice, OPLOC2 ("Steal-from") on the stall;
/// - the forced movements the client installs (PLAYER_INFO mask 0x8): the rope
///   from (2477, 3420, 2) to (2483, 3420, 2) over 6 ticks, the branch down from
///   (2486, 3420, 2) to level 0 over 2, the net from (2486, 3425) to (2486,
///   3427) over 2 and the pipe from (2487, 3430) to (2487, 3437) over 7 (30
///   cycles a tick, the server's crossing table); while each runs the player is
///   drawn between its ends, with the obstacle's sequence;
/// - Agility XP (wiki, Gnome Stronghold Agility Course: 7.5, 5, 7.5, 7.5),
///   carried in tenths: 7, 12, 20, 27;
/// - thieving (wiki, Thieving: the marketeer is 29.5 XP and 30 coins, the
///   Baker's stall a cake and 16 XP): the coins and the cake in the backpack's
///   first two slots, 29 then 45 XP; the caught try: the marketeer's shout overhead, a hitmark on
///   the player and the stun's spot animation; the stall's tile holds the empty
///   stall (cache loc 34381) after the theft.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_support_session_crosses_obstacles_and_steals() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut presses = Vec::new();
    let mut slides: Vec<Slide> = Vec::new();
    let mut drawn_between = 0;
    let mut sequences = std::collections::BTreeSet::new();
    let (mut hit, mut stunned, mut shout, mut emptied) = (false, false, false, false);
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        let recorded = trace.bytes(b"OUT ", cycle);
        if PRESSES.contains(&(cycle - 1)) {
            // The click picks the loc or NPC from the renderer's drawn models,
            // which the headless replay does not draw: the recorded frame is the
            // op the real client sent.
            // (A developer command may share the cycle: only the ops are kept.)
            use crate::proto::client::{OPLOC1, OPLOC2, OPNPC3};
            presses.extend(
                client_frames(&recorded)?
                    .into_iter()
                    .filter(|(op, _)| [OPLOC1, OPLOC2, OPNPC3].contains(op)),
            );
        } else {
            assert_eq!(
                mask_wall_clock(&out.written)?,
                mask_wall_clock(&recorded)?,
                "cycle {cycle}: client packets differ from the recording"
            );
        }
        let game = replay.game();
        let base = [game.runtime.map.base_x, game.runtime.map.base_z];
        let state = &game.runtime.feed.state;
        let local = state.players.players[game.runtime.map.local]
            .as_ref()
            .context("local player")?;
        let f = local.forced;
        let slide = Slide {
            start: [base[0] + f[0], base[1] + f[1], f[4]],
            end: [base[0] + f[2], base[1] + f[3], f[5]],
            cycles: f[7] - f[6],
        };
        if f[7] > 0 && slides.last() != Some(&slide) {
            slides.push(slide.clone());
        }
        // While a slide runs the player is drawn between its ends, playing its sequence.
        if f[6] < cycle_of(game) && f[7] > cycle_of(game) && slide.start != slide.end {
            let x = local.fine_x as i32 / 512 + base[0];
            let z = local.fine_z as i32 / 512 + base[1];
            let (lo_x, hi_x) = (
                slide.start[0].min(slide.end[0]),
                slide.start[0].max(slide.end[0]),
            );
            let (lo_z, hi_z) = (
                slide.start[1].min(slide.end[1]),
                slide.start[1].max(slide.end[1]),
            );
            if (lo_x..=hi_x).contains(&x) && (lo_z..=hi_z).contains(&z) {
                drawn_between += 1;
            }
            sequences.insert(local.animation.main.id());
        }
        hit |= local
            .combat
            .as_ref()
            .is_some_and(|c| c.hits.iter().any(|h| h[4] != 0));
        stunned |= local
            .animation
            .spots
            .iter()
            .any(|s| s.id == spot::THIEVING_STUN.id());
        shout |= state.npcs.entities.values().any(|n| {
            n.type_id == npc::MENAPHITE_MARKETEER.id()
                && n.path
                    .chat
                    .as_ref()
                    .and_then(|c| c.text.as_deref())
                    .is_some_and(|t| t == "What do you think you're doing?")
        });
        emptied |= loc_request(game, STALL_TILE)
            .is_some_and(|(id, ..)| id == loc::ARDOUGNE_EMPTY_STALL.id());
    }

    // The presses: the four obstacles, the marketeer twice, the stall.
    let marketeer = replay
        .game()
        .runtime
        .feed
        .state
        .npcs
        .entities
        .iter()
        .find(|(_, n)| n.type_id == npc::MENAPHITE_MARKETEER.id())
        .map(|(&slot, _)| slot)
        .context("the marketeer")?;
    let ops: Vec<(u8, i32, i32, i32)> = presses
        .iter()
        .map(|(op, payload)| match *op {
            crate::proto::client::OPNPC3 => (*op, opnpc_index(payload) as i32, 0, 0),
            _ => {
                let (id, x, z) = oploc(payload);
                (*op, id, x, z)
            }
        })
        .collect();
    let rope = [loc::GNOME_ROPE_START.id(), loc::GNOME_ROPE.id()];
    assert_eq!(ops.len(), 7, "{ops:?}");
    assert!(
        ops[0].0 == crate::proto::client::OPLOC1 && rope.contains(&ops[0].1),
        "{ops:?}"
    );
    assert_eq!(
        ops[1..4].iter().map(|o| (o.0, o.1)).collect::<Vec<_>>(),
        [
            (crate::proto::client::OPLOC1, loc::GNOME_BRANCH_DOWN.id()),
            (crate::proto::client::OPLOC1, loc::GNOME_NET_OVER.id()),
            (crate::proto::client::OPLOC1, loc::GNOME_PIPE_EAST.id())
        ]
    );
    assert_eq!(
        ops[4..6].to_vec(),
        [(crate::proto::client::OPNPC3, marketeer as i32, 0, 0); 2]
    );
    assert_eq!(
        ops[6],
        (
            crate::proto::client::OPLOC2,
            loc::ARDOUGNE_BAKERS_STALL.id(),
            STALL_TILE[0],
            STALL_TILE[1]
        )
    );

    // The four forced movements, as the client installed them.
    assert_eq!(
        slides,
        [
            Slide {
                start: [2477, 3420, 2],
                end: [2483, 3420, 2],
                cycles: 6 * 30
            },
            Slide {
                start: [2486, 3420, 2],
                end: [2486, 3420, 0],
                cycles: 2 * 30
            },
            Slide {
                start: [2486, 3425, 0],
                end: [2486, 3427, 0],
                cycles: 2 * 30
            },
            Slide {
                start: [2487, 3430, 0],
                end: [2487, 3437, 0],
                cycles: 7 * 30
            },
        ]
    );
    assert!(
        drawn_between > 100,
        "drawn between the ends for {drawn_between} cycles"
    );
    for sequence in [seq::LOG_BALANCE_WALK, seq::CLIMB_LADDER, seq::CRAWL] {
        assert!(sequences.contains(&sequence.id()), "{sequences:?}");
    }

    // The XP and the backpack, from the server's frames.
    let all = arrivals(&trace)?;
    let stats: Vec<[i32; 3]> = all
        .iter()
        .filter(|a| a.opcode == sp::UPDATE_STAT)
        .map(|a| update_stat(&a.payload))
        .collect();
    let xp = |skill: i32| -> Vec<i32> {
        stats
            .iter()
            .filter(|s| s[0] == skill && s[1] > 0)
            .map(|s| s[1])
            .collect()
    };
    assert_eq!(xp(AGILITY), [7, 12, 20, 27]);
    // 29.5 for the pocket (29, 5 tenths carried), 16 for the stall.
    assert_eq!(
        xp(THIEVING),
        [THIEVING_SET, THIEVING_SET + 29, THIEVING_SET + 45]
    );
    let backpack: Vec<[i32; 3]> = all
        .iter()
        .filter(|a| a.opcode == sp::UPDATE_INV_PARTIAL)
        .map(|a| update_inv_partial(&a.payload))
        .filter(|(inv, _)| *inv == BACKPACK_INV)
        .flat_map(|(_, slots)| slots)
        .collect();
    assert_eq!(backpack, [[0, obj::COINS.id(), 30], [1, obj::CAKE.id(), 1]]);
    let messages: Vec<String> = all
        .iter()
        .filter(|a| a.opcode == sp::MESSAGE_GAME)
        .map(|a| message_game(&a.payload).1)
        .filter(|m| m.contains("pocket") || m.contains("stunned") || m.contains("steal"))
        .collect();
    assert_eq!(
        messages,
        [
            "You attempt to pick the menaphite marketeer's pocket.",
            "You pick the menaphite marketeer's pocket.",
            "You attempt to pick the menaphite marketeer's pocket.",
            "You've been stunned!",
            "You steal cake from the stall."
        ]
    );
    assert!(hit, "no hitmark on the stunned player");
    assert!(stunned, "no stun spot animation on the player");
    assert!(shout, "the marketeer did not shout");
    assert!(emptied, "the stall did not empty");
    Ok(())
}

/// The client's logic cycle of the replayed game (what the forced movement's cycles count in).
fn cycle_of(game: &crate::client_game::ClientGame) -> i32 {
    game.cycle
}
