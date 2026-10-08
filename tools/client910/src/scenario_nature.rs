//! Hunter, farming and divination, client side: one recorded dev-server session
//! beside the Lumbridge tree patch in which the real client lays a bird snare
//! that catches a crimson swift and checks it, rakes the patch, plants an oak
//! sapling and sees it grow (the patch's varbit picks each stage's form), and
//! harvests a pale wisp that opens into its spring (an NPC_INFO type change),
//! then configures an energy rift in its conversion window and converts the
//! memories to energy; replayed through the production owners (`session_replay`).
//!
//! Fixture: `fixtures/session-replay/nature/` (`record.sh`; the release client
//! against the dev lobby/world with the TEST-ONLY `ALTO_SKILLS_ALWAYS=1`, so
//! every roll goes the player's way). The character is placed by a written save
//! on the path south of the patch; the snare's Lay and the window's energy
//! button are UI operations; loc and NPC options are the dev server's `oploc`,
//! `useloc` and `opnpc` commands (the snare, the rift and the patch forms are
//! animated locs the headless replay cannot pick, and an added NPC's index is
//! not the script's to know); `farmtick` runs growth windows.
use super::scenario_tests::ui;
use super::scenario_woodcutting::{
    chat, cs2_ints, if_opensub, loc_request, message_game, update_stat,
};
use super::session_replay::{
    arrivals, mask_wall_clock, observe, server_trace_file, Arrival, Replay, Trace,
};
use super::*;
use rs910_symbols::{component, interface, inv, loc, location, npc, obj, varbit};

const FIXTURE: &str = "fixtures/session-replay/nature";
/// The player's tile (the save) is where it lays the snare.
const SNARE_TILE: [i32; 3] = [
    location::LUMBRIDGE_TREE_PATCH_PATH.x(),
    location::LUMBRIDGE_TREE_PATCH_PATH.z(),
    location::LUMBRIDGE_TREE_PATCH_PATH.level(),
];
const BACKPACK_INV: i32 = inv::BACKPACK.id();
const CENTRAL_SLOT: i32 = component::game_window::CENTRAL_SLOT.packed();
const CONVERSION_WINDOW: i32 = interface::MEMORY_CONVERSION.id();
/// Stat ids (the cache's stat names, enum 680): Farming, Hunter, Divination.
const FARMING: i32 = 19;
const HUNTER: i32 = 21;
const DIVINATION: i32 = 25;
/// The conversion mode the window's energy button sets (the window's own script
/// highlights mode 2 on that button).
const ENERGY_MODE: i32 = 2;

/// Server content texts (Hunter, Farming and Divination modules; unverified against 910).
const TEXTS: [&str; 4] = [
    "You lay the trap.",
    "The patch is now clear of weeds.",
    "You plant the acorn in the patch.",
    "You harvest some energy from the spring.",
];

/// The client packets of a cycle without the round-trip reports (the session is
/// longer than the 30 s the client waits between two, and the recording machine's
/// ping answered some cycles after the replay's fixed answer).
fn without_reports(bytes: &[u8]) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    Ok(mask_wall_clock(bytes)?
        .into_iter()
        .filter(|(op, _)| *op != crate::proto::client::PING_STATISTICS)
        .collect())
}

/// A varbit of the local player as the client holds it.
fn client_varbit(replay: &mut Replay, id: rs910_symbols::VarbitId) -> anyhow::Result<i32> {
    let session = replay.core.session.as_mut().context("session")?;
    let game = session.game.as_mut().context("game")?;
    let id = u16::try_from(id.id())?;
    crate::client_game::with_game(game, |vars| vars.get_bit(id, false))
}

/// The form the client shows for the tree patch at `value`: its parent loc's
/// multiloc entry, picked the way the scene and the menus pick it.
fn patch_form(replay: &mut Replay, value: i32) -> anyhow::Result<i32> {
    let locs = ui(replay)
        .engine
        .configs
        .locs
        .clone()
        .context("loc config")?;
    let parent = locs
        .get(u32::try_from(loc::LUMBRIDGE_TREE_PATCH.id())?)
        .context("the patch loc")?;
    let bit = varbit::LUMBRIDGE_TREE_PATCH_STATE.id();
    let form = parent
        .multi_loc(&|is_bit, id| (is_bit && id == bit).then_some(value))
        .context("a form")?;
    Ok(i32::try_from(form)?)
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
/// - the snare's tile in the client's loc-change snapshot: the laid snare, then
///   the crimson swift's catching form and its full form (the cache's bird snare
///   forms that draw the swift's own model), then nothing once it is checked;
/// - the added wisp's NPC slot: a pale wisp, then a pale spring (the server's
///   type change), the spring staying for the rest of the session;
/// - the tree patch: its varbit, as the client holds it, goes 0, 1, 2 (weeds
///   raked), 3 (clear), 8 (the oak planted), 9 (one growth window), 12 (three
///   more: grown), and the patch's form for each is the cache's: the overgrown
///   patch, the clear patch, the planted oak, its first growth and the grown
///   oak waiting for a check;
/// - the conversion window (interface 131) is mounted in the central slot once
///   and closed by its energy button, which leaves the mode varbit at 2;
/// - XP from the UPDATE_STAT frames: Hunter 20 (the swift), Farming 2411 (the
///   `setxp` command) + 3 weeds x 4 + 14 for the sapling = 2437, Divination one
///   XP per harvest and one per memory converted to energy;
/// - the backpack at the end (CS2 `inv_getobj`): the check's bones in the
///   snare's old slot, the rake and the spade, the pale energy where the sapling
///   was, then the red feathers, the raw bird meat, the snare back and the weeds.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_nature_session_snares_a_swift_grows_an_oak_and_converts_wisp_memories(
) -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let arrivals = arrivals(&trace)?;
    let (server_players, _) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let mut replay = Replay::start(&trace)?;
    let mut done = 0;
    let (mut snare, mut wisp_types, mut patch, mut windows) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut wisp_slot = None;
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            without_reports(&out.written)?,
            without_reports(&trace.bytes(b"OUT ", cycle))?,
            "cycle {cycle}: client packets differ from the recording"
        );
        let start = done;
        done = replay.processed(&arrivals);
        let seen = observe(replay.game());
        let local = seen.local.context("local player")?;
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

        track(
            &mut snare,
            loc_request(replay.game(), SNARE_TILE).map(|r| r.0),
        );
        if wisp_slot.is_none() {
            wisp_slot = seen
                .npcs
                .iter()
                .find(|(_, (t, _))| *t == npc::PALE_WISP.id())
                .map(|(&i, _)| i);
        }
        if let Some((t, _)) = wisp_slot.and_then(|slot| seen.npcs.get(&slot)) {
            track(&mut wisp_types, *t);
        }
        track(
            &mut patch,
            client_varbit(&mut replay, varbit::LUMBRIDGE_TREE_PATCH_STATE)?,
        );
        let mounted = ui(&mut replay)
            .state
            .life
            .subs
            .get(CENTRAL_SLOT)
            .map(|s| s.borrow().id)
            == Some(CONVERSION_WINDOW);
        track(&mut windows, mounted);
    }

    // The snare: laid, springing on the swift, holding it, gone once checked.
    assert_eq!(
        snare,
        [
            None,
            Some(loc::BIRD_SNARE_LAID.id()),
            Some(loc::BIRD_SNARE_CATCHING_CRIMSON_SWIFT.id()),
            Some(loc::BIRD_SNARE_HOLDING_CRIMSON_SWIFT.id()),
            Some(-1),
        ],
        "the snare's tile"
    );
    // The wisp opens into its spring and stays one.
    assert_eq!(
        wisp_types,
        [npc::PALE_WISP.id(), npc::PALE_SPRING.id()],
        "the wisp's NPC slot"
    );
    // The patch: weeds raked, the oak planted, one growth window, then three.
    assert_eq!(patch, [0, 1, 2, 3, 8, 9, 12], "the tree patch's varbit");
    let forms: Vec<i32> = [0, 3, 8, 9, 12]
        .into_iter()
        .map(|value| patch_form(&mut replay, value))
        .collect::<anyhow::Result<_>>()?;
    assert_eq!(
        forms,
        [
            loc::TREE_PATCH_OVERGROWN.id(),
            loc::TREE_PATCH_CLEAR.id(),
            loc::OAK_SAPLING_PLANTED.id(),
            loc::OAK_GROWING_FIRST.id(),
            loc::OAK_GROWN_UNCHECKED.id(),
        ],
        "the patch's forms"
    );
    // The conversion window: opened once, closed by its energy button.
    assert_eq!(windows, [false, true, false], "the conversion window");
    assert_eq!(
        client_varbit(&mut replay, varbit::MEMORY_CONVERSION_MODE)?,
        ENERGY_MODE,
        "the conversion mode"
    );
    let opened: Vec<[i32; 3]> = arrivals
        .iter()
        .filter(|a| a.opcode == sp::IF_OPENSUB)
        .map(|a| if_opensub(&a.payload))
        .filter(|[_, id, _]| *id == CONVERSION_WINDOW)
        .collect();
    assert_eq!(opened.len(), 1, "IF_OPENSUB of the conversion window");

    let texts: Vec<String> = arrivals
        .iter()
        .filter(|a| a.opcode == sp::MESSAGE_GAME)
        .map(|a| message_game(&a.payload).1)
        .collect();
    assert_eq!(texts, TEXTS);
    assert_eq!(
        stat_updates(&arrivals, HUNTER).last(),
        Some(&[20, 1]),
        "Hunter XP"
    );
    assert!(
        stat_updates(&arrivals, FARMING).contains(&[2411, 15]),
        "Farming 15 from the command"
    );
    assert!(
        stat_updates(&arrivals, FARMING).contains(&[2437, 15]),
        "Farming: 3 weeds and the sapling"
    );

    // Divination: as many harvests as memories, each memory converted to one energy (the lowest roll).
    let mut code = Vec::new();
    for slot in 0..8 {
        code.extend([
            ("push_constant_int", BACKPACK_INV),
            ("push_constant_int", slot),
            ("inv_getobj", 0),
        ]);
    }
    code.extend([
        ("push_constant_int", BACKPACK_INV),
        ("push_constant_int", 3),
        ("inv_getnum", 0),
    ]);
    let pack = cs2_ints(&mut replay, &code)?;
    assert_eq!(
        pack[..8],
        [
            obj::BONES.id(),
            obj::RAKE.id(),
            obj::SPADE.id(),
            obj::PALE_ENERGY.id(),
            obj::RED_FEATHER.id(),
            obj::RAW_BIRD_MEAT.id(),
            obj::BIRD_SNARE.id(),
            obj::WEEDS.id(),
        ],
        "backpack slots 0..7"
    );
    let energy = pack[8];
    let memories = energy / 2;
    assert!(
        memories > 0 && energy % 2 == 0,
        "every harvest gave a memory and an energy, every memory one energy more"
    );
    assert_eq!(
        stat_updates(&arrivals, DIVINATION).last(),
        Some(&[energy, 1]),
        "Divination XP: one per harvest and per conversion"
    );
    Ok(())
}
