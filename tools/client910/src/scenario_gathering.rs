//! Gathering beyond the first rung, client side: one recorded dev-server
//! session in which the real client chops the Lumbridge willow twice (level 30,
//! the tree falls after a log, a bird's nest drops, the tree grows back), replayed
//! through the production owners (`session_replay`) with the renderer's headless
//! pick frame while the mouse is on the willow.
//!
//! Fixture: `fixtures/session-replay/gathering/` (`record.sh`; the release
//! client with `CLIENT910_RECORD` against the dev lobby/world with the TEST-ONLY
//! `ALTO_SKILLS_ALWAYS=1`, so every roll goes the player's way). The session
//! also carries what `record.sh` set up for an iron rock (levels, tools, the rock
//! added with `locadd`) and a lure spot. Its two clicks on them landed on the
//! ground (the client did not pick the added rock, and the spot had already moved
//! along the bank), so they are walk clicks: mining with the stamina bar and
//! lure fishing are proven by `GatheringE2E.integration.test.ts`, not by a replay.
use super::scenario_tests::{ui, writers};
use super::scenario_woodcutting::{
    chat, cs2_ints, loc_request, menu_on_loc, message_game, update_inv_partial, update_stat,
};
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Arrival, Replay, Trace,
};
use super::*;
use rs910_symbols::{inv, loc, obj, seq};

const FIXTURE: &str = "fixtures/session-replay/gathering";
/// `record.sh`: the presses on the willow, and the two on the rock and the spot
/// (walk clicks); the mouse is on its target from 10 cycles before each press.
const PRESS_CYCLES: [i32; 2] = [900, 2000];
const WALK_PRESS_CYCLES: [i32; 2] = [1200, 2300];

/// Cache and world facts (the server's own loc, obj and seq decoders).
const WILLOW: i32 = loc::WILLOW.id();
const WILLOW_TILE: [i32; 3] = [3235, 3232, 0]; // shape 10, angle 2
const IRON_ROCK: i32 = loc::IRON_ROCK.id();
const ROCK_TILE: [i32; 3] = [3233, 3232, 0]; // `locadd 113158 10 0 -1 0` beside the player
const BRONZE_HATCHET: i32 = obj::BRONZE_HATCHET.id();
const FLY_FISHING_ROD: i32 = obj::FLY_FISHING_ROD.id();
const FEATHER: i32 = obj::FEATHER.id();
const WILLOW_LOGS: i32 = obj::WILLOW_LOGS.id();
const CHOP_SEQ: i32 = seq::CHOP_BRONZE_HATCHET.id();
const WOODCUTTING: i32 = 8;
const MINING: i32 = 14;
const FISHING: i32 = 10;
const BACKPACK_INV: i32 = inv::BACKPACK.id();
/// Server content texts (Woodcutting data.ts MESSAGES; unverified against 910).
const SWING: &str = "You swing your hatchet at the willow.";
const NEST: &str = "A bird's nest falls out of the tree.";
const SUCCESS: &str = "You get some willow logs.";

/// One chop of the recording.
struct Chop {
    swing: usize,
    logs: usize,
    stat: usize,
    success: usize,
    crown_del: usize,
    respawn: usize,
}

struct Events {
    stats: [usize; 3],
    tools: [usize; 3],
    rock: usize,
    chops: [Chop; 2],
}

fn find(
    arrivals: &[Arrival],
    from: usize,
    what: &str,
    pred: &dyn Fn(&Arrival) -> bool,
) -> anyhow::Result<usize> {
    arrivals[from..]
        .iter()
        .position(pred)
        .map(|at| from + at)
        .with_context(|| format!("{what} not in the fixture"))
}

/// Located server frames, each searched after the previous one: the order is
/// part of what the recording shows.
fn events(arrivals: &[Arrival]) -> anyhow::Result<Events> {
    use crate::proto::server as sp;
    let stat = |skill, xp, level| {
        move |a: &Arrival| {
            a.opcode == sp::UPDATE_STAT && update_stat(&a.payload) == [skill, xp, level]
        }
    };
    let inv = |slot, obj, count| {
        move |a: &Arrival| {
            a.opcode == sp::UPDATE_INV_PARTIAL
                && update_inv_partial(&a.payload) == (BACKPACK_INV, vec![[slot, obj, count]])
        }
    };
    let msg = |text: &'static str| {
        move |a: &Arrival| {
            a.opcode == sp::MESSAGE_GAME && message_game(&a.payload) == (0, text.to_string())
        }
    };
    let woodcutting = find(arrivals, 0, "Woodcutting 30", &stat(WOODCUTTING, 13363, 30))?;
    let mining = find(arrivals, woodcutting, "Mining 20", &stat(MINING, 4470, 20))?;
    let fishing = find(arrivals, mining, "Fishing 20", &stat(FISHING, 4470, 20))?;
    let hatchet = find(arrivals, fishing, "hatchet", &inv(0, BRONZE_HATCHET, 1))?;
    let rod = find(arrivals, hatchet, "rod", &inv(1, FLY_FISHING_ROD, 1))?;
    let feathers = find(arrivals, rod, "feathers", &inv(2, FEATHER, 50))?;
    let rock = find(arrivals, feathers, "the rock", &|a| {
        a.opcode == sp::LOC_ADD_CHANGE
    })?;
    let mut at = rock + 1;
    let mut chops = Vec::new();
    for (n, xp) in [(0, 13430), (1, 13498)] {
        let swing = find(arrivals, at, "swing", &msg(SWING))?;
        let nest = find(arrivals, swing, "nest", &msg(NEST))?;
        let logs = find(arrivals, nest, "logs", &inv(3 + n, WILLOW_LOGS, 1))?;
        let stat_at = find(arrivals, logs, "XP", &stat(WOODCUTTING, xp, 30))?;
        let success = find(arrivals, stat_at, "success", &msg(SUCCESS))?;
        let stump = find(arrivals, success, "stump", &|a| {
            a.opcode == sp::LOC_ADD_CHANGE
        })?;
        let crown_del = find(arrivals, stump, "crown removal", &|a| {
            a.opcode == sp::LOC_DEL
        })?;
        // The tree and its crown come back as two LOC_ADD_CHANGE; the first is the tree.
        let respawn = find(arrivals, crown_del, "tree back", &|a| {
            a.opcode == sp::LOC_ADD_CHANGE
        })?;
        chops.push(Chop {
            swing,
            logs,
            stat: stat_at,
            success,
            crown_del,
            respawn,
        });
        at = respawn + 2;
    }
    let second = chops.pop().context("second chop")?;
    let first = chops.pop().context("first chop")?;
    Ok(Events {
        stats: [woodcutting, mining, fishing],
        tools: [hatchet, rod, feathers],
        rock,
        chops: [first, second],
    })
}

fn fixture() -> anyhow::Result<(PathBuf, Trace)> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    Ok((root, trace))
}

/// One recorded cycle through the production owners, the pick frame installed
/// while the mouse is on the willow, checking the exact outgoing bytes.
fn replay_cycle(replay: &mut Replay, trace: &Trace, cycle: i32) -> anyhow::Result<Vec<u8>> {
    let hovering = PRESS_CYCLES
        .iter()
        .chain(&WALK_PRESS_CYCLES)
        .any(|press| (press - 10..=press + 1).contains(&cycle));
    let written = if hovering {
        replay_pick_cycle(replay, trace, cycle)?
    } else {
        replay.cycle(trace, cycle)?.written
    };
    let (ours, recorded) = (
        mask_wall_clock(&written)?,
        mask_wall_clock(&trace.bytes(b"OUT ", cycle))?,
    );
    if WALK_PRESS_CYCLES.contains(&(cycle - 1)) {
        // The walk's destination is the ground tile under the mouse, which the
        // renderer's terrain pick gives (not run headless): compare the frames
        // with the destination left out, and that both are walks.
        let frames = |frames: &[(u8, Vec<u8>)]| -> Vec<(u8, Vec<u8>)> {
            frames
                .iter()
                .map(|(op, payload)| (*op, if *op == 33 { vec![] } else { payload.clone() }))
                .collect()
        };
        assert_eq!(
            frames(&ours),
            frames(&recorded),
            "cycle {cycle}: walk click"
        );
        assert!(
            frames(&recorded).iter().any(|f| f.0 == 33),
            "cycle {cycle}: a walk"
        );
    } else {
        assert_eq!(
            ours, recorded,
            "cycle {cycle}: client packets differ from the recording"
        );
    }
    Ok(written)
}

/// The woodcutting scenario's pick cycle: its hover window is its own, so the
/// pick frame is installed here and the cycle run through the same owners.
fn replay_pick_cycle(replay: &mut Replay, trace: &Trace, cycle: i32) -> anyhow::Result<Vec<u8>> {
    use super::scenario_tests::install_pick_frame;
    ui(replay).paint(cycle, true, [0.; 3])?;
    install_pick_frame(replay)?;
    let mut scene = replay.assets.scene_graph.take().context("scene graph")?;
    let out = replay.cycle_with_picks(trace, cycle, &mut |_, rt| {
        let mouse = rt.input.click.unwrap_or(rt.engine.platform.mouse);
        if let Some(frame) = rt.engine.scene.player_picks.as_mut() {
            frame.refresh_locs(&mut scene, mouse);
        }
    });
    replay.assets.scene_graph = Some(scene);
    Ok(out?.written)
}

fn backpack(replay: &mut Replay) -> anyhow::Result<Vec<i32>> {
    let code: Vec<(&str, i32)> = (0..5)
        .flat_map(|slot| {
            [
                ("push_constant_int", BACKPACK_INV),
                ("push_constant_int", slot),
                ("inv_getobj", 0),
            ]
        })
        .collect();
    cs2_ints(replay, &code)
}

fn skills(replay: &mut Replay) -> anyhow::Result<Vec<i32>> {
    let code: Vec<(&str, i32)> = [WOODCUTTING, MINING, FISHING]
        .into_iter()
        .flat_map(|skill| {
            [
                ("push_constant_int", skill),
                ("stat_base", 0),
                ("push_constant_int", skill),
                ("stat_visible_xp", 0),
            ]
        })
        .collect();
    cs2_ints(replay, &code)
}

/// The recorded gathering session on the real client, replayed cycle by cycle
/// with the exact outgoing bytes of every cycle.
///
/// Expected values, none from the client under test:
/// - menu: with the mouse on the willow the left click is "Chop down" (cache loc
///   38616 ops `[Chop down, .., Examine]`, op 0 is action 3);
/// - OPLOC1 (client packet 12) `p1_alt2(ctrl 0) p2(z) p4(38616) p2_alt3(x)` of the
///   willow tile, flushed the cycle after each press; the two other presses of
///   the session are walk clicks and send no OPLOC;
/// - positions: the dev server's own trace (`server-trace.jsonl`), one entry per
///   PLAYER_INFO;
/// - chop seq 21191 (offhand 1351) from each swing message until the log resets it;
/// - chat: MESSAGE_GAME type 0 texts of the server content, in order, twice;
/// - the levels the `setxp` commands set (30, 20, 20) and the tools `invset` gave,
///   read with CS2 `stat_base` / `inv_getobj`; each log into the next free backpack
///   slot (3, then 4) and the XP of a willow log, 67.5 carried in tenths (13430,
///   then 13498), read with CS2 `stat_visible_xp`;
/// - zone state in the client's loc-change snapshot: the added rock stands on its
///   tile throughout; the willow's tile holds another loc between the fall and the
///   growing back, and the crown above it (level 1) is removed; both come back.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_gathering_session_chops_the_willow_twice_and_it_grows_back() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let (root, trace) = fixture()?;
    let arrivals = arrivals(&trace)?;
    let (server_players, _) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let e = events(&arrivals)?;
    let mut replay = Replay::start(&trace)?;
    let mut done = 0;
    let mut oplocs = Vec::new();
    let mut swing_cycle = [None; 2];
    let mut success_cycle = [None; 2];
    let mut chop_anim = [None; 2];
    let mut anim_reset = [None; 2];
    let mut stumps = Vec::new();
    for cycle in 1..=trace.last_cycle() {
        let written = replay_cycle(&mut replay, &trace, cycle)?;
        oplocs.extend(
            client_frames(&written)?
                .into_iter()
                .filter(|(op, _)| [12u8, 96, 27, 61, 119, 39, 21].contains(op))
                .map(|f| (cycle, f)),
        );
        let start = done;
        done = replay.processed(&arrivals);
        let has = |index: usize| done > index;
        let fresh = |index: usize| (start..done).contains(&index);

        // The mouse on the willow: "Chop down" is the left click.
        if PRESS_CYCLES.contains(&(cycle + 1)) {
            let m = &ui(&mut replay).state.minimenu;
            let active = m.active.map(|a| m.entry(a).clone()).context("active")?;
            assert_eq!(
                (active.op.as_str(), (active.entity_id >> 32) as i32),
                ("Chop down", WILLOW),
                "cycle {cycle}: the left click on the willow"
            );
        }

        // Positions against the server's own trace, one per PLAYER_INFO.
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

        // The chop sequence runs from each swing message to the log.
        for (n, chop) in e.chops.iter().enumerate() {
            if has(chop.swing) && swing_cycle[n].is_none() {
                swing_cycle[n] = Some(cycle);
            }
            if has(chop.success) && success_cycle[n].is_none() {
                success_cycle[n] = Some(cycle);
            }
            if has(chop.swing) && local.main_anim == CHOP_SEQ && chop_anim[n].is_none() {
                chop_anim[n] = Some(cycle);
            }
            if chop_anim[n].is_some() && local.main_anim != CHOP_SEQ && anim_reset[n].is_none() {
                anim_reset[n] = Some(cycle);
            }
        }

        // Chat history (MESSAGE_GAME type 0).
        let mut expected_chat = Vec::new();
        for chop in &e.chops {
            if has(chop.swing) {
                expected_chat.push((0, SWING.to_string()));
            }
            if has(chop.logs) {
                expected_chat.push((0, NEST.to_string()));
            }
            if has(chop.success) {
                expected_chat.push((0, SUCCESS.to_string()));
            }
        }
        assert_eq!(
            chat(ui(&mut replay)),
            expected_chat,
            "cycle {cycle}: chat history"
        );

        // Backpack, stats and zone state, every 25 cycles and on the cycles
        // their packets are processed.
        let probe = cycle % 25 == 0
            || e.stats
                .iter()
                .chain(&e.tools)
                .chain([&e.rock])
                .chain(
                    e.chops
                        .iter()
                        .flat_map(|c| [&c.logs, &c.crown_del, &c.respawn]),
                )
                .any(|&i| fresh(i));
        if !probe {
            continue;
        }
        let mut expected_pack = vec![-1; 5];
        for (slot, (index, obj)) in [
            (0, (e.tools[0], BRONZE_HATCHET)),
            (1, (e.tools[1], FLY_FISHING_ROD)),
            (2, (e.tools[2], FEATHER)),
            (3, (e.chops[0].logs, WILLOW_LOGS)),
            (4, (e.chops[1].logs, WILLOW_LOGS)),
        ] {
            if has(index) {
                expected_pack[slot] = obj;
            }
        }
        assert_eq!(
            backpack(&mut replay)?,
            expected_pack,
            "cycle {cycle}: backpack"
        );
        let xp = if has(e.chops[1].stat) {
            13498
        } else if has(e.chops[0].stat) {
            13430
        } else {
            13363
        };
        let set = |index: usize, level, xp| if has(index) { [level, xp] } else { [1, 0] };
        let (wc, mining, fishing) = (
            set(e.stats[0], 30, xp),
            set(e.stats[1], 20, 4470),
            set(e.stats[2], 20, 4470),
        );
        assert_eq!(
            skills(&mut replay)?,
            [wc, mining, fishing].concat(),
            "cycle {cycle}: stat_base / stat_visible_xp of Woodcutting, Mining, Fishing"
        );

        let game = replay.game();
        if has(e.rock) {
            assert_eq!(
                loc_request(game, ROCK_TILE).map(|r| r.0),
                Some(IRON_ROCK),
                "cycle {cycle}: the added rock stands"
            );
        }
        let crown_removed = game
            .runtime
            .feed
            .state
            .zones
            .locations
            .iter()
            .any(|r| r.level == 1 && r.id == -1);
        for chop in &e.chops {
            let fallen = has(chop.crown_del) && !has(chop.respawn);
            let willow = loc_request(game, WILLOW_TILE).map(|r| r.0);
            if fallen {
                assert!(crown_removed, "cycle {cycle}: the crown is removed");
                assert!(
                    willow.is_some_and(|id| id != WILLOW),
                    "cycle {cycle}: a stump on the willow's tile, not {willow:?}"
                );
                stumps.extend(willow);
            }
        }
        if has(e.chops[0].respawn) && !has(e.chops[1].crown_del) {
            assert_eq!(willow_id(game), Some(WILLOW), "cycle {cycle}: grown back");
        }
    }

    // OPLOC1 on the willow, twice, flushed the cycle after each press.
    let mut expected = vec![writers::p1_alt2(0)];
    expected.extend(writers::p2(WILLOW_TILE[1]));
    expected.extend(writers::p4(WILLOW));
    expected.extend(writers::p2_alt3(WILLOW_TILE[0]));
    assert_eq!(
        oplocs,
        PRESS_CYCLES
            .iter()
            .map(|press| (press + 1, (12u8, expected.clone())))
            .collect::<Vec<_>>(),
        "OPLOC1 on the socket"
    );

    // The chop seq arrives with each swing and plays until the log resets it.
    assert!(swing_cycle.iter().all(Option::is_some));
    assert_eq!(chop_anim, swing_cycle, "chop seq with the swing message");
    assert_eq!(anim_reset, success_cycle, "chop seq reset on the log tick");

    // The same stump both times; the willow is back at the end.
    assert!(!stumps.is_empty() && stumps.iter().all(|&id| id == stumps[0]));
    assert_eq!(willow_id(replay.game()), Some(WILLOW), "respawned");
    Ok(())
}

fn willow_id(game: &crate::client_game::ClientGame) -> Option<i32> {
    loc_request(game, WILLOW_TILE).map(|r| r.0)
}

/// Between the fall and the growing back: the stump's menu has no "Chop down".
///
/// Expected: the stump is a loc of the cache named as a stump with no options,
/// so the scene options add only its Examine (1002) with Face here / Walk here,
/// and the left click walks.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_gathering_stump_cannot_be_chopped() -> anyhow::Result<()> {
    let (_, trace) = fixture()?;
    let arrivals = arrivals(&trace)?;
    let e = events(&arrivals)?;
    let mut replay = Replay::start(&trace)?;
    let mut cycle = 0;
    let mut settled = None;
    while settled.is_none_or(|at| cycle < at + 25) {
        cycle += 1;
        replay_cycle(&mut replay, &trace, cycle)?;
        let done = replay.processed(&arrivals);
        if settled.is_none() && done > e.chops[0].crown_del {
            settled = Some(cycle);
        }
        anyhow::ensure!(done <= e.chops[0].respawn, "grown back before the check");
    }
    let stump = willow_id(replay.game()).context("the stump")?;
    assert_ne!(stump, WILLOW);
    let mut cycle = 100_000;
    let entries = menu_on_loc(&mut replay, &mut cycle, stump)?;
    assert!(
        entries.iter().all(|(op, ..)| op != "Chop down")
            && entries.iter().any(|(op, target, action)| op == "Examine"
                && target.to_lowercase().contains("stump")
                && *action == 1002),
        "the stump's menu {entries:?}"
    );
    let m = &ui(&mut replay).state.minimenu;
    assert_eq!(
        m.active.map(|a| m.entry(a).op.clone()).as_deref(),
        Some("Walk here")
    );
    Ok(())
}
