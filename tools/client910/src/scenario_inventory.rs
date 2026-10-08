//! Bank, shop and trade on the real client: dev-server sessions recorded with
//! the release client (`fixtures/session-replay/{bank,shop,trade}/record.sh`),
//! replayed through the production owners (`session_replay`), cycle by cycle.
//!
//! Every replayed cycle must write exactly the bytes the recording wrote (the
//! cache's own scripts decide what a click sends), and the client state after
//! the session is what the flow leaves behind: the inventories the server
//! transmitted, the varps its layout scripts read, and the mounted windows.
//! The trade fixture is one of two real clients; the partner's offer reaches
//! the recorded client only through the server's packets.
use super::scenario_tests::ui;
use super::session_replay::{client_frames, mask_wall_clock, Replay, Trace};
use super::*;
use rs910_symbols::{component, interface, inv, obj, varc, varp, InterfaceId, InvId, VarpId};

/// One decoded client button packet.
#[derive(Debug, PartialEq, Eq)]
enum Sent {
    /// `IF_BUTTONn`: option, component, child slot.
    Button(u8, i32, i32),
    /// `IF_BUTTOND`: source component and slot, target component and slot.
    Drag(i32, i32, i32, i32),
    /// `RESUME_P_COUNTDIALOG`: the typed amount.
    Count(i32),
}

fn p4_alt3(b: &[u8]) -> i32 {
    i32::from(b[1]) << 24 | i32::from(b[0]) << 16 | i32::from(b[3]) << 8 | i32::from(b[2])
}

fn p4(b: &[u8]) -> i32 {
    i32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn p2_alt2(b: &[u8]) -> i32 {
    i32::from(b[0]) << 8 | ((i32::from(b[1]) - 128) & 0xFF)
}

fn p2_alt1(b: &[u8]) -> i32 {
    i32::from(b[1]) << 8 | i32::from(b[0])
}

fn decode(op: u8, payload: &[u8]) -> Option<Sent> {
    use crate::proto::client as cp;
    let buttons = [
        cp::IF_BUTTON1,
        cp::IF_BUTTON2,
        cp::IF_BUTTON3,
        cp::IF_BUTTON4,
        cp::IF_BUTTON5,
        cp::IF_BUTTON6,
        cp::IF_BUTTON7,
        cp::IF_BUTTON8,
        cp::IF_BUTTON9,
        cp::IF_BUTTON10,
    ];
    if let Some(n) = buttons.iter().position(|&b| b == op) {
        // object p2_alt3, child p2_alt2, component p4.
        return Some(Sent::Button(
            n as u8 + 1,
            p4(&payload[4..8]),
            p2_alt2(&payload[2..4]),
        ));
    }
    if op == cp::IF_BUTTOND {
        // target slot p2, source component p4_alt3, target object p2_alt1,
        // source object p2_alt3, target component p4_alt3, source slot p2_alt1.
        return Some(Sent::Drag(
            p4_alt3(&payload[2..6]),
            p2_alt1(&payload[14..16]),
            p4_alt3(&payload[10..14]),
            i32::from(payload[0]) << 8 | i32::from(payload[1]),
        ));
    }
    if op == cp::RESUME_P_COUNTDIALOG {
        return Some(Sent::Count(p4(&payload[..4])));
    }
    None
}

/// A replayed session: the client at the end, and every button, drag and
/// count packet it wrote, with its cycle.
struct Session {
    replay: Replay,
    sent: Vec<(i32, Sent)>,
}

/// Replay `dir`'s recording. `probe` runs after each cycle.
fn replay_session(
    dir: &str,
    probe: &mut dyn FnMut(i32, &mut Replay) -> anyhow::Result<()>,
) -> anyhow::Result<Session> {
    let root = rs910_core::test_support::client_dir()
        .join("fixtures/session-replay")
        .join(dir);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut sent = Vec::new();
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            mask_wall_clock(&out.written)?,
            mask_wall_clock(&trace.bytes(b"OUT ", cycle))?,
            "cycle {cycle}: client packets differ from the recording"
        );
        for (op, payload) in client_frames(&out.written)? {
            if let Some(packet) = decode(op, &payload) {
                sent.push((cycle, packet));
            }
        }
        probe(cycle, &mut replay)?;
    }
    Ok(Session { replay, sent })
}

fn slots(replay: &mut Replay, inv: InvId, secondary: bool) -> Vec<(i32, i32)> {
    let inventory = ui(replay).engine.inv_cache.inventory(inv.id(), secondary);
    inventory
        .map(|i| {
            i.obj_ids
                .iter()
                .zip(&i.counts)
                .filter(|(&id, _)| id >= 0)
                .map(|(&id, &n)| (id, n))
                .collect()
        })
        .unwrap_or_default()
}

fn var(replay: &mut Replay, scope: native910::vars::VarScope, id: u16) -> anyhow::Result<i32> {
    let game = replay
        .core
        .session
        .as_mut()
        .context("session")?
        .game
        .as_mut()
        .context("game")?;
    match crate::client_game::with_game(game, |v| v.get(scope, id, false))? {
        native910::vm::Value::Int(n) => Ok(n),
        other => anyhow::bail!("variable {id}: {other:?}"),
    }
}

fn varp(replay: &mut Replay, id: VarpId) -> anyhow::Result<i32> {
    var(replay, native910::vars::VarScope::Player, id.id() as u16)
}

/// Whether interface `id` is mounted in a window slot right now.
fn mounted(replay: &mut Replay, id: InterfaceId) -> bool {
    ui(replay)
        .state
        .layout
        .subs
        .iter()
        .any(|&(_, sub)| sub == id.id())
}

fn stock(pairs: &[(i32, i32)]) -> Vec<(i32, i32)> {
    pairs.to_vec()
}

/// Bank (517): deposit, withdraw with every quantity form, the amount
/// prompt, note mode, placeholders and a tab made by dragging a row onto the
/// tab strip, from the cache scripts' own clicks.
///
/// Expected values are the flow's arithmetic on the recorded start (`invtest`
/// backpack, `bankfill` bank), read off the client's own inventory cache and
/// the varps its layout scripts read:
/// - deposit 1 whip, all sharks: bank whip 1 -> 2, sharks 25 -> 27;
/// - withdraw-5 runes (3000 -> 2995), withdraw-X 1000 coins typed at the
///   prompt (250000 -> 249000);
/// - note mode on: withdrawing all 50 logs and the hatchet gives their notes
///   (1512, 1352); placeholders on: the emptied hatchet row stays at count 0;
/// - the drag of row 1 (the whip) onto tab-strip slot 2 (the "new tab"
///   target) is IF_BUTTOND, tab 2 then holds one item (varp 4759 = 1) and
///   moves to the front.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_bank_session_deposits_withdraws_and_makes_a_tab() -> anyhow::Result<()> {
    let mut s = replay_session("bank", &mut |_, _| Ok(()))?;
    let backpack = component::bank_window::BACKPACK_SLOTS.packed();
    let rows = component::bank_window::SLOTS.packed();
    assert_eq!(
        s.sent,
        [
            (701, Sent::Button(1, backpack, 2)),
            (741, Sent::Button(7, backpack, 3)),
            (781, Sent::Button(3, rows, 3)),
            (821, Sent::Button(6, rows, 4)),
            (851, Sent::Count(1000)),
            (
                901,
                Sent::Button(1, component::bank_window::NOTE_BUTTON.packed(), 65535)
            ),
            (941, Sent::Button(7, rows, 6)),
            (
                981,
                Sent::Button(
                    1,
                    component::bank_window::PLACEHOLDER_BUTTON.packed(),
                    65535
                )
            ),
            (1021, Sent::Button(7, rows, 5)),
            (
                1151,
                Sent::Drag(rows, 1, component::bank_window::TAB_STRIP.packed(), 2)
            ),
        ]
    );
    assert_eq!(
        slots(&mut s.replay, inv::BANK_CONTENTS, false),
        stock(&[
            (obj::ABYSSAL_WHIP.id(), 2),
            (obj::BRONZE_DAGGER.id(), 1),
            (obj::SHARK.id(), 27),
            (obj::FIRE_RUNE.id(), 2995),
            (obj::COINS.id(), 249_000),
            (obj::BRONZE_HATCHET.id(), 0)
        ])
    );
    assert_eq!(
        slots(&mut s.replay, inv::BACKPACK, false),
        stock(&[
            (obj::COINS.id(), 101_001),
            (obj::BRONZE_DAGGER.id(), 1),
            (obj::LOGS_NOTED.id(), 50),
            (obj::BRONZE_HATCHET_NOTED.id(), 1),
            (obj::FIRE_RUNE.id(), 1505),
            (obj::ABYSSAL_WHIP_NOTED.id(), 20)
        ])
    );
    // Rows drawn (placeholder included), the note and placeholder settings
    // the client toggled and the server mirrored, and the new tab's size.
    assert_eq!(varp(&mut s.replay, varp::BANK_OCCUPIED_SLOTS)?, 6);
    assert_eq!(varp(&mut s.replay, varp::BANK_NOTE_MODE)?, 1);
    assert_eq!(
        varp(&mut s.replay, varp::BANK_DEFAULT_QUANTITY)? >> 4 & 1,
        1
    );
    assert_eq!(varp(&mut s.replay, varp::BANK_TAB_SIZES_2_3)?, 1);
    assert!(mounted(&mut s.replay, interface::BANK_WINDOW));
    Ok(())
}

/// Shop (1265): buy from the stock, switch to the sell tab, select a
/// backpack row, raise the amount with the panel's +5, sell it, and buy again
/// from the stock tab.
///
/// - buy-5 of stock row 0 (pots, 1 coin each, base stock 30): coins
///   100001 -> 99996, the pots fill five backpack slots, the stock line 25;
/// - Info on backpack row 5 (fire runes), +5 (amount 1 + 5 = 6), Sell: runes
///   1500 -> 1494 and coins +30 (5 each), the shop's rune line 0 -> 6;
/// - buy-1 of row 1 adds a jug for 1 coin (stock 10 -> 9): 99996 + 30 - 1 =
///   100025. The restock clock (every 100 ticks) did not fire in the window.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_shop_session_buys_sells_and_shows_the_stock() -> anyhow::Result<()> {
    let mut s = replay_session("shop", &mut |_, _| Ok(()))?;
    let rows = component::shop::STOCK_ROWS.packed();
    assert_eq!(
        s.sent,
        [
            (701, Sent::Button(3, rows, 0)),
            (
                761,
                Sent::Button(1, component::shop::SELL_TAB.packed(), 65535)
            ),
            (801, Sent::Button(1, rows, 5)),
            (
                841,
                Sent::Button(1, component::shop::AMOUNT_PLUS_FIVE.packed(), 65535)
            ),
            (
                881,
                Sent::Button(1, component::shop::CONFIRM_BUTTON.packed(), 65535)
            ),
            (
                921,
                Sent::Button(1, component::shop::BUY_TAB.packed(), 65535)
            ),
            (961, Sent::Button(2, rows, 1)),
        ]
    );
    let bag = slots(&mut s.replay, inv::BACKPACK, false);
    assert_eq!(bag[0], (obj::COINS.id(), 100_025));
    assert_eq!(
        bag.iter().filter(|s| s.0 == obj::EMPTY_POT.id()).count(),
        5,
        "five bought pots"
    );
    assert_eq!(
        bag.iter().filter(|s| s.0 == obj::JUG.id()).count(),
        1,
        "the jug"
    );
    assert_eq!(
        bag.iter().find(|s| s.0 == obj::FIRE_RUNE.id()),
        Some(&(obj::FIRE_RUNE.id(), 1494))
    );
    let shop = slots(&mut s.replay, inv::GENERAL_STORE_STOCK, false);
    assert_eq!(shop.first(), Some(&(obj::EMPTY_POT.id(), 25)));
    assert_eq!(
        shop.iter().find(|s| s.0 == obj::JUG.id()),
        Some(&(obj::JUG.id(), 9))
    );
    assert_eq!(
        shop.last(),
        Some(&(obj::FIRE_RUNE.id(), 6)),
        "the sold runes join the stock"
    );
    assert_eq!(
        varp(&mut s.replay, varp::SHOP_STOCK)?,
        inv::GENERAL_STORE_STOCK.id(),
        "the stock inventory the rows draw"
    );
    assert_eq!(
        varp(&mut s.replay, varp::SHOP_SELL_MODE)?,
        0,
        "back on the buy tab"
    );
    assert!(mounted(&mut s.replay, interface::SHOP));
    Ok(())
}

/// Trade with a second real client: offer from the side panel (Offer, then
/// Offer-5), the partner's offer arriving as the secondary offer inventory,
/// the value totals (client variables 729 and 697), accept, the confirmation
/// screen (334), confirm, and the swap.
///
/// - probe 900: my offer is the whip and both sharks (4151, 385, 385), the
///   partner's two sharks, totals 120601 and 600 (the cache costs: 120001 +
///   2 x 300; 2 x 300);
/// - probe 1350: after the first accept the confirmation screen replaced the
///   offer screen (334 mounted, 335 not);
/// - end: the windows are gone, the offer inventories stopped transmitting,
///   and the backpack lost the whip and got the partner's two sharks (the
///   two of mine went to the partner).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_trade_session_offers_confirms_and_swaps() -> anyhow::Result<()> {
    use native910::vars::VarScope::Client;
    let mut probes = Vec::new();
    let mut s = replay_session("trade", &mut |cycle, replay| {
        if cycle == 900 || cycle == 1350 {
            probes.push((
                slots(replay, inv::TRADE_OFFER, false),
                slots(replay, inv::TRADE_OFFER, true),
                var(replay, Client, varc::TRADE_MY_VALUE.id() as u16)?,
                var(replay, Client, varc::TRADE_THEIR_VALUE.id() as u16)?,
                mounted(replay, interface::TRADE_OFFER),
                mounted(replay, interface::TRADE_CONFIRM),
            ));
        }
        Ok(())
    })?;
    let side = component::trade_side::BACKPACK_GRID.packed();
    assert_eq!(
        s.sent,
        [
            (801, Sent::Button(1, side, 2)),
            (841, Sent::Button(2, side, 3)),
            (
                1101,
                Sent::Button(1, component::trade_offer::ACCEPT.packed(), 65535)
            ),
            (
                1401,
                Sent::Button(1, component::trade_confirm::ACCEPT.packed(), 65535)
            ),
        ]
    );
    let mine = stock(&[
        (obj::ABYSSAL_WHIP.id(), 1),
        (obj::SHARK.id(), 1),
        (obj::SHARK.id(), 1),
    ]);
    let theirs = stock(&[(obj::SHARK.id(), 1), (obj::SHARK.id(), 1)]);
    assert_eq!(
        probes[0],
        (mine.clone(), theirs.clone(), 120_601, 600, true, false)
    );
    assert_eq!(probes[1], (mine, theirs, 120_601, 600, false, true));
    assert_eq!(
        slots(&mut s.replay, inv::BACKPACK, false),
        stock(&[
            (obj::COINS.id(), 100_001),
            (obj::BRONZE_DAGGER.id(), 1),
            (obj::SHARK.id(), 1),
            (obj::SHARK.id(), 1),
            (obj::FIRE_RUNE.id(), 1500),
            (obj::ABYSSAL_WHIP_NOTED.id(), 20)
        ])
    );
    assert!(slots(&mut s.replay, inv::TRADE_OFFER, false).is_empty());
    assert!(slots(&mut s.replay, inv::TRADE_OFFER, true).is_empty());
    assert!(
        !mounted(&mut s.replay, interface::TRADE_OFFER)
            && !mounted(&mut s.replay, interface::TRADE_CONFIRM)
    );
    Ok(())
}
