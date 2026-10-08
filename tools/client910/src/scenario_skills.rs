//! Mining and cooking, client side: one recorded dev-server session in which
//! the real client mines a copper rock in the Lumbridge Swamp mine, then
//! cooks four raw shrimps through the Make-X interface, replayed through the
//! production owners (`session_replay`), with the renderer's headless pick
//! frame (`scenario_tests::install_pick_frame`, plus the models of the
//! dynamic locs the app's frame builds) while the mouse is on the rock.
//!
//! Fixture: `fixtures/session-replay/skills/` (`record.sh`; the release
//! client with `CLIENT910_RECORD` against the dev lobby/world with the
//! TEST-ONLY `ALTO_SKILLS_ALWAYS=1`, so every success roll hits and nothing
//! burns).
use super::scenario_tests::{component_rect, install_pick_frame, menu, ui, writers};
use super::scenario_woodcutting::{
    all_components, chat, cs2_ints, if_opensub, loc_request, message_game, setvarc_large,
    update_inv_partial, update_stat, FrameReader,
};
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, ActorView, Arrival,
    Replay, Trace,
};
use super::*;
use rs910_symbols::{component, enums, interface, inv, loc, obj, seq, varc, varp};

const FIXTURE: &str = "fixtures/session-replay/skills";
/// `record.sh`: the mouse on the rock from HOVER_CYCLE, the left click on it
/// at ROCK_CYCLE and on the Make-X "Cook" button at COOK_CYCLE
/// (`CLIENT910_UI_HOVER` / `CLIENT910_UI_CLICKS`).
const HOVER_CYCLE: i32 = 400;
const ROCK_CYCLE: i32 = 450;
const COOK_CYCLE: i32 = 1450;
const ROCK_CLICK: [i32; 2] = [665, 579];
const COOK_CLICK: [i32; 2] = [622, 524];
/// `record.sh` sends the `makex 317` command at this cycle (one server
/// command per 100 cycles from cycle 101), and a server tick is 30 cycles.
const MAKEX_CYCLE: i32 = 1301;
const TICK_CYCLES: i32 = 30;

/// Cache facts (the server's own loc, enum, seq and stat decoders).
const COPPER_ROCK: i32 = loc::COPPER_ROCK.id(); // 2x2, ops [Mine, .., .., Prospect, .., Examine]
const ROCK_TILE: [i32; 3] = [3230, 3147, 0];
/// The saved character's tile (`record.sh`).
const START_TILE: [i32; 3] = [3229, 3150, 0];
const ROCK_SIZE: i32 = 2;
const FIRE: i32 = loc::FIRE.id(); // ops [.., Cook-at, .., .., .., Examine]
const FIRE_TILE: [i32; 3] = [3230, 3150, 0]; // east of the character (record.sh `locadd`)
const BRONZE_PICKAXE_SWING: i32 = seq::MINE_BRONZE_PICKAXE.id(); // offhand: the bronze pickaxe
/// The cooking seq at a fire (28 frames; the server's choice).
const COOK_SEQ: i32 = seq::COOK_ON_FIRE.id();
const RAW_SHRIMPS: i32 = obj::RAW_SHRIMPS.id();
const SHRIMPS: i32 = obj::SHRIMPS.id();
const COPPER_ORE: i32 = obj::COPPER_ORE.id();
const BACKPACK_INV: i32 = inv::BACKPACK.id();
const MINING: i32 = 14;
const COOKING: i32 = 7;
/// Window mounts and interfaces of the Make-X flow (cache interface ids).
const MAKE_X_MOUNT: i32 = component::game_window::CENTRAL_SLOT.packed();
const MAKE_X: i32 = interface::MAKE_X.id();
const MAKE_X_LIST: i32 = interface::MAKE_X_PRODUCTS.id();
const MAKE_X_LIST_MOUNT: i32 = component::make_x::PRODUCT_LIST_SLOT.packed();
const COOK_BUTTON: i32 = component::make_x::MAKE_BUTTON.packed();
const LEVEL_UP_MOUNT: i32 = component::game_window::LEVELUP_SLOT.packed();
const LEVEL_UP: i32 = interface::LEVELUP_POPUP.id();
/// The level-up banner graphic of Cooking (cache enum 745, index 7).
const COOKING_LEVEL_UP_GRAPHIC: i32 = 1482;
/// Server content texts (Mining.ts, Cooking.ts MESSAGES; unverified against 910).
const SWING: &str = "You swing your pickaxe at the rock.";
const MINED: &str = "You manage to mine some copper.";
const COOKED: &str = "You successfully cook the shrimps.";

/// `VARP_SMALL`: `(varp, value)` (p1 value, p2_alt2 varp).
fn varp_small(payload: &[u8]) -> [i32; 2] {
    let mut r = FrameReader(payload, 0);
    let value = i32::from(r.g1() as i8);
    [r.g2_alt2(), value]
}

/// `VARP_LARGE`: `(varp, value)` (p2_alt1 varp, p4 value).
fn varp_large(payload: &[u8]) -> [i32; 2] {
    let mut r = FrameReader(payload, 0);
    let varp = r.g1() | r.g1() << 8;
    [varp, r.g4s()]
}

/// `IF_SETEVENTS`: `(component, from, to, mask)` (p2_alt2 from, p4 mask,
/// p2_alt3 to, p4_alt1 component).
fn if_setevents(payload: &[u8]) -> [i32; 4] {
    let mut r = FrameReader(payload, 0);
    let from = r.g2_alt2();
    let mask = r.g4s();
    let to = (r.g1() - 128) & 0xFF | r.g1() << 8;
    [r.g4_alt1(), from, to, mask]
}

/// `IF_CLOSESUB`: the parent component (p4_alt2).
fn if_closesub(payload: &[u8]) -> i32 {
    let mut r = FrameReader(payload, 0);
    let b = [r.g1(), r.g1(), r.g1(), r.g1()];
    b[2] << 24 | b[3] << 16 | b[0] << 8 | b[1]
}

/// `LOC_ADD_CHANGE`: `(shape, angle, id, zone-local x, z)` (p1 shape<<2|angle,
/// p4_alt3 id, p1_alt2 x<<4|z).
fn loc_add_change(payload: &[u8]) -> [i32; 5] {
    let mut r = FrameReader(payload, 0);
    let packed = r.g1();
    let b = [r.g1(), r.g1(), r.g1(), r.g1()];
    let id = b[1] << 24 | b[0] << 16 | b[3] << 8 | b[2];
    let tile = r.g1_alt2();
    [packed >> 2, packed & 3, id, tile >> 4, tile & 15]
}

/// The client packet `RESUME_PAUSEBUTTON`'s bytes: p4_alt3 component,
/// p2_alt2 slot.
fn resume_pausebutton(component: i32, slot: i32) -> Vec<u8> {
    let mut out = vec![
        (component >> 16) as u8,
        (component >> 24) as u8,
        component as u8,
        (component >> 8) as u8,
    ];
    out.extend(writers::p2_alt2(slot));
    out
}

/// Located server frames of the recording, in the order the server sent them
/// (each search starts after the previous frame, so the order is part of the
/// finding).
struct Events {
    /// The loc the `locadd` server command put east of the character.
    fire: usize,
    swing: usize,
    /// `UPDATE_STAT` of Mining, one per swing.
    mining_stats: Vec<usize>,
    /// The ore and the message that goes with it, on the fifth swing.
    ore: usize,
    mined: usize,
    /// The Make-X open: varps, the two mounts, the pause button's events.
    varps: Vec<usize>,
    open: usize,
    open_list: usize,
    enable_button: usize,
    /// The server closes Make-X when the Cook button has been pressed.
    close: usize,
    /// Per cooked shrimp: the backpack slot, the stat and the message.
    cooked: Vec<[usize; 3]>,
    popup: usize,
    varc: usize,
}

/// Mining XP after each swing: the cache rock's per-swing XP is 23.76 tenths,
/// which the server rounds to 24 tenths a swing and carries below one point
/// (SkillXp.ts), so the whole XP after n swings is floor(2.4 n). A recording has
/// six or seven swings: the `makex` command that ends the mining is on the wall clock
/// and lands on the swing cadence's boundary (see `events`).
const MINING_XP: [i32; 7] = [2, 4, 7, 9, 12, 14, 16];
/// Cooking XP after each shrimp (30 XP, +10% at a fire) and its level (level 2
/// from 83 XP, the skill XP table).
const COOKING_XP: [(i32, i32); 4] = [(33, 1), (66, 1), (99, 2), (132, 2)];
/// A mining swing and a cook cycle are four server ticks apart.
const SWING_TICKS: usize = 4;

fn events(arrivals: &[Arrival]) -> anyhow::Result<Events> {
    use crate::proto::server as sp;
    // The first frame at or after `from` that `pred` accepts.
    let find = |what: &str, from: usize, pred: &dyn Fn(&Arrival) -> bool| {
        arrivals[from..]
            .iter()
            .position(pred)
            .map(|at| from + at)
            .with_context(|| format!("{what} not in the fixture after frame {from}"))
    };
    let stat = |stat: i32, xp: i32, level: i32| {
        move |a: &Arrival| {
            a.opcode == sp::UPDATE_STAT && update_stat(&a.payload) == [stat, xp, level]
        }
    };
    let text = |text: &'static str| {
        move |a: &Arrival| {
            a.opcode == sp::MESSAGE_GAME && message_game(&a.payload) == (0, text.to_string())
        }
    };
    let fire = find("fire", 0, &|a| a.opcode == sp::LOC_ADD_CHANGE)?;
    let swing = find("swing message", fire, &text(SWING))?;
    let mut at = swing;
    let mut mining_stats = Vec::new();
    for xp in MINING_XP[..4].iter().copied() {
        at = find("mining stat", at, &stat(MINING, xp, 1))?;
        mining_stats.push(at);
    }
    // The fifth swing: the ore in backpack slot 4 (slots 0..3 hold the
    // shrimps), its XP, then the message; the sixth swing is only XP.
    let ore = find("copper ore", at, &|a| {
        a.opcode == sp::UPDATE_INV_PARTIAL
            && update_inv_partial(&a.payload) == (BACKPACK_INV, vec![[4, COPPER_ORE, 1]])
    })?;
    let fifth = find("mining stat", ore, &stat(MINING, MINING_XP[4], 1))?;
    let mined = find("mined message", fifth, &text(MINED))?;
    let sixth = find("mining stat", mined, &stat(MINING, MINING_XP[5], 1))?;
    mining_stats.extend([fifth, sixth]);
    // The seventh swing is the one the `makex` command (every 100 cycles from
    // cycle 101, so on the wall clock, not the tick) may or may not interrupt:
    // the swings are four ticks apart and the command lands within a tick of the
    // seventh. Nothing but XP comes of it.
    if let Ok(seventh) = find("mining stat", sixth, &stat(MINING, MINING_XP[6], 1)) {
        mining_stats.push(seventh);
    }
    let at = *mining_stats.last().context("swings")?;
    let varps = {
        let mut from = at;
        let mut found = Vec::new();
        for (varp, value, large) in [
            (varp::MAKE_X_CATEGORY.id(), enums::COOKING_FISH.id(), true),
            (varp::MAKE_X_PRODUCT.id(), SHRIMPS, true),
            (varp::MAKE_X_MAXIMUM.id(), 4, false),
            (varp::MAKE_X_AMOUNT.id(), 4, false),
        ] {
            let opcode = if large {
                sp::VARP_LARGE
            } else {
                sp::VARP_SMALL
            };
            from = find("make-x varp", from, &|a| {
                a.opcode == opcode
                    && (if large {
                        varp_large(&a.payload)
                    } else {
                        varp_small(&a.payload)
                    }) == [varp, value]
            })?;
            found.push(from);
        }
        found
    };
    let open = find("make-x mount", varps[3], &|a| {
        a.opcode == sp::IF_OPENSUB && if_opensub(&a.payload) == [MAKE_X_MOUNT, MAKE_X, 0]
    })?;
    let open_list = find("make-x list mount", open, &|a| {
        a.opcode == sp::IF_OPENSUB && if_opensub(&a.payload) == [MAKE_X_LIST_MOUNT, MAKE_X_LIST, 1]
    })?;
    let enable_button = find("cook button events", open_list, &|a| {
        a.opcode == sp::IF_SETEVENTS && if_setevents(&a.payload) == [COOK_BUTTON, 65535, 65535, 1]
    })?;
    let close = find("make-x close", enable_button, &|a| {
        a.opcode == sp::IF_CLOSESUB && if_closesub(&a.payload) == MAKE_X_MOUNT
    })?;
    let mut from = close;
    let mut cooked = Vec::new();
    let (mut popup, mut varc) = (0, 0);
    for (slot, (xp, level)) in COOKING_XP.into_iter().enumerate() {
        let inv = find("shrimps in the backpack", from, &|a| {
            a.opcode == sp::UPDATE_INV_PARTIAL
                && update_inv_partial(&a.payload) == (BACKPACK_INV, vec![[slot as i32, SHRIMPS, 1]])
        })?;
        let cooking = find("cooking stat", inv, &stat(COOKING, xp, level))?;
        if level == 2 && popup == 0 {
            popup = find("level-up popup", cooking, &|a| {
                a.opcode == sp::IF_OPENSUB
                    && if_opensub(&a.payload) == [LEVEL_UP_MOUNT, LEVEL_UP, 1]
            })?;
            varc = find("level-up varc", popup, &|a| {
                a.opcode == sp::CLIENT_SETVARC_LARGE
                    && setvarc_large(&a.payload) == [varc::LEVELUP_POPUP.id(), COOKING | 2 << 23]
            })?;
        }
        let message = find("cooked message", cooking, &text(COOKED))?;
        cooked.push([inv, cooking, message]);
        from = message;
    }
    // The fire is `locadd 2732 10 0 1 0` east of the character: shape 10,
    // angle 0, in the zone the character stands in.
    assert_eq!(
        loc_add_change(&arrivals[fire].payload),
        [10, 0, FIRE, FIRE_TILE[0] & 7, FIRE_TILE[1] & 7],
        "the server's fire"
    );
    // The swing animation and the XP are not sent at the first tick: one
    // swing every SWING_TICKS ticks, the cooking cycle the same.
    let ticks = |a: usize, b: usize| {
        arrivals[a..b]
            .iter()
            .filter(|f| f.opcode == sp::PLAYER_INFO)
            .count()
    };
    for pair in mining_stats.windows(2) {
        assert_eq!(ticks(pair[0], pair[1]), SWING_TICKS, "ticks between swings");
    }
    for pair in cooked.windows(2) {
        assert_eq!(
            ticks(pair[0][1], pair[1][1]),
            SWING_TICKS,
            "ticks between cooks"
        );
    }
    // Nothing else of these happened in the session.
    let count = |pred: &dyn Fn(&Arrival) -> bool| arrivals.iter().filter(|a| pred(a)).count();
    assert_eq!(
        count(&|a| a.opcode == sp::UPDATE_STAT
            && update_stat(&a.payload)[0] == MINING
            && update_stat(&a.payload)[1] > 0),
        mining_stats.len(),
        "mining XP frames"
    );
    assert_eq!(count(&|a| text(SWING)(a)), 1, "swing messages");
    assert_eq!(count(&|a| text(MINED)(a)), 1, "ore messages");
    assert_eq!(
        count(&|a| text(COOKED)(a)),
        COOKING_XP.len(),
        "cooked messages"
    );
    assert_eq!(
        count(&|a| a.opcode == sp::UPDATE_INV_PARTIAL
            && update_inv_partial(&a.payload)
                .1
                .iter()
                .any(|s| s[1] == COPPER_ORE)),
        1,
        "ore frames"
    );
    let e = Events {
        fire,
        swing,
        mining_stats,
        ore,
        mined,
        varps,
        open,
        open_list,
        enable_button,
        close,
        cooked,
        popup,
        varc,
    };
    // `makex` runs after the mining: the Make-X open follows the last swing.
    assert!(
        e.fire < e.swing
            && e.mined < e.varps[0]
            && e.varps[3] < e.open
            && e.open < e.open_list
            && e.open_list < e.enable_button
            && e.enable_button < e.close
            && e.close < e.cooked[0][0]
            && e.cooked[3][2] > e.popup,
        "server event order"
    );
    Ok(e)
}

/// One recorded cycle through the production owners, with the renderer's pick
/// frame while the mouse is on the rock (ViewerApp's `refresh_picks` loc
/// re-test), checking the exact outgoing bytes.
fn replay_cycle(replay: &mut Replay, trace: &Trace, cycle: i32) -> anyhow::Result<Vec<u8>> {
    let out = if (HOVER_CYCLE..=ROCK_CYCLE + 1).contains(&cycle) {
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
        out?
    } else {
        replay.cycle(trace, cycle)?
    };
    assert_eq!(
        mask_wall_clock(&out.written)?,
        mask_wall_clock(&trace.bytes(b"OUT ", cycle))?,
        "cycle {cycle}: client packets differ from the recording"
    );
    Ok(out.written)
}

/// What the replay saw at the end of one cycle.
struct Tick {
    cycle: i32,
    /// How many server frames the client has processed.
    done: usize,
    local: ActorView,
}

/// The cycle's view for a probe: which located frames are processed, and
/// which of them in this very cycle.
struct Frame {
    cycle: i32,
    start: usize,
    done: usize,
}
impl Frame {
    fn has(&self, index: usize) -> bool {
        self.done > index
    }
    fn fresh(&self, index: usize) -> bool {
        (self.start..self.done).contains(&index)
    }
}

/// A replayed session.
struct Played {
    ticks: Vec<Tick>,
    /// Every loc option, examine and use-on-loc packet (client packets OPLOC1
    /// to OPLOC5, OPLOC6, OPLOCT) and RESUME_PAUSEBUTTON, with its cycle.
    sent: Vec<(i32, u8, Vec<u8>)>,
}

/// Every recorded cycle through [`replay_cycle`]; `probe` runs after each.
/// The common checks are the dev server's own position trace (one entry per
/// PLAYER_INFO) and the chat history (the MESSAGE_GAME frames processed so
/// far).
fn play(
    arrivals: &[Arrival],
    probe: &mut dyn FnMut(&mut Replay, &Frame) -> anyhow::Result<()>,
) -> anyhow::Result<Played> {
    use crate::proto::{client as cp, server as sp};
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let (server_players, _) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let mut replay = Replay::start(&trace)?;
    let (mut done, mut ticks, mut sent) = (0, Vec::new(), Vec::new());
    for cycle in 1..=trace.last_cycle() {
        let written = replay_cycle(&mut replay, &trace, cycle)?;
        sent.extend(
            client_frames(&written)?
                .into_iter()
                .filter(|(op, _)| {
                    [12u8, 96, 27, 61, 119, 39, 21, cp::RESUME_PAUSEBUTTON].contains(op)
                })
                .map(|(op, payload)| (cycle, op, payload)),
        );
        let start = done;
        done = replay.processed(arrivals);
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
        probe(&mut replay, &Frame { cycle, start, done })?;
        ticks.push(Tick { cycle, done, local });
    }
    Ok(Played { ticks, sent })
}

/// `(base level, visible XP, level)` of a skill read with the CS2 stat
/// commands.
fn stat_reads(replay: &mut Replay, stat: i32) -> anyhow::Result<Vec<i32>> {
    cs2_ints(
        replay,
        &[
            ("push_constant_int", stat),
            ("stat_base", 0),
            ("push_constant_int", stat),
            ("stat_visible_xp", 0),
            ("push_constant_int", stat),
            ("stat", 0),
        ],
    )
}

/// Backpack objects of slots 0..=4 and the count of slot 4, read with CS2
/// `inv_getobj` / `inv_getnum`.
fn backpack(replay: &mut Replay) -> anyhow::Result<Vec<i32>> {
    let mut code = Vec::new();
    for slot in 0..=4 {
        code.extend([
            ("push_constant_int", BACKPACK_INV),
            ("push_constant_int", slot),
            ("inv_getobj", 0),
        ]);
    }
    code.extend([
        ("push_constant_int", BACKPACK_INV),
        ("push_constant_int", 4),
        ("inv_getnum", 0),
    ]);
    cs2_ints(replay, &code)
}

/// Whether the backpack tab (1473:7) draws object `obj` in slot `slot`.
fn backpack_shows(replay: &mut Replay, slot: i32, obj: i32) -> bool {
    component_rect(ui(replay), &|c| {
        c.f.parentlayer == component::backpack::SLOTS.packed()
            && c.f.id == slot
            && c.f.invobject == obj
    })
    .is_some()
}

fn sub_id(replay: &mut Replay, mount: i32) -> Option<i32> {
    ui(replay).state.life.subs.get(mount).map(|s| s.borrow().id)
}

fn varp(replay: &mut Replay, id: u16) -> anyhow::Result<i32> {
    let game = replay
        .core
        .session
        .as_mut()
        .context("session")?
        .game
        .as_mut()
        .context("game")?;
    match crate::client_game::with_game(game, |v| {
        v.get(native910::vars::VarScope::Player, id, false)
    })? {
        native910::vm::Value::Int(n) => Ok(n),
        other => anyhow::bail!("varp {id}: {other:?}"),
    }
}

/// The rock's footprint: the reach tile is an orthogonal neighbour of it
/// (outside, sharing an edge).
fn edge_distance(tile: [i32; 3]) -> i32 {
    let far = ROCK_SIZE - 1;
    let dx = (ROCK_TILE[0] - tile[0])
        .max(tile[0] - (ROCK_TILE[0] + far))
        .max(0);
    let dz = (ROCK_TILE[1] - tile[1])
        .max(tile[1] - (ROCK_TILE[1] + far))
        .max(0);
    dx + dz
}

/// The facing angle (16384 to a turn) of an offset from the target to the
/// actor, as the server faces a character towards a loc's centre.
fn facing_angle(dx: f64, dz: f64) -> i32 {
    ((dx.atan2(dz) * 2_607.594_587_617_613_3) as i32) & 0x3FFF
}

fn fixture_arrivals() -> anyhow::Result<Vec<Arrival>> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    arrivals(&Trace::load(&root.join("session.rtr"))?)
}

/// Mining on the real client: the recorded session up to the Make-X open,
/// replayed through the production owners cycle by cycle with the exact
/// outgoing bytes of every cycle.
///
/// Expected values, none from the client under test:
/// - menu: the scene options (Face here / Walk here, the loc loop adding ops
///   last to first with white-tagged names) and the menu update (the actions
///   from 1000 up first, the tail is the left-click option) over cache loc 113148
///   "Copper rock" ops `[Mine, .., .., Prospect, .., Examine]` (op 0 is action
///   3, op 3 action 6, Examine 1002);
/// - OPLOC1 (client packet 12): `p1_alt2(ctrl 0) p2(z) p4(113148)
///   p2_alt3(x)` of the absolute rock tile, flushed by the next cycle's
///   socket write (the press is at ROCK_CYCLE), and no other loc packet;
/// - positions: the dev server's own trace (`server-trace.jsonl`,
///   ALTO_TRACE_INFO), one entry per PLAYER_INFO; the reach tile shares an
///   edge with the 2x2 rock;
/// - swing seq 32540 (cache seq whose offhand is obj 1265, the bronze
///   pickaxe) from the swing message and unbroken while the swings go on;
/// - chat: MESSAGE_GAME type 0 texts of the server content (Mining.ts);
/// - XP: UPDATE_STAT of Mining 2, 4, 7, 9, 12, 14 (, 16) on swings 1..6 (or 7) (24 tenths a
///   swing, carried), read with CS2 `stat_base`/`stat_visible_xp`/`stat`;
/// - copper ore 436 into backpack slot 4 (slots 0..3 hold the four raw
///   shrimps of the saved character) on the fifth swing, read with CS2
///   `inv_getobj`/`inv_getnum`.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_skills_session_mines_copper_swing_by_swing() -> anyhow::Result<()> {
    use crate::proto::client as cp;
    let arrivals = fixture_arrivals()?;
    let e = events(&arrivals)?;
    let mut menu_seen = false;
    let mut ore_cycle = 0;
    let played = play(&arrivals, &mut |replay, frame| {
        let cycle = frame.cycle;
        // The mouse on the rock: the menu, "Mine" on top.
        if cycle == ROCK_CYCLE - 1 {
            let rock = "<col=ffff>Copper rock".to_string();
            let entry =
                |op: &str, target: &str, action| (op.to_string(), target.to_string(), action);
            assert_eq!(ui(replay).engine.platform.mouse, ROCK_CLICK);
            assert_eq!(
                menu(replay),
                [
                    entry("Cancel", "", 1006),
                    entry("Examine", &rock, 1002),
                    entry("Face here", "", 60),
                    entry("Walk here", "", 23),
                    entry("Prospect", &rock, 6),
                    entry("Mine", &rock, 3),
                ],
                "minimenu with the mouse on the rock"
            );
            let base = {
                let g = replay.game();
                [g.runtime.map.base_x, g.runtime.map.base_z]
            };
            let m = &ui(replay).state.minimenu;
            let active = m.active.map(|a| m.entry(a).clone()).context("active")?;
            assert_eq!(
                (
                    active.op.as_str(),
                    (active.entity_id >> 32) as i32,
                    active.tile_x,
                    active.tile_z
                ),
                (
                    "Mine",
                    COPPER_ROCK,
                    ROCK_TILE[0] - base[0],
                    ROCK_TILE[1] - base[1]
                ),
                "the left-click option targets loc 113148 on the rock tile"
            );
            menu_seen = true;
        }
        // Backpack, stats (every 25 cycles and on the cycles their packets
        // are processed).
        let located = e
            .mining_stats
            .iter()
            .chain([&e.ore])
            .any(|&i| frame.fresh(i));
        // The backpack changes again once the shrimps are cooked.
        if (cycle % 25 != 0 && !located) || frame.has(e.varps[0]) {
            return Ok(());
        }
        let swings = e.mining_stats.iter().filter(|&&i| frame.has(i)).count();
        let xp = if swings == 0 {
            0
        } else {
            MINING_XP[swings - 1]
        };
        assert_eq!(
            stat_reads(replay, MINING)?,
            [1, xp, 1],
            "cycle {cycle}: stat_base/stat_visible_xp/stat of Mining"
        );
        let ore = if frame.has(e.ore) { COPPER_ORE } else { -1 };
        assert_eq!(
            backpack(replay)?,
            [
                RAW_SHRIMPS,
                RAW_SHRIMPS,
                RAW_SHRIMPS,
                RAW_SHRIMPS,
                ore,
                i32::from(ore >= 0)
            ],
            "cycle {cycle}: backpack"
        );
        // The backpack tab draws them once its hooks have run (on the UI
        // ticks after the packet).
        if frame.fresh(e.ore) {
            ore_cycle = cycle;
        }
        if cycle % 25 == 0 && frame.has(e.ore) && cycle >= ore_cycle + 2 * TICK_CYCLES {
            for (slot, obj) in [RAW_SHRIMPS, RAW_SHRIMPS, RAW_SHRIMPS, RAW_SHRIMPS, ore]
                .into_iter()
                .enumerate()
            {
                assert!(
                    backpack_shows(replay, slot as i32, obj),
                    "cycle {cycle}: the backpack tab shows {obj} in slot {slot}"
                );
            }
        }
        Ok(())
    })?;
    assert!(menu_seen);
    // OPLOC1, once, flushed the cycle after the press; the Make-X press is
    // the only other thing the client sent in this set.
    let mut expected = vec![writers::p1_alt2(0)];
    expected.extend(writers::p2(ROCK_TILE[1]));
    expected.extend(writers::p4(COPPER_ROCK));
    expected.extend(writers::p2_alt3(ROCK_TILE[0]));
    let loc_ops: Vec<_> = played
        .sent
        .iter()
        .filter(|(_, op, _)| *op != cp::RESUME_PAUSEBUTTON)
        .cloned()
        .collect();
    assert_eq!(
        loc_ops,
        [(ROCK_CYCLE + 1, cp::OPLOC1, expected)],
        "OPLOC1 on the socket"
    );
    // The walk to the reach tile, then the swings: the swing message arrives
    // with the character at the rock, the swing seq plays with it and stays
    // until the `makex` command stops the character.
    let at = |index: usize| {
        played
            .ticks
            .iter()
            .find(|t| t.done > index)
            .context("frame never processed")
    };
    let swing = at(e.swing)?;
    assert_eq!(
        edge_distance(swing.local.tile),
        1,
        "reach tile {:?}",
        swing.local.tile
    );
    assert_eq!(
        swing.local.main_anim, BRONZE_PICKAXE_SWING,
        "swing seq with the message"
    );
    let mut walked: Vec<[i32; 3]> = Vec::new();
    for t in played
        .ticks
        .iter()
        .filter(|t| (ROCK_CYCLE..=swing.cycle).contains(&t.cycle))
    {
        if walked.last() != Some(&t.local.tile) {
            walked.push(t.local.tile);
        }
    }
    assert_eq!(
        walked.first(),
        Some(&START_TILE),
        "the character starts where it was saved"
    );
    assert_eq!(walked.last(), Some(&swing.local.tile), "{walked:?}");
    assert!(
        walked.len() >= 2,
        "the character walked to the rock: {walked:?}"
    );
    let ended = played
        .ticks
        .iter()
        .find(|t| t.cycle > swing.cycle && t.local.main_anim != BRONZE_PICKAXE_SWING)
        .context("the swing seq never ended")?;
    assert_eq!(
        ended.local.main_anim, -1,
        "the seq is reset when the character is stopped"
    );
    assert!(
        ended.cycle > MAKEX_CYCLE
            && ended.cycle > at(*e.mining_stats.last().context("swings")?)?.cycle,
        "the swings go on until the makex command (sent at {MAKEX_CYCLE}), ended at {}",
        ended.cycle
    );
    // Facing the rock's centre two server ticks after the arrival, while swinging.
    let centre = |tile: i32, size: f64| f64::from(tile) + size / 2.;
    let facing = facing_angle(
        centre(swing.local.tile[0], 1.) - centre(ROCK_TILE[0], f64::from(ROCK_SIZE)),
        centre(swing.local.tile[1], 1.) - centre(ROCK_TILE[1], f64::from(ROCK_SIZE)),
    );
    for tick in played
        .ticks
        .iter()
        .filter(|t| (swing.cycle..ended.cycle).contains(&t.cycle))
    {
        assert_eq!(
            tick.local.tile, swing.local.tile,
            "cycle {}: stands at the rock",
            tick.cycle
        );
        assert_eq!(
            tick.local.main_anim, BRONZE_PICKAXE_SWING,
            "cycle {}: swing seq",
            tick.cycle
        );
        if tick.cycle >= swing.cycle + 2 * TICK_CYCLES {
            assert_eq!(
                tick.local.desired_angle & 0x3FFF,
                facing,
                "cycle {}: faces the rock",
                tick.cycle
            );
            assert_eq!(
                tick.local.angle & 0x3FFF,
                facing,
                "cycle {}: turned to the rock",
                tick.cycle
            );
        }
    }
    Ok(())
}

/// Make-X and cooking on the real client: the same recorded session, from the
/// `makex 317` server command (use the raw shrimps on the nearest fire) to the
/// fourth cooked shrimp, with the exact outgoing bytes of every cycle.
///
/// Expected values, none from the client under test:
/// - the server's open: varps 1169 = 6797 and 1170 = 315 (the product list's
///   layout), 8846 = 8847 = 4, then IF_OPENSUB 1370 into 1477:677 (type 0)
///   and 1371 into 1370:3 (type 1), and IF_SETEVENTS giving 1370:30 (the
///   "Cook" pause button) event mask 1; the retained UI mounts both, the
///   varps hold those values, the button's active mask has the pause-button
///   bit and the recorded click lies inside its rectangle;
/// - RESUME_PAUSEBUTTON (client packet 98): `p4_alt3(1370:30) p2_alt2(-1)`,
///   once, flushed the cycle after the press at COOK_CYCLE; the server answers
///   by closing 1477:677 (IF_CLOSESUB), and the client closes Make-X and its
///   product list with it;
/// - four cooks, four server ticks apart: raw shrimps 317 in backpack slots 0..3
///   become shrimps 315 one by one (the copper ore stays in slot 4), each with
///   the MESSAGE_GAME text and Cooking XP 33, 66, 99, 132 (30 XP + 10% at a
///   fire), read with CS2 `inv_getobj`/`inv_getnum` and `stat_base`/
///   `stat_visible_xp`/`stat`;
/// - level 2 at 83 XP (the skill XP table): IF_OPENSUB 1216 into 1477:34 with
///   varc 5188 = 7 | 2 << 23 (read back through the client varc domain) opens
///   the level-up banner, whose graphic cache script 336 (`levelup_start`)
///   sets from enum 745[7] = 1482;
/// - the fire the `locadd` command put at (3230, 3150) is in the client's
///   loc-change snapshot as loc 2732, shape 10.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_skills_session_cooks_four_shrimps_through_make_x() -> anyhow::Result<()> {
    use crate::proto::client as cp;
    let arrivals = fixture_arrivals()?;
    let e = events(&arrivals)?;
    let mut button_checked = false;
    let mut banner_checked = false;
    let mut last_change = 0;
    let played = play(&arrivals, &mut |replay, frame| {
        let cycle = frame.cycle;
        // Make-X is mounted from the open frames until the server's close.
        // The client drops the mount of 1370 and with it everything drawn
        // under it; the product list's own mount entry (1371 in 1370:3)
        // stays in the table, undrawn, as the original's table does.
        let open = frame.has(e.open) && !frame.has(e.close);
        assert_eq!(
            sub_id(replay, MAKE_X_MOUNT),
            open.then_some(MAKE_X),
            "cycle {cycle}: Make-X mount"
        );
        if frame.has(e.open_list) && !frame.has(e.close) {
            assert_eq!(
                sub_id(replay, MAKE_X_LIST_MOUNT),
                Some(MAKE_X_LIST),
                "cycle {cycle}: product list mount"
            );
        }
        if cycle % 25 == 0 || frame.fresh(e.close) {
            let drawn = |replay: &mut Replay, interface: i32| {
                component_rect(ui(replay), &|c| c.f.parentlayer >> 16 == interface).is_some()
            };
            if cycle > COOK_CYCLE {
                assert!(
                    open || (!drawn(replay, MAKE_X) && !drawn(replay, MAKE_X_LIST)),
                    "cycle {cycle}: nothing of Make-X is drawn after the close"
                );
            }
            if open && cycle > COOK_CYCLE - 100 && frame.has(e.enable_button) {
                assert!(
                    drawn(replay, MAKE_X) && drawn(replay, MAKE_X_LIST),
                    "cycle {cycle}: Make-X and its list are drawn"
                );
            }
        }
        // The press: the Cook button is enabled for the pause button, and the
        // recorded click lies inside it.
        if cycle == COOK_CYCLE && !button_checked {
            let rt = ui(replay);
            let button = all_components(rt)
                .into_iter()
                .find(|c| {
                    let c = c.borrow();
                    c.f.parentlayer == COOK_BUTTON && c.f.id == -1
                })
                .context("the Cook button component")?;
            assert!(
                crate::ui_minimenu::mask_pausebutton(rt.state.layout.active_mask(&button)),
                "the server's event mask 1 enables the pause button"
            );
            let [x, y, w, h] =
                component_rect(rt, &|c| c.f.parentlayer == COOK_BUTTON && c.f.id == -1)
                    .context("the Cook button is drawn")?;
            assert!(
                (x..x + w).contains(&COOK_CLICK[0]) && (y..y + h).contains(&COOK_CLICK[1]),
                "the click {COOK_CLICK:?} is inside the button {:?}",
                [x, y, w, h]
            );
            assert!(
                frame.has(e.enable_button) && !frame.has(e.close),
                "the button is pressed while Make-X is open"
            );
            button_checked = true;
        }
        let located = e
            .varps
            .iter()
            .chain([
                &e.fire,
                &e.open,
                &e.open_list,
                &e.enable_button,
                &e.close,
                &e.popup,
                &e.varc,
            ])
            .chain(e.cooked.iter().flatten())
            .any(|&i| frame.fresh(i));
        if cycle % 25 != 0 && !located {
            return Ok(());
        }
        // The fire in the loc-change snapshot.
        if frame.has(e.fire) {
            assert_eq!(
                loc_request(replay.game(), FIRE_TILE),
                Some((FIRE, 10, 0)),
                "cycle {cycle}: the fire"
            );
        }
        for (index, (varp_id, value)) in [
            (varp::MAKE_X_CATEGORY, enums::COOKING_FISH.id()),
            (varp::MAKE_X_PRODUCT, SHRIMPS),
            (varp::MAKE_X_MAXIMUM, 4),
            (varp::MAKE_X_AMOUNT, 4),
        ]
        .map(|(varp, value)| (varp.id() as u16, value))
        .into_iter()
        .enumerate()
        {
            if frame.has(e.varps[index]) {
                assert_eq!(
                    varp(replay, varp_id)?,
                    value,
                    "cycle {cycle}: varp {varp_id}"
                );
            }
        }
        // Backpack: the cooked slots, the ore in slot 4.
        let cooked = e.cooked.iter().filter(|c| frame.has(c[0])).count();
        let slot = |k: usize| if k < cooked { SHRIMPS } else { RAW_SHRIMPS };
        let ore = if frame.has(e.ore) {
            (COPPER_ORE, 1)
        } else {
            (-1, 0)
        };
        assert_eq!(
            backpack(replay)?,
            [slot(0), slot(1), slot(2), slot(3), ore.0, ore.1],
            "cycle {cycle}: backpack"
        );
        // Cooking XP and level, read after the stat frames.
        let stats = e.cooked.iter().filter(|c| frame.has(c[1])).count();
        let (xp, level) = if stats == 0 {
            (0, 1)
        } else {
            COOKING_XP[stats - 1]
        };
        assert_eq!(
            stat_reads(replay, COOKING)?,
            [level, xp, level],
            "cycle {cycle}: stat_base/stat_visible_xp/stat of Cooking"
        );
        // The backpack tab draws the same objects once its hooks have run
        // (on the UI ticks after the packet).
        if e.cooked.iter().any(|c| frame.fresh(c[0])) {
            last_change = cycle;
        }
        if cycle % 25 == 0 && cycle >= last_change + 60 {
            for (k, obj) in [slot(0), slot(1), slot(2), slot(3), ore.0]
                .into_iter()
                .enumerate()
            {
                assert!(
                    backpack_shows(replay, k as i32, obj),
                    "cycle {cycle}: the backpack tab shows {obj} in slot {k}"
                );
            }
        }
        // The level-up banner (the hooks run on the ticks after the packets).
        let popup = sub_id(replay, LEVEL_UP_MOUNT);
        if !frame.has(e.popup) {
            assert_eq!(popup, None, "cycle {cycle}: no banner before IF_OPENSUB");
            return Ok(());
        }
        if cycle % 25 != 0 {
            return Ok(());
        }
        assert_eq!(popup, Some(LEVEL_UP), "cycle {cycle}: 1216 in 1477:34");
        let game = replay
            .core
            .session
            .as_mut()
            .context("session")?
            .game
            .as_mut()
            .context("game")?;
        let varc = crate::client_game::with_game(game, |v| {
            v.get(
                native910::vars::VarScope::Client,
                varc::LEVELUP_POPUP.id() as u16,
                false,
            )
        })?;
        assert_eq!(
            varc,
            native910::vm::Value::Int(0),
            "cycle {cycle}: varc 5188 after levelup_start"
        );
        let graphic = ui(replay)
            .store
            .interfaces
            .get(&LEVEL_UP)
            .and_then(|i| i.borrow().components.borrow().get(17).cloned().flatten())
            .map(|c| c.borrow().f.graphic);
        assert_eq!(
            graphic,
            Some(COOKING_LEVEL_UP_GRAPHIC),
            "cycle {cycle}: script 336 set 1216:17"
        );
        banner_checked = true;
        Ok(())
    })?;
    assert!(button_checked && banner_checked);
    // RESUME_PAUSEBUTTON, once, flushed the cycle after the press.
    let presses: Vec<_> = played
        .sent
        .iter()
        .filter(|(_, op, _)| *op == cp::RESUME_PAUSEBUTTON)
        .cloned()
        .collect();
    assert_eq!(
        presses,
        [(
            COOK_CYCLE + 1,
            cp::RESUME_PAUSEBUTTON,
            resume_pausebutton(COOK_BUTTON, 65535)
        )],
        "RESUME_PAUSEBUTTON on the socket"
    );
    // The server closes Make-X after it has read the press, not before.
    let closed_at = played
        .ticks
        .iter()
        .find(|t| t.done > e.close)
        .map(|t| t.cycle)
        .context("close never processed")?;
    assert!(
        closed_at > COOK_CYCLE + 1,
        "closed at {closed_at}, pressed at {COOK_CYCLE}"
    );
    // The cooking seq plays once per cook: the first starts with the close
    // and each play ends as its shrimp reaches the backpack (within a tick).
    let mut plays: Vec<[i32; 2]> = Vec::new();
    let mut previous = -1;
    for t in &played.ticks {
        match (previous, t.local.main_anim) {
            (before, COOK_SEQ) if before != COOK_SEQ => plays.push([t.cycle, i32::MAX]),
            (COOK_SEQ, after) if after != COOK_SEQ => {
                assert_eq!(after, -1, "cycle {}: the cooking seq is reset", t.cycle);
                plays.last_mut().context("a play")?[1] = t.cycle;
            }
            _ => {}
        }
        previous = t.local.main_anim;
    }
    assert_eq!(
        plays.len(),
        e.cooked.len(),
        "one cooking play per shrimp: {plays:?}"
    );
    assert!(
        plays[0][0] >= closed_at,
        "the first play starts after the close: {plays:?}"
    );
    for (play, cooked) in plays.iter().zip(&e.cooked) {
        let landed = played
            .ticks
            .iter()
            .find(|t| t.done > cooked[0])
            .context("the cooked shrimp never landed")?
            .cycle;
        assert!(
            (play[1]..=play[1] + TICK_CYCLES).contains(&landed),
            "play {play:?} ends with the shrimp, landed at {landed}"
        );
    }
    Ok(())
}
