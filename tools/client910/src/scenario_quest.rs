//! Quests on the real client: a dev-server session recorded with the release client
//! (`fixtures/session-replay/quest/record.sh`), replayed through the production owners (`session_replay`).
use super::scenario_tests::ui;
use super::session_replay::{Replay, Trace};
use super::*;
use rs910_symbols::{component, interface, inv, obj, varp, ComponentId, InterfaceId, VarpId};

fn fixtures() -> std::path::PathBuf {
    rs910_core::test_support::client_fixtures().join("session-replay")
}

/// `fixtures/session-replay/quest/record.sh`: the cycles of the client's operations.
const ACCEPT_CYCLE: i32 = 1251;
const SCROLL_CONTINUE_CYCLE: i32 = 3651;
const JOURNAL_CYCLE: i32 = 3751;
/// A cycle of each stage: the offer is up and not answered; the quest started, between the two conversations; the
/// scroll is up; the journal is up.
const OFFERED_CYCLE: i32 = 1240;
const STARTED_CYCLE: i32 = 1900;
const SCROLL_CYCLE: i32 = 3600;
const JOURNAL_SHOWN_CYCLE: i32 = 3850;
/// Cook's Assistant's row of the quest list: its index in the cache's quest list enum (2252), which the list's script
/// uses as the row's dynamic child.
const COOKS_ASSISTANT_ROW: i32 = 1;
/// The points every quest of the cache's quest list offers together.
const AVAILABLE_POINTS: i32 = 408;

fn mounted(replay: &mut Replay, id: InterfaceId) -> bool {
    ui(replay)
        .state
        .layout
        .subs
        .iter()
        .any(|&(_, sub)| sub == id.id())
}

/// The component `child` under `layer` (a dynamic child, or the static component itself for -1).
fn child_of(
    replay: &mut Replay,
    layer: ComponentId,
    child: i32,
) -> Option<crate::ui_components::Ref> {
    super::scenario_woodcutting::all_components(ui(replay))
        .into_iter()
        .find(|c| c.borrow().f.parentlayer == layer.packed() && c.borrow().f.id == child)
}

fn text_of(replay: &mut Replay, component: ComponentId) -> Option<String> {
    child_of(replay, component, -1)
        .map(|c| String::from_utf16_lossy(c.borrow().f.text.as_deref().unwrap_or(&[])))
}

/// The client's own reading of the quest's state: `quest_started` and `quest_finished` of Cook's Assistant, the
/// commands the quest list's status script (3982) runs on the quest record and the player's variables.
fn quest_state(replay: &mut Replay) -> anyhow::Result<(i32, i32)> {
    let quest = rs910_symbols::quest::COOKS_ASSISTANT_QUEST.id();
    let ints = super::scenario_woodcutting::cs2_ints(
        replay,
        &[
            ("push_constant_int", quest),
            ("quest_started", 0),
            ("push_constant_int", quest),
            ("quest_finished", 0),
        ],
    )?;

    Ok((ints[0], ints[1]))
}

/// Every text drawn under interface `id`.
fn texts_under(replay: &mut Replay, id: InterfaceId) -> Vec<String> {
    super::scenario_woodcutting::all_components(ui(replay))
        .into_iter()
        .filter(|c| (c.borrow().f.parentlayer as u32 >> 16) as i32 == id.id())
        .filter_map(|c| c.borrow().f.text.as_deref().map(String::from_utf16_lossy))
        .filter(|t| !t.is_empty())
        .collect()
}

fn varp_value(replay: &mut Replay, id: VarpId) -> anyhow::Result<i32> {
    let game = replay
        .core
        .session
        .as_mut()
        .context("session")?
        .game
        .as_mut()
        .context("game")?;
    match crate::client_game::with_game(game, |v| {
        v.get(native910::vars::VarScope::Player, id.id() as u16, false)
    })? {
        native910::vm::Value::Int(n) => Ok(n),
        other => anyhow::bail!("variable {id:?}: {other:?}"),
    }
}

fn backpack(replay: &mut Replay) -> Vec<(i32, i32)> {
    ui(replay)
        .engine
        .inv_cache
        .inventory(inv::BACKPACK.id(), false)
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

/// Cook's Assistant from the offer to the scroll (the whole session of `quest`), from the real client's own clicks:
///
/// - the cook's conversation (NPC 278 beside the player): his words in the NPC chat interface (1184), the options
///   (1188) and the first one, the player's words (1191), each answered with Continue (`RESUME_PAUSEBUTTON`);
/// - the quest noticeboard: the journal window (1500) on the quest's overview, and the client's Accept Quest
///   (`IF_BUTTON1` on 1500:404); the quest list's row of Cook's Assistant (drawn by the cache's script from the
///   quest record and the progress varp) goes from red to yellow as the progress varp goes from 0 to 1;
/// - the hand-in: the cook's question, the three ingredients given (narrated in the message box, 1186, and said by
///   the player), his thanks; the progress varp becomes 2, the row turns green and the list's points line reads one
///   point of the 408 the list offers;
/// - the complete scroll (1244): the quest's name, "1 Quest Point" and the record's reward text; the rewards in the
///   backpack (500 coins, 20 sardines); the scroll's Continue closes it;
/// - the journal opened from the quest list's row: the quest's name and its lines, struck through.
///
/// Expected values, none from the client under test: the cache (the quest record, the list's script colours, the
/// interfaces), the dated wiki (the transcript's words, the rewards) and `record.sh` (the cycles of the clicks).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_cooks_assistant_offer_hand_in_scroll_and_journal() -> anyhow::Result<()> {
    use super::session_replay::{client_frames, mask_wall_clock};
    use crate::proto::client as cp;
    let trace = Trace::load(&fixtures().join("quest/session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let watched = [
        interface::NPC_CHAT,
        interface::DIALOGUE_OPTIONS,
        interface::PLAYER_CHAT,
        interface::MESSAGE_BOX,
        interface::QUEST_JOURNAL_WINDOW,
        interface::QUEST_COMPLETE_SCROLL,
    ];
    let mut windows: Vec<Vec<i32>> = Vec::new();
    let mut sent: Vec<(i32, u8, Vec<u8>)> = Vec::new();
    let mut greeting = None;
    let mut progress = Vec::new();
    let mut scroll = Vec::new();
    let mut journal = Vec::new();
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        // The statistics report goes by the machine's clock (the recording ran slower while it took screenshots).
        let answers = |frames: Vec<(u8, Vec<u8>)>| -> Vec<(u8, Vec<u8>)> {
            frames
                .into_iter()
                .filter(|(op, _)| *op != cp::PING_STATISTICS)
                .collect()
        };
        assert_eq!(
            answers(mask_wall_clock(&out.written)?),
            answers(mask_wall_clock(&trace.bytes(b"OUT ", cycle))?),
            "cycle {cycle}: client packets differ from the recording"
        );
        for (op, payload) in client_frames(&out.written)? {
            if [cp::RESUME_PAUSEBUTTON, cp::IF_BUTTON1].contains(&op) {
                sent.push((cycle, op, payload));
            }
        }
        let now: Vec<i32> = watched
            .map(|i| i.id())
            .into_iter()
            .filter(|&i| mounted(&mut replay, InterfaceId(i)))
            .collect();
        if windows.last() != Some(&now) {
            windows.push(now);
        }
        if greeting.is_none() && mounted(&mut replay, interface::NPC_CHAT) {
            greeting = text_of(&mut replay, component::npc_chat::TEXT);
        }
        if [OFFERED_CYCLE, STARTED_CYCLE, SCROLL_CYCLE].contains(&cycle) {
            progress.push((
                varp_value(&mut replay, varp::COOKS_ASSISTANT_PROGRESS)?,
                quest_state(&mut replay)?,
                varp_value(&mut replay, varp::QUEST_POINTS)?,
                varp_value(&mut replay, varp::QUEST_POINTS_AVAILABLE)?,
            ));
        }
        if cycle == SCROLL_CYCLE {
            scroll = texts_under(&mut replay, interface::QUEST_COMPLETE_SCROLL);
        }
        if cycle == JOURNAL_SHOWN_CYCLE {
            journal = texts_under(&mut replay, interface::QUEST_JOURNAL_WINDOW);
        }
    }
    let (npc, options, player, plain) = (
        interface::NPC_CHAT.id(),
        interface::DIALOGUE_OPTIONS.id(),
        interface::PLAYER_CHAT.id(),
        interface::MESSAGE_BOX.id(),
    );
    let (board, done) = (
        interface::QUEST_JOURNAL_WINDOW.id(),
        interface::QUEST_COMPLETE_SCROLL.id(),
    );
    // The windows, in order. The offer: the cook, the options, the player's choice, the cook's long plea, the
    // noticeboard, then his thanks and the player's question. The hand-in: his question, each ingredient narrated and
    // named by the player, his thanks, the scroll; then the journal from the quest list.
    assert_eq!(
        windows,
        [
            vec![],
            vec![npc],
            vec![options],
            vec![player],
            vec![npc],
            vec![board],
            vec![npc],
            vec![player],
            vec![npc],
            vec![],
            vec![npc],
            vec![plain],
            vec![player],
            vec![plain],
            vec![player],
            vec![plain],
            vec![player],
            vec![npc],
            vec![player],
            vec![npc],
            vec![player],
            vec![npc],
            vec![done],
            vec![],
            vec![board]
        ]
    );
    assert_eq!(greeting.as_deref(), Some("What am I to do?"));
    // Every screen was answered by the client's own Continue (or the first option), 24 in all.
    let pauses: Vec<i32> = sent
        .iter()
        .filter(|(_, op, _)| *op == cp::RESUME_PAUSEBUTTON)
        .map(|(_, _, p)| i32::from_be_bytes([p[1], p[0], p[3], p[2]]))
        .collect();
    assert_eq!(pauses.len(), 24);
    assert_eq!(
        pauses[1],
        component::dialogue_options::FIRST_OPTION.packed()
    );
    assert!(pauses.contains(&component::message_box::CONTINUE_BUTTON.packed()));
    // The client's operations: Accept Quest, the scroll's Continue, the journal option of Cook's Assistant's row.
    let buttons: Vec<(i32, i32, i32)> = sent
        .iter()
        .filter(|(_, op, _)| *op == cp::IF_BUTTON1)
        .map(|(cycle, _, p)| {
            (
                *cycle,
                i32::from_be_bytes([p[4], p[5], p[6], p[7]]),
                (i32::from(p[2]) << 8) | ((i32::from(p[3]) - 128) & 0xff),
            )
        })
        .collect();
    assert_eq!(
        buttons
            .iter()
            .map(|&(cycle, component, _)| (cycle, component))
            .collect::<Vec<_>>(),
        [
            (
                ACCEPT_CYCLE,
                component::quest_journal_window::ACCEPT_BUTTON.packed()
            ),
            (
                SCROLL_CONTINUE_CYCLE,
                component::quest_complete_scroll::CONTINUE_BUTTON.packed()
            ),
            (JOURNAL_CYCLE, component::quest_list_window::ROWS.packed())
        ]
    );
    assert_eq!(buttons[2].2, COOKS_ASSISTANT_ROW);
    // The progress varp, the client's own reading of the quest record (quest_started, quest_finished) and the points
    // line's two varps: offered, started, complete.
    assert_eq!(
        progress,
        [
            (0, (0, 0), 0, AVAILABLE_POINTS),
            (1, (1, 0), 0, AVAILABLE_POINTS),
            (2, (1, 1), 1, AVAILABLE_POINTS)
        ]
    );
    // The scroll: the quest, the point and the record's reward text.
    for text in [
        "Cook's Assistant",
        "1 Quest Point",
        "300 Cooking XP<br>500 coins<br>20 sardines<br>Access to the cook's range",
    ] {
        assert!(
            scroll.iter().any(|t| t == text),
            "the scroll lacks {text:?}: {scroll:?}"
        );
    }
    // The journal: its lines struck through, and the quest complete.
    for text in [
        "<str>Talk to the cook in the kitchen of Lumbridge Castle.</str>",
        "<str>Some top-quality milk.</str>",
        "<col=ff0000>QUEST COMPLETE!</col>",
    ] {
        assert!(
            journal.iter().any(|t| t == text),
            "the journal lacks {text:?}: {journal:?}"
        );
    }
    // The rewards in the backpack: 500 coins and 20 sardines (one a slot).
    let bag = backpack(&mut replay);
    assert_eq!(
        bag.iter()
            .filter(|&&(id, _)| id == obj::COINS.id())
            .map(|&(_, n)| n)
            .sum::<i32>(),
        500
    );
    assert_eq!(
        bag.iter()
            .filter(|&&(id, _)| id == obj::SARDINE.id())
            .count(),
        20
    );
    Ok(())
}
