//! World travel, client side: one recorded dev-server session in which the real
//! client rubs a genie's lamp through the skill choice window, buys Quick
//! Teleport charges with vis wax and quick-teleports to Burthorpe, rubs an
//! amulet of glory to Al Kharid, climbs down Pollnivneach's smokey well into the
//! Smoke Dungeon and back up its rope, and crosses the first obstacles of the
//! Agility Pyramid; replayed through the production owners (`session_replay`),
//! every map transaction included.
//!
//! Fixture: `fixtures/session-replay/world-travel/` (`record.sh`; the release
//! client against the dev lobby/world). The lamp, the wax, the lodestone window
//! and the glory are the client's own UI operations; loc options are the dev
//! server's `oploc` command and the moves between places its `tele` (the
//! headless replay picks no scene locs).
use super::scenario_tests::ui;
use super::scenario_woodcutting::{if_opensub, update_stat};
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Arrival, Replay, Trace,
};
use super::*;
use rs910_symbols::{component, interface, inv, location, obj, varbit, Location};

const FIXTURE: &str = "fixtures/session-replay/world-travel";
/// Stat ids (the cache's stat names, enum 680).
const PRAYER: i32 = 5;
const AGILITY: i32 = 16;
/// `record.sh`: `setxp 16 1210421` (Agility 75).
const AGILITY_SET: i32 = 1_210_421;

/// The client packets of a cycle without the round-trip reports (the recording
/// machine's ping answered some cycles after the replay's fixed answer) and the
/// idle keepalives (`NO_TIMEOUT` after fifty idle polls: the recording polled the
/// connection while a map transaction's acknowledgement waited on the machine's
/// scene work, which the headless replay does not wait for).
fn without_reports(bytes: &[u8]) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    use crate::proto::client as cp;
    Ok(mask_wall_clock(bytes)?
        .into_iter()
        .filter(|(op, _)| *op != cp::PING_STATISTICS && *op != cp::NO_TIMEOUT)
        .collect())
}

/// A varbit of the local player as the client holds it.
fn client_varbit(replay: &mut Replay, id: rs910_symbols::VarbitId) -> anyhow::Result<i32> {
    let session = replay.core.session.as_mut().context("session")?;
    let game = session.game.as_mut().context("game")?;
    let id = u16::try_from(id.id())?;
    crate::client_game::with_game(game, |vars| vars.get_bit(id, false))
}

/// The player's inventory `inv` as `(slot, obj, count)` of its filled slots.
fn slots(replay: &mut Replay, inv: i32) -> Vec<[i32; 3]> {
    ui(replay)
        .engine
        .inv_cache
        .inventory(inv, false)
        .map(|i| {
            i.obj_ids
                .iter()
                .zip(&i.counts)
                .enumerate()
                .filter(|(_, (&id, _))| id >= 0)
                .map(|(slot, (&id, &n))| [slot as i32, id, n])
                .collect()
        })
        .unwrap_or_default()
}

/// The stat updates of one stat in the recording: XP in order.
fn stat_xp(arrivals: &[Arrival], stat: i32) -> Vec<i32> {
    use crate::proto::server as sp;
    arrivals
        .iter()
        .filter(|a| a.opcode == sp::UPDATE_STAT)
        .map(|a| update_stat(&a.payload))
        .filter(|u| u[0] == stat)
        .map(|u| u[1])
        .collect()
}

/// A location as `[x, z, level]`.
fn tile(place: Location) -> [i32; 3] {
    [place.x(), place.z(), place.level()]
}

/// A 16-bit field as the client writes "none" (65535).
fn short(value: i32) -> i32 {
    if value == 0xffff {
        -1
    } else {
        value
    }
}

/// What the client sent: `IF_BUTTONn` as `(op, component, slot, obj)` and
/// `RESUME_PAUSEBUTTON` as `(0, component, slot, -1)`.
fn presses(written: &[u8]) -> anyhow::Result<Vec<[i32; 4]>> {
    use crate::proto::client as cp;
    let buttons = [
        cp::IF_BUTTON1,
        cp::IF_BUTTON2,
        cp::IF_BUTTON3,
        cp::IF_BUTTON4,
    ];
    Ok(client_frames(written)?
        .into_iter()
        .filter_map(|(op, p)| {
            if let Some(n) = buttons.iter().position(|&b| b == op) {
                let obj = i32::from(p[0].wrapping_sub(128)) | i32::from(p[1]) << 8;
                let slot = i32::from(p[2]) << 8 | i32::from(p[3].wrapping_sub(128));
                let component = i32::from_be_bytes([p[4], p[5], p[6], p[7]]);
                Some([n as i32 + 1, component, short(slot), short(obj)])
            } else if op == cp::RESUME_PAUSEBUTTON {
                let component = i32::from_be_bytes([p[1], p[0], p[3], p[2]]);
                let slot = i32::from(p[4]) << 8 | i32::from(p[5].wrapping_sub(128));
                Some([0, component, short(slot), -1])
            } else {
                None
            }
        })
        .collect())
}

/// One forced movement as the client holds it: absolute start and end tiles and
/// the cycles it lasts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Slide {
    start: [i32; 3],
    end: [i32; 3],
    cycles: i32,
}

/// The recorded session on the real client, cycle by cycle, with the exact
/// outgoing bytes of every cycle.
///
/// Expected values, none from the client under test:
/// - positions: the dev server's own trace (`server-trace.jsonl`), one entry per
///   PLAYER_INFO, each a map the client has installed (six map transactions
///   mid-session);
/// - the places: Burthorpe's lodestone (its location symbol), Al Kharid where
///   today's wiki's glory teleport map puts it (3305, 3123), the Smoke Dungeon
///   under Pollnivneach's smokey well where the dated wiki's dungeon page links
///   it (the world table's `wikiDungeon` rule, (3204, 9379)) and back beside the
///   well, the pyramid's foot;
/// - the lamp: the skill choice window (1263) opened by the server, the client's
///   Confirm a pause on 1263:13 as Prayer's button (enum 1482: 7), ten Prayer XP
///   (the client script's genie lamp rule: ten a level, Prayer 1);
/// - Quick Teleport: ten charges bought with one vis wax (wiki), one spent: 9;
/// - the glory: a (3) in the backpack slot the (4) was in;
/// - the pyramid: the low wall from (3354, 2848, 1) to (3354, 2850, 1) over 60
///   cycles, the ledge (3363, 2851, 1) to (3368, 2851, 1) over 150, the plank
///   (3375, 2846, 1) to (3375, 2843, 1) over 90 (the crossing table), Agility XP
///   8, 5.2 and 5.64 over the set 1,210,421 (wiki Agility Pyramid, tenths carried).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_world_travel_session_teleports_climbs_and_crosses() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let arrivals = arrivals(&trace)?;
    let (server_players, _) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let mut replay = Replay::start(&trace)?;
    let mut done = 0;
    let mut bases: Vec<[i32; 2]> = Vec::new();
    let mut tiles: Vec<[i32; 3]> = Vec::new();
    let mut sent: Vec<[i32; 4]> = Vec::new();
    let mut slides: Vec<Slide> = Vec::new();
    let mut windows: Vec<i32> = Vec::new();
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            without_reports(&out.written)?,
            without_reports(&trace.bytes(b"OUT ", cycle))?,
            "cycle {cycle}: client packets differ from the recording"
        );
        sent.extend(presses(&out.written)?);
        let start = done;
        done = replay.processed(&arrivals);
        let game = replay.game();
        let base = [game.runtime.map.base_x, game.runtime.map.base_z];
        if bases.last() != Some(&base) {
            bases.push(base);
        }
        let local = observe(game).local.context("local player")?;
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
            // The places the player stood, as the server placed it.
            if tiles.last() != Some(&local.tile) {
                tiles.push(local.tile);
            }
        }
        let state = &game.runtime.feed.state;
        let player = state.players.players[game.runtime.map.local]
            .as_ref()
            .context("local player")?;
        let f = player.forced;
        let slide = Slide {
            start: [base[0] + f[0], base[1] + f[1], f[4]],
            end: [base[0] + f[2], base[1] + f[3], f[5]],
            cycles: f[7] - f[6],
        };
        if f[7] > 0 && slides.last() != Some(&slide) {
            slides.push(slide);
        }
        for a in &arrivals[start..done] {
            if a.opcode == sp::IF_OPENSUB {
                windows.push(if_opensub(&a.payload)[1]);
            }
        }
    }

    // Seven maps: the login's and one per teleport, climb and `tele`.
    assert_eq!(bases.len(), 7, "map installs: {bases:?}");
    let near = |a: [i32; 3], b: [i32; 3], d: i32| {
        a[2] == b[2] && (a[0] - b[0]).abs() <= d && (a[1] - b[1]).abs() <= d
    };
    let burthorpe = tile(location::BURTHORPE_LODESTONE);
    assert!(
        tiles.iter().any(|&t| near(t, burthorpe, 3)),
        "beside the Burthorpe lodestone: {tiles:?}"
    );
    for place in [
        location::AL_KHARID_GLORY_TELEPORT,
        location::SMOKE_DUNGEON_ROPE_BOTTOM,
        location::AGILITY_PYRAMID_ENTRANCE,
    ] {
        assert!(tiles.contains(&tile(place)), "{place:?} in {tiles:?}");
    }
    // Back up the rope: beside the well again, after the dungeon.
    let down = tiles
        .iter()
        .position(|&t| t == tile(location::SMOKE_DUNGEON_ROPE_BOTTOM))
        .context("the dungeon")?;
    assert_eq!(tiles[down + 1], tile(location::SMOKEY_WELL_SIDE));

    // The windows: the skill choice, the options box (the wax), the lodestone network, the options box (the glory).
    let shown: Vec<i32> = windows
        .iter()
        .copied()
        .filter(|w| {
            [
                interface::SKILL_CHOICE.id(),
                interface::LODESTONE_NETWORK.id(),
                interface::DIALOGUE_OPTIONS.id(),
            ]
            .contains(w)
        })
        .collect();
    assert_eq!(
        shown,
        [
            interface::SKILL_CHOICE.id(),
            interface::DIALOGUE_OPTIONS.id(),
            interface::LODESTONE_NETWORK.id(),
            interface::DIALOGUE_OPTIONS.id()
        ]
    );
    // The client's answers, in order.
    assert_eq!(
        sent,
        [
            [
                1,
                component::backpack::SLOTS.packed(),
                0,
                obj::GENIE_LAMP.id()
            ],
            [0, component::skill_choice::CONFIRM.packed(), 7, -1],
            [1, component::backpack::SLOTS.packed(), 1, obj::VIS_WAX.id()],
            [
                0,
                component::dialogue_options::FIRST_OPTION.packed(),
                -1,
                -1
            ],
            [1, component::minimap::HOME_TELEPORT_BUTTON.packed(), -1, -1],
            [
                2,
                component::lodestone_network::BURTHORPE_BUTTON.packed(),
                -1,
                -1
            ],
            [
                4,
                component::backpack::SLOTS.packed(),
                2,
                obj::AMULET_OF_GLORY_4.id()
            ],
            [
                0,
                component::dialogue_options::FIRST_OPTION.packed() + 15,
                -1,
                -1
            ],
        ]
    );
    assert_eq!(stat_xp(&arrivals, PRAYER).last(), Some(&10), "Prayer XP");
    assert_eq!(
        client_varbit(&mut replay, varbit::QUICK_TELEPORT_CHARGES)?,
        9
    );
    assert_eq!(
        slots(&mut replay, inv::BACKPACK.id()),
        [[2, obj::AMULET_OF_GLORY_3.id(), 1]],
        "the lamp and the wax used, the glory a (3)"
    );

    // The pyramid's three crossings, as the client installed them.
    assert_eq!(
        slides,
        [
            Slide {
                start: [3354, 2848, 1],
                end: [3354, 2850, 1],
                cycles: 60
            },
            Slide {
                start: [3363, 2851, 1],
                end: [3368, 2851, 1],
                cycles: 150
            },
            Slide {
                start: [3375, 2846, 1],
                end: [3375, 2843, 1],
                cycles: 90
            },
        ]
    );
    assert_eq!(
        stat_xp(&arrivals, AGILITY),
        [
            0,
            AGILITY_SET,
            AGILITY_SET + 8,
            AGILITY_SET + 60,
            AGILITY_SET + 116
        ]
    );
    Ok(())
}
