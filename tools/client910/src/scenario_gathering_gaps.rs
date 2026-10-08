//! The gathering gaps, client side: one recorded dev-server session beside the
//! Catherby fruit tree patch in which the real client mines a copper rock that
//! lights another as a rockertunity (the client's own spot on its tile, sent to
//! this player alone) and mines the lit one, which puts it out; catches a baby
//! impling into its jar; fills the ore box from the backpack (the box's copper
//! varbit, which the client's own ore box scripts read); rakes the patch,
//! plants an apple sapling, grows it and checks its health (the patch's varbit
//! picks each form); replayed through the production owners (`session_replay`).
//!
//! Fixture: `fixtures/session-replay/gathering-gaps/` (`record.sh`; the release
//! client against the dev lobby/world with the TEST-ONLY `ALTO_SKILLS_ALWAYS=1`,
//! so every roll goes the player's way). The character is placed by a written
//! save; the ore box's Fill is a UI operation; loc and NPC options are the dev
//! server's `oploc`, `useloc` and `opnpc` commands (the added rocks and NPC are
//! not the script's to pick, and the patch forms are animated locs); `farmtick`
//! runs growth windows.
use super::scenario_tests::ui;
use super::scenario_woodcutting::{chat, cs2_ints, message_game, update_stat};
use super::session_replay::{
    arrivals, mask_wall_clock, observe, server_trace_file, Arrival, Replay, Trace,
};
use super::*;
use rs910_symbols::{inv, loc, location, npc, obj, spot, varbit};

const FIXTURE: &str = "fixtures/session-replay/gathering-gaps";
const BACKPACK_INV: i32 = inv::BACKPACK.id();
/// Stat ids (the cache's stat names, enum 680): Mining, Farming, Hunter.
const MINING: i32 = 14;
const FARMING: i32 = 19;
const HUNTER: i32 = 21;
/// The two copper rocks the recording adds (2x2), south-west of the stand:
/// the second is the one the first lights.
const FIRST_ROCK: [i32; 2] = [
    location::CATHERBY_FRUIT_TREE_PATCH_WEST.x() - 4,
    location::CATHERBY_FRUIT_TREE_PATCH_WEST.z() - 5,
];
const SECOND_ROCK: [i32; 2] = [
    location::CATHERBY_FRUIT_TREE_PATCH_WEST.x() - 4,
    location::CATHERBY_FRUIT_TREE_PATCH_WEST.z() - 2,
];

/// Server content texts (Mining, Hunter and Farming modules; unverified against 910).
const TEXTS: [&str; 8] = [
    "You swing your pickaxe at the rock.",
    "You swing your pickaxe at the rock.",
    "You manage to mine some copper.",
    "You manage to catch the baby impling.",
    "You put 3 ores into your ore box.",
    "The patch is now clear of weeds.",
    "You plant the apple tree seed in the patch.",
    "You examine the tree for signs of disease and find that it is in perfect health.",
];

/// The client packets of a cycle without the round-trip reports.
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

/// The spots the client keeps on map tiles: `[x, z, spot]`, absolute tiles.
fn tile_spots(game: &crate::client_game::ClientGame) -> Vec<[i32; 3]> {
    let base = [game.runtime.map.base_x, game.runtime.map.base_z];
    let mut spots: Vec<[i32; 3]> = game
        .runtime
        .feed
        .state
        .zones
        .transients
        .spots
        .iter()
        .map(|s| {
            [
                base[0] + (s.key >> 16) as i32,
                base[1] + (s.key & 0xffff) as i32,
                s.effect,
            ]
        })
        .collect();
    spots.sort_unstable();
    spots
}

/// The form the client shows for the fruit tree patch at `value`.
fn patch_form(replay: &mut Replay, value: i32) -> anyhow::Result<i32> {
    let locs = ui(replay)
        .engine
        .configs
        .locs
        .clone()
        .context("loc config")?;
    let parent = locs
        .get(u32::try_from(loc::CATHERBY_FRUIT_TREE_PATCH.id())?)
        .context("the patch loc")?;
    let bit = varbit::CATHERBY_FRUIT_TREE_PATCH_STATE.id();
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
/// - the spots on map tiles: none, then the cache's large rockertunity spot on
///   the second rock's tile (the first rock's first swing lights it), then none
///   once the second rock is mined (and its own swing lights the first rock);
/// - the baby impling: present once added, gone once caught;
/// - the ore box's copper varbit: 0, then 2 (the two copper ores of the save);
/// - the fruit tree patch's varbit, as the client holds it: 0, 1, 2 (weeds
///   raked), 3 (clear), 8 (the apple sapling planted), 34 (six growth windows:
///   grown, Check-health), 20 (checked: six apples), and the patch's form for
///   each is the cache's;
/// - XP from the UPDATE_STAT frames: Hunter (the catch), Farming 9730 (the
///   `setxp` command) + 3 weeds x 4 + 22 for the sapling, then the check;
/// - the backpack at the end (CS2 `inv_getobj`): the box, the copper gone, the
///   baby impling jar where the empty jar was.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_gathering_gaps_session_rockertunity_ore_box_impling_and_fruit_tree(
) -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let arrivals = arrivals(&trace)?;
    let (server_players, _) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let mut replay = Replay::start(&trace)?;
    let mut done = 0;
    let (mut spots, mut box_copper, mut patch, mut implings) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
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

        track(&mut spots, tile_spots(replay.game()));
        track(
            &mut implings,
            seen.npcs
                .values()
                .any(|(t, _)| *t == npc::BABY_IMPLING.id()),
        );
        track(
            &mut box_copper,
            client_varbit(&mut replay, varbit::ORE_BOX_COPPER)?,
        );
        track(
            &mut patch,
            client_varbit(&mut replay, varbit::CATHERBY_FRUIT_TREE_PATCH_STATE)?,
        );
    }

    let texts: Vec<String> = arrivals
        .iter()
        .filter(|a| a.opcode == sp::MESSAGE_GAME)
        .map(|a| message_game(&a.payload).1)
        .collect();
    let mut code = Vec::new();
    for slot in 0..7 {
        code.extend([
            ("push_constant_int", BACKPACK_INV),
            ("push_constant_int", slot),
            ("inv_getobj", 0),
        ]);
    }
    let pack = cs2_ints(&mut replay, &code)?;

    // The rockertunity: the first rock's swing lights the second (the large spot, a 2x2 rock), the swing at the
    // second puts it out and lights the first, which fades unmined.
    let large = spot::ROCKERTUNITY_LARGE_ROCK.id();
    assert_eq!(
        spots,
        [
            vec![],
            vec![[SECOND_ROCK[0], SECOND_ROCK[1], large]],
            vec![[FIRST_ROCK[0], FIRST_ROCK[1], large]],
            vec![],
        ],
        "the spots on map tiles"
    );
    // The impling: added, caught, back after its respawn.
    assert_eq!(implings, [false, true, false, true], "the baby impling");
    // The ore box: the save's two copper ores and the one the lit swing mined.
    assert_eq!(box_copper, [0, 3], "the ore box's copper");
    // The patch: weeds raked, the sapling planted, six windows (grown), checked.
    assert_eq!(
        patch,
        [0, 1, 2, 3, 8, 34, 20],
        "the fruit tree patch's varbit"
    );
    let forms: Vec<i32> = [0, 3, 8, 34, 20]
        .into_iter()
        .map(|value| patch_form(&mut replay, value))
        .collect::<anyhow::Result<_>>()?;
    assert_eq!(
        forms,
        [
            loc::FRUIT_TREE_PATCH_OVERGROWN.id(),
            loc::FRUIT_TREE_PATCH_CLEAR.id(),
            loc::APPLE_TREE_PLANTED.id(),
            loc::APPLE_TREE_GROWN_UNCHECKED.id(),
            loc::APPLE_TREE_SIX_APPLES.id(),
        ],
        "the patch's forms"
    );
    assert_eq!(texts, TEXTS);
    assert_eq!(
        pack,
        [
            obj::BRONZE_ORE_BOX.id(),
            obj::WEEDS.id(),
            -1,
            obj::BABY_IMPLING_JAR.id(),
            obj::RAKE.id(),
            obj::SPADE.id(),
            -1,
        ],
        "backpack slots 0..6"
    );
    // XP: the catch (wiki baby impling, Gielinor: 25); Farming 9730 (`setxp`) + 3 weeds x 4 + 22 to plant, then
    // 1199.5 for the check; the swing at the lit rock earns about four times a plain one.
    assert_eq!(
        stat_updates(&arrivals, HUNTER).last(),
        Some(&[37224 + 25, 40]),
        "Hunter XP"
    );
    let farming = stat_updates(&arrivals, FARMING);
    assert!(
        farming.contains(&[9730 + 12 + 22, 27]),
        "Farming: weeds and the sapling"
    );
    assert_eq!(
        farming.last(),
        Some(&[9730 + 12 + 22 + 1199, 28]),
        "Farming: the check"
    );
    let mining: Vec<i32> = stat_updates(&arrivals, MINING)
        .iter()
        .map(|row| row[0])
        .collect();
    let (plain, lit) = (mining[1] - mining[0], mining[2] - mining[1]);
    assert!(
        (3 * plain..=5 * plain).contains(&lit),
        "Mining XP: a plain swing {plain}, the lit one {lit}"
    );
    Ok(())
}
