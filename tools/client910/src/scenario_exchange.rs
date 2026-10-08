//! The Grand Exchange, client side: one recorded dev-server session in which
//! the real client opens the exchange from a clerk (the hero window's Grand
//! Exchange tab), sells ten logs through the real window (slot 0's Sell, the
//! logs' Offer in the backpack beside it, Confirm Offer), watches the offer's
//! progress screen fill as the offer trades, and collects the coins; replayed
//! through the production owners (`session_replay`).
//!
//! Fixture: `fixtures/session-replay/exchange/` (`record.sh`; the release
//! client against the dev lobby/world with the DEV-ONLY
//! `ALTO_GE_MARKET_MAKER=3`, which buys three logs a sweep at the offer's
//! price). The window's buttons are UI operations; the clerk is added and its
//! Exchange option chosen by the dev server's `npcadd` and `opnpc` commands.
use super::scenario_tests::ui;
use super::scenario_woodcutting::{all_components, chat, cs2_ints, if_opensub, message_game};
use super::session_replay::{
    arrivals, mask_wall_clock, observe, server_trace_file, Arrival, Replay, Trace,
};
use super::*;
use rs910_symbols::{component, interface, inv, obj, ComponentId};

const FIXTURE: &str = "fixtures/session-replay/exchange";
const BACKPACK_INV: i32 = inv::BACKPACK.id();
/// The offer's status bits of the client's row (bits 0-2) and the sell bit.
const STATUS_BITS: u8 = 7;
const SELL_BIT: u8 = 8;
const RUNNING: u8 = 2;
const FINISHED: u8 = 5;

/// The client packets of a cycle without the round-trip reports and the idle
/// keepalives: a report is written every 30 s of wall clock (the recording ran
/// in real time, the replay does not) and it restarts the 50-cycle idle count
/// that `NO_TIMEOUT` follows.
fn without_reports(bytes: &[u8]) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    use crate::proto::client::{NO_TIMEOUT, PING_STATISTICS};
    Ok(mask_wall_clock(bytes)?
        .into_iter()
        .filter(|(op, _)| *op != PING_STATISTICS && *op != NO_TIMEOUT)
        .collect())
}

/// `UPDATE_STOCKMARKET_SLOT` as the server sent it: market, slot, state,
/// item, price, quantity, completed quantity, completed gold.
fn stock_row(payload: &[u8]) -> [i32; 8] {
    let int = |at: usize| {
        i32::from_be_bytes([
            payload[at],
            payload[at + 1],
            payload[at + 2],
            payload[at + 3],
        ])
    };
    [
        i32::from(payload[0]),
        i32::from(payload[1]),
        i32::from(payload[2]),
        i32::from(u16::from_be_bytes([payload[3], payload[4]])),
        int(5),
        int(9),
        int(13),
        int(17),
    ]
}

/// A number the way the window's script writes it (`tostring_localised`):
/// thousands separated by commas.
fn grouped(value: i32) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The text of static component `component`.
fn text_of(replay: &mut Replay, component: ComponentId) -> Option<String> {
    let packed = component.packed();
    all_components(ui(replay))
        .into_iter()
        .find(|c| c.borrow().f.parentlayer == packed && c.borrow().f.id == -1)
        .map(|c| String::from_utf16_lossy(c.borrow().f.text.as_deref().unwrap_or(&[])))
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
/// - positions: the dev server's own trace; chat: the MESSAGE_GAME texts;
/// - the exchange window: the hero window's first pane holds interface 105
///   once the clerk's Exchange is chosen;
/// - the offer: the client's row for market 0, slot 0 follows the server's
///   UPDATE_STOCKMARKET_SLOT rows (a running sell, its fills, finished, then
///   empty once collected), and the progress screen's text, drawn by the
///   window's own script when the row arrives (its stock transmit hook), says
///   each partial fill as the server sent it;
/// - every cycle that brings an offer row stamps the stock transmit cycle the
///   interfaces' `onstocktransmit` hooks wait for;
/// - the backpack at the end (CS2 `inv_getobj`/`inv_getnum`): the coins of
///   the whole sale in slot 0, the logs gone.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_exchange_session_sells_logs_through_the_window_and_collects_the_coins(
) -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let arrivals: Vec<Arrival> = arrivals(&trace)?;
    let (server_players, _) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let mut replay = Replay::start(&trace)?;
    let mut done = 0;
    let (mut rows, mut expected_rows, mut texts, mut mounted) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let pane = component::hero_window::BACKPACK_PANE.packed();
    for cycle in 1..=trace.last_cycle() {
        let stamp = ui(&mut replay).state.life.cycles.stock;
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

        // An offer row stamps the stock transmit cycle, so the windows' `onstocktransmit` hooks run.
        if arrivals[start..done]
            .iter()
            .any(|a| a.opcode == sp::UPDATE_STOCKMARKET_SLOT)
        {
            let now = ui(&mut replay).state.life.cycles.stock;
            assert!(
                now != 0 && now != stamp,
                "cycle {cycle}: the stock transmit cycle is stamped"
            );
        }
        let row = ui(&mut replay).engine.stockmarket_slots[0][0];
        track(
            &mut rows,
            [
                i32::from(row.state),
                row.object,
                row.price,
                row.count,
                row.completed_count,
                row.completed_gold,
            ],
        );
        let sent = arrivals[..done]
            .iter()
            .rev()
            .filter(|a| a.opcode == sp::UPDATE_STOCKMARKET_SLOT)
            .map(|a| stock_row(&a.payload))
            .find(|r| r[0] == 0 && r[1] == 0)
            .map_or([0; 6], |r| [r[2], r[3], r[4], r[5], r[6], r[7]]);
        track(&mut expected_rows, sent);
        if let Some(text) =
            text_of(&mut replay, component::exchange::PROGRESS_TEXT).filter(|t| !t.is_empty())
        {
            track(&mut texts, text);
        }
        let open = ui(&mut replay)
            .state
            .life
            .subs
            .get(pane)
            .map(|s| s.borrow().id)
            == Some(interface::EXCHANGE.id());
        track(&mut mounted, open);
    }

    // The window: mounted in the hero frame once, by the clerk's Exchange.
    assert_eq!(
        mounted,
        [false, true],
        "the exchange window in the hero frame"
    );
    let opened: Vec<[i32; 3]> = arrivals
        .iter()
        .filter(|a| a.opcode == sp::IF_OPENSUB)
        .map(|a| if_opensub(&a.payload))
        .filter(|[_, id, _]| *id == interface::EXCHANGE.id())
        .collect();
    assert_eq!(opened.len(), 1, "IF_OPENSUB of the exchange");

    // The offer row: each cycle the client holds the last row the server sent for slot 0.
    assert_eq!(rows, expected_rows, "the client's offer row for slot 0");
    let status = |r: &[i32; 6]| (r[0] as u8) & STATUS_BITS;
    let sell = |r: &[i32; 6]| (r[0] as u8) & SELL_BIT != 0;
    let fills: Vec<&[i32; 6]> = rows
        .iter()
        .filter(|r| status(r) == RUNNING && r[4] > 0)
        .collect();
    assert!(fills.len() >= 2, "the sale fills in parts: {rows:?}");
    assert!(
        rows.iter()
            .all(|r| r[1] == 0 || (r[1] == obj::LOGS.id() && sell(r) && r[3] == 10)),
        "a sell of ten logs"
    );
    let finished = rows
        .iter()
        .find(|r| status(r) == FINISHED)
        .context("the offer finishes")?;
    assert_eq!(finished[4], 10, "all ten sold");
    assert_eq!(
        *rows.last().unwrap(),
        [0; 6],
        "the slot is empty once collected"
    );

    // The progress screen redraws on each fill (the stock transmit hook): the
    // text of every partial fill the server sent.
    for r in &fills {
        let text = format!(
            "You have sold a total of <col=cc9900>{}</col> so far<br>for a total price of <col=cc9900>{}</col> gp.",
            grouped(r[4]),
            grouped(r[5])
        );
        assert!(texts.contains(&text), "progress text {text:?} in {texts:?}");
    }

    // The coins of the whole sale are in the backpack, the logs gone.
    let pack = cs2_ints(
        &mut replay,
        &[
            ("push_constant_int", BACKPACK_INV),
            ("push_constant_int", 0),
            ("inv_getobj", 0),
            ("push_constant_int", BACKPACK_INV),
            ("push_constant_int", 0),
            ("inv_getnum", 0),
            ("push_constant_int", BACKPACK_INV),
            ("push_constant_int", obj::LOGS.id()),
            ("inv_total", 0),
        ],
    )?;
    assert_eq!(
        pack,
        [obj::COINS.id(), finished[5], 0],
        "the backpack: the coins of the sale, no logs"
    );
    Ok(())
}
