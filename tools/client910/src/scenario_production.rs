//! Smelting, smithing, fletching and herblore, client side: one recorded
//! dev-server session in which the real client smelts two bronze bars at a
//! furnace and smiths a bronze full helm from them at an anvil (the smelting
//! and smithing window, interface 37), whittles and strings a shortbow
//! through the Make-X interface, and mixes an attack potion from its
//! backpack option, replayed through the production owners
//! (`session_replay`).
//!
//! Fixture: `fixtures/session-replay/production/` (`record.sh`; the release
//! client at 1500x950, because the smelting window is wider than the
//! 1024x768 layout holds, against the dev lobby/world with the TEST-ONLY
//! `ALTO_SKILLS_ALWAYS=1`). The furnace's Smelt and the anvil's Smith are chosen
//! by the dev server's `oploc` command (the headless replay cannot pick an animated
//! loc from the scene to reproduce a click; the click path itself is the mining
//! session's). The character is placed by a written save in the
//! Lumbridge smithy (furnace 113261 at (3226, 3256), forge 113259, anvil
//! 113258 at (3229, 3254), all of the cache map) at Smithing 6 by a server
//! command (`setxp 13 600`: bronze starts at full heat from level 6).
use super::scenario_tests::ui;
use super::scenario_woodcutting::{chat, cs2_ints, if_opensub, message_game, update_stat};
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Arrival, Replay, Trace,
};
use super::*;
use rs910_symbols::{component, interface, inv, obj};

const FIXTURE: &str = "fixtures/session-replay/production";
/// Cache facts (the server's own decoders): interface ids, the window
/// 1477 slots the interfaces mount into, objs, stats.
const METAL_WINDOW: i32 = interface::METAL_WINDOW.id();
/// Window 1047 "Central Interface (Large)": its content slot (enum 7716 -> struct 40393).
const LARGE_MOUNT: i32 = component::game_window::CENTRAL_LARGE_SLOT.packed();
const MAKE_X: i32 = interface::MAKE_X.id();
const MAKE_X_LIST: i32 = interface::MAKE_X_PRODUCTS.id();
const MAKE_X_MOUNT: i32 = component::game_window::CENTRAL_SLOT.packed();
const BACKPACK_INV: i32 = inv::BACKPACK.id();
const BRONZE_FULL_HELM: i32 = obj::BRONZE_FULL_HELM.id();
const SHORTBOW: i32 = obj::SHORTBOW.id();
const ATTACK_POTION: i32 = obj::ATTACK_POTION_3.id();
const SMITHING: i32 = 13;
const FLETCHING: i32 = 9;
const HERBLORE: i32 = 15;

/// Server content texts (Smithing, Production and Herblore modules; unverified against 910).
const TEXTS: [&str; 7] = [
    "You smelt the ores into a bronze bar.",
    "You smelt the ores into a bronze bar.",
    "You begin your project.",
    "You finish the bronze full helm.",
    "You make a shortbow (u).",
    "You make a shortbow.",
    "You make an attack potion (3).",
];

fn sub_id(replay: &mut Replay, mount: i32) -> Option<i32> {
    ui(replay).state.life.subs.get(mount).map(|s| s.borrow().id)
}

/// The client packets of a cycle without the round-trip reports, which the
/// recording machine's ping answered some cycles after the replay's fixed
/// answer (the session is longer than the 30 s the client waits between two).
fn without_reports(bytes: &[u8]) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    Ok(mask_wall_clock(bytes)?
        .into_iter()
        .filter(|(op, _)| *op != crate::proto::client::PING_STATISTICS)
        .collect())
}

/// One recorded cycle through the production owners, checking the exact
/// outgoing bytes.
fn replay_cycle(replay: &mut Replay, trace: &Trace, cycle: i32) -> anyhow::Result<Vec<u8>> {
    let out = replay.cycle(trace, cycle)?;
    assert_eq!(
        without_reports(&out.written)?,
        without_reports(&trace.bytes(b"OUT ", cycle))?,
        "cycle {cycle}: client packets differ from the recording"
    );
    Ok(out.written)
}

/// The stat updates of one stat in the recording, in order: `[xp, level]`.
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

/// Production on the real client: the recorded session replayed through the
/// production owners cycle by cycle with the exact outgoing bytes of every
/// cycle.
///
/// Expected values, none from the client under test:
/// - the client sends no loc packet (the `oploc` command starts both loc
///   options) and RESUME_PAUSEBUTTON once, the Fletch button of Make-X; the
///   byte-for-byte comparison with the recording covers every packet;
/// - the windows: interface 37 is mounted into 1477:668 (window 1047, "Central
///   Interface (Large)") twice, once for smelting and once for smithing, each
///   closed by the server before the next opens; Make-X (1370 in 1477:677,
///   its list 1371) opens once;
/// - positions: the dev server's own trace (`server-trace.jsonl`,
///   ALTO_TRACE_INFO), one entry per PLAYER_INFO;
/// - chat: MESSAGE_GAME texts of the server content, in order;
/// - XP, read from the UPDATE_STAT frames: Smithing 600 (the `setxp` command)
///   then 1 for each bar (the cache's 10 tenths, carried) and 30 for the helm
///   (cache param 2697 = 300 tenths) = 632; Fletching 5 for the unstrung bow
///   and 5 for stringing it = 10; Herblore 25 for the potion (250 tenths);
/// - the backpack at the end, read with CS2 `inv_getobj`: the bronze full helm
///   (1155) in slot 0, the shortbow (841) in slot 1, the attack potion (121)
///   in slot 2.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_production_session_smelts_smiths_fletches_and_mixes() -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let arrivals = arrivals(&trace)?;
    let (server_players, _) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let mut replay = Replay::start(&trace)?;
    let (mut done, mut sent) = (0, Vec::new());
    let (mut metal_seen, mut make_x_seen) = (0, 0);
    let mut metal_open = false;
    let mut make_x_open = false;
    for cycle in 1..=trace.last_cycle() {
        let written = replay_cycle(&mut replay, &trace, cycle)?;
        sent.extend(
            client_frames(&written)?
                .into_iter()
                .filter(|(op, _)| [12u8, cp::RESUME_PAUSEBUTTON].contains(op))
                .map(|(op, _)| op),
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
        // The mounts follow the server's open and close.
        let metal = sub_id(&mut replay, LARGE_MOUNT) == Some(METAL_WINDOW);
        let make_x = sub_id(&mut replay, MAKE_X_MOUNT) == Some(MAKE_X);
        metal_seen += i32::from(metal && !metal_open);
        make_x_seen += i32::from(make_x && !make_x_open);
        (metal_open, make_x_open) = (metal, make_x);
    }
    assert_eq!(sent.iter().filter(|&&op| op == 12).count(), 0, "OPLOC1");
    assert_eq!(
        sent.iter()
            .filter(|&&op| op == cp::RESUME_PAUSEBUTTON)
            .count(),
        1,
        "RESUME_PAUSEBUTTON"
    );
    assert_eq!((metal_seen, make_x_seen), (2, 1), "windows opened");
    assert!(
        !metal_open && !make_x_open,
        "every window closed at the end"
    );
    // Server frames: the mounts it opened, in order.
    let opened: Vec<[i32; 3]> = arrivals
        .iter()
        .filter(|a| a.opcode == sp::IF_OPENSUB)
        .map(|a| if_opensub(&a.payload))
        .filter(|[_, id, _]| [METAL_WINDOW, MAKE_X, MAKE_X_LIST].contains(id))
        .collect();
    assert_eq!(
        opened,
        [
            [LARGE_MOUNT, METAL_WINDOW, 0],
            [LARGE_MOUNT, METAL_WINDOW, 0],
            [MAKE_X_MOUNT, MAKE_X, 0],
            [
                component::make_x::PRODUCT_LIST_SLOT.packed(),
                MAKE_X_LIST,
                1,
            ],
        ]
    );
    let texts: Vec<String> = arrivals
        .iter()
        .filter(|a| a.opcode == sp::MESSAGE_GAME)
        .map(|a| message_game(&a.payload).1)
        .collect();
    assert_eq!(texts, TEXTS);
    assert_eq!(stat_updates(&arrivals, SMITHING).last(), Some(&[632, 6]));
    assert_eq!(stat_updates(&arrivals, FLETCHING).last(), Some(&[10, 1]));
    assert_eq!(stat_updates(&arrivals, HERBLORE).last(), Some(&[25, 1]));
    let mut code = Vec::new();
    for slot in 0..=2 {
        code.extend([
            ("push_constant_int", BACKPACK_INV),
            ("push_constant_int", slot),
            ("inv_getobj", 0),
        ]);
    }
    assert_eq!(
        cs2_ints(&mut replay, &code)?,
        [BRONZE_FULL_HELM, SHORTBOW, ATTACK_POTION],
        "backpack slots 0..2"
    );
    Ok(())
}
