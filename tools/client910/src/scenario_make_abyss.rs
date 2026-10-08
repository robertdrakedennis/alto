//! Make-X quantity, a fire that burns out, a runecrafting pouch and the
//! Abyss, client side: one recorded dev-server session in which the real
//! client lights a fire, chooses five of six on Make-X with the quantity
//! slider's Decrease button and makes five, fills, empties and fills its small
//! pouch, sees the fire become a heap of ashes, then is teleported into the
//! Abyss by the Mage of Zamorak and mines through a rock into the inner ring;
//! replayed through the production owners (`session_replay`).
//!
//! Fixture: `fixtures/session-replay/make-and-abyss/` (`record.sh`; the
//! release client against the dev lobby/world with the TEST-ONLY
//! `ALTO_SKILLS_ALWAYS=1`, so every roll goes the player's way). The character
//! is placed by a written save in the field south of Lumbridge; the backpack's
//! options, the product row, the slider and the Make button are UI operations;
//! the mage (added by `npcadd`) and the rock are server commands (`opnpc`,
//! `oploc`), as the client's menu would send them.
use super::scenario_tests::ui;
use super::scenario_woodcutting::{
    chat, cs2_ints, loc_request, message_game, update_stat, FrameReader,
};
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Arrival, Replay, Trace,
};
use super::*;
use rs910_symbols::{component, inv, loc, location, obj, varbit, varp};

const FIXTURE: &str = "fixtures/session-replay/make-and-abyss";
const BACKPACK_INV: i32 = inv::BACKPACK.id();
/// The fire's tile: where the character stands when it lights the logs (the save).
const FIRE_TILE: [i32; 3] = [
    location::LUMBRIDGE_SWAMP_MINE_EAST.x(),
    location::LUMBRIDGE_SWAMP_MINE_EAST.z(),
    location::LUMBRIDGE_SWAMP_MINE_EAST.level(),
];
/// Stat ids (the cache's stat names, enum 680): Mining, Fletching.
const MINING: i32 = 14;
const FLETCHING: i32 = 9;

/// The client packets of a cycle without the round-trip reports (the session is
/// longer than the 30 s the client waits between two, and the recording
/// machine's ping answered some cycles after the replay's fixed answer) and
/// without the keep-alives: a `NO_TIMEOUT` goes out after fifty cycles in which
/// nothing was sent, so it moves with the reports, and this session idles for
/// minutes while the fire burns.
fn without_reports(bytes: &[u8]) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    use crate::proto::client as cp;
    Ok(mask_wall_clock(bytes)?
        .into_iter()
        .filter(|(op, _)| ![cp::PING_STATISTICS, cp::NO_TIMEOUT].contains(op))
        .collect())
}

/// A varbit of the local player as the client holds it.
fn client_varbit(replay: &mut Replay, id: rs910_symbols::VarbitId) -> anyhow::Result<i32> {
    let session = replay.core.session.as_mut().context("session")?;
    let game = session.game.as_mut().context("game")?;
    let id = u16::try_from(id.id())?;
    crate::client_game::with_game(game, |vars| vars.get_bit(id, false))
}

/// The ground stack the client holds on a tile: `(obj, count)` by rank.
fn stack_at(game: &crate::client_game::ClientGame, tile: [i32; 3]) -> Vec<(i32, i32)> {
    let key = i64::from(tile[2] & 3) << 28
        | i64::from(tile[1] & 0x3fff) << 14
        | i64::from(tile[0] & 0x3fff);
    game.runtime
        .feed
        .state
        .zones
        .objects
        .stacks
        .get(&key)
        .map(|stack| stack.iter().map(|o| (o.id, o.count)).collect())
        .unwrap_or_default()
}

/// `VARP_SMALL`: `(varp, value)` (p1 value, p2_alt2 varp).
fn varp_small(payload: &[u8]) -> [i32; 2] {
    let mut r = FrameReader(payload, 0);
    let value = i32::from(r.g1() as i8);
    [r.g2_alt2(), value]
}

/// `IF_BUTTON1`: `(component, child)` (p2_alt3 obj, p2_alt2 child, p4 component).
fn if_button(payload: &[u8]) -> (i32, i32) {
    let mut r = FrameReader(payload, 2);
    let child = r.g2_alt2();
    (r.g4s(), child)
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

/// Push `value` onto `seen` when it differs from the last one.
fn track<T: PartialEq>(seen: &mut Vec<T>, value: T) {
    if seen.last() != Some(&value) {
        seen.push(value);
    }
}

/// The recorded session on the real client, cycle by cycle, with the exact
/// outgoing bytes of every cycle.
///
/// Expected values, none from the client under test:
/// - positions: the dev server's own trace (`server-trace.jsonl`), one entry per
///   PLAYER_INFO; chat: the MESSAGE_GAME texts of the server content, in order;
/// - Make-X: the client sends IF_BUTTON1 on the second product row (1371:22
///   child 5) and on the slider's Decrease button (1371:19 child 0) and
///   RESUME_PAUSEBUTTON once; the server's amount varp goes 6 (six logs: make
///   all), 6 (the shortbow selected), 5 (Decrease); five unstrung shortbows are
///   made (5 Fletching XP each);
/// - the fire's tile in the client's loc-change snapshot: the fire, then
///   nothing once it has burnt out, and a heap of ashes on the ground there;
/// - the small pouch's count varbit as the client holds it: 0, 3, 0, 3;
/// - the Abyss: the miniquest varp at 4 (the mage's Teleport form), the
///   player's tile in front of the rock of slot 1 and then on its inside tile,
///   the ring varbit 0 (the layout), the rock at work, then broken; 25 Mining
///   XP.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_session_makes_five_burns_out_fills_a_pouch_and_crosses_the_abyss() -> anyhow::Result<()>
{
    use crate::proto::{client as cp, server as sp};
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let arrivals = arrivals(&trace)?;
    let (server_players, _) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let mut replay = Replay::start(&trace)?;
    let mut done = 0;
    let (mut buttons, mut pauses) = (Vec::new(), 0);
    let (mut fire, mut ashes, mut pouch, mut ring, mut tiles) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            without_reports(&out.written)?,
            without_reports(&trace.bytes(b"OUT ", cycle))?,
            "cycle {cycle}: client packets differ from the recording"
        );
        for (op, payload) in client_frames(&out.written)? {
            if op == cp::IF_BUTTON1 {
                buttons.push(if_button(&payload));
            }
            pauses += i32::from(op == cp::RESUME_PAUSEBUTTON);
        }
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
        // Above ground (the Abyss lies far north, z 4800 and more): the fire's tile.
        if local.tile[1] < 4000 {
            track(
                &mut fire,
                loc_request(replay.game(), FIRE_TILE).map(|r| r.0),
            );
            track(&mut ashes, stack_at(replay.game(), FIRE_TILE));
        }
        track(
            &mut pouch,
            client_varbit(&mut replay, varbit::SMALL_POUCH_ESSENCE)?,
        );
        track(
            &mut ring,
            client_varbit(&mut replay, varbit::ABYSS_OBSTACLE_RING)?,
        );
        track(&mut tiles, local.tile);
    }

    // Make-X: the product row and the slider's Decrease reach the server; the Make button once.
    let list = component::make_x_products::LIST.packed();
    let slider = component::make_x_products::QUANTITY_SLIDER.packed();
    assert_eq!(
        buttons
            .iter()
            .filter(|(packed, _)| [list, slider].contains(packed))
            .copied()
            .collect::<Vec<_>>(),
        [(list, 5), (slider, 0)],
        "IF_BUTTON1 on the product list and the slider"
    );
    assert_eq!(pauses, 1, "RESUME_PAUSEBUTTON");
    let amounts: Vec<i32> = arrivals
        .iter()
        .filter(|a| a.opcode == sp::VARP_SMALL)
        .map(|a| varp_small(&a.payload))
        .filter(|[id, _]| *id == varp::MAKE_X_AMOUNT.id())
        .map(|[_, value]| value)
        .collect();
    assert_eq!(amounts, [6, 6, 5], "the amount varp");
    assert_eq!(
        stat_updates(&arrivals, FLETCHING).last(),
        Some(&[25, 1]),
        "Fletching: five unstrung shortbows"
    );
    let mut code = Vec::new();
    for slot in 4..=10 {
        code.extend([
            ("push_constant_int", BACKPACK_INV),
            ("push_constant_int", slot),
            ("inv_getobj", 0),
        ]);
    }
    let pack = cs2_ints(&mut replay, &code)?;
    assert_eq!(
        pack.iter()
            .filter(|&&id| id == obj::SHORTBOW_U.id())
            .count(),
        5,
        "five unstrung shortbows in the backpack: {pack:?}"
    );

    // The fire burns out (the teleport into the Abyss later drops the tile from the scene), and the client holds
    // a heap of ashes where it stood.
    assert_eq!(
        fire[..3],
        [None, Some(loc::LOGS_FIRE.id()), Some(-1)],
        "the fire's tile: {fire:?}"
    );
    assert_eq!(
        ashes[..2],
        [vec![], vec![(obj::ASHES.id(), 1)]],
        "the ashes on the fire's tile: {ashes:?}"
    );

    // The pouch: filled with three, emptied, filled again.
    assert_eq!(pouch, [0, 3, 0, 3], "the small pouch's count");

    // The Abyss: the mage's form, the arrival and the rock.
    let outside = location::ABYSS_SLOT_1_OUTSIDE;
    let inside = location::ABYSS_SLOT_1_INSIDE;
    assert!(
        tiles.contains(&[outside.x(), outside.z(), outside.level()]),
        "in front of the rock"
    );
    assert_eq!(
        tiles.last(),
        Some(&[inside.x(), inside.z(), inside.level()]),
        "the inner ring"
    );
    assert_eq!(ring.last(), Some(&13), "the ring varbit: {ring:?}");
    assert!(ring.contains(&12), "the rock at work: {ring:?}");
    assert_eq!(
        stat_updates(&arrivals, MINING).last(),
        Some(&[25, 1]),
        "Mining XP for the rock"
    );
    let miniquest: Vec<[i32; 2]> = arrivals
        .iter()
        .filter(|a| a.opcode == sp::VARP_SMALL)
        .map(|a| varp_small(&a.payload))
        .filter(|[id, _]| *id == varp::ABYSS_MINIQUEST.id())
        .collect();
    assert_eq!(
        miniquest,
        [[varp::ABYSS_MINIQUEST.id(), 4]],
        "the mage's varp"
    );
    Ok(())
}
