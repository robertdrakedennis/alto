//! Treasure Trails on the real client: a dev-server session recorded with the release client
//! (`fixtures/session-replay/trails/record.sh`), replayed through the production owners (`session_replay`).
use super::scenario_tests::ui;
use super::session_replay::{observe, Replay, Trace};
use super::*;
use rs910_symbols::{component, interface, inv, npc, obj, seq, varp, InterfaceId, VarpId};

fn fixtures() -> std::path::PathBuf {
    rs910_core::test_support::client_fixtures().join("session-replay")
}

/// `record.sh`: the cycles of the client's own operations (each sent on the cycle after the one it was queued on).
const READ_COORDINATES: i32 = 301;
const DIG: i32 = 401;
const READ_EMOTE_CLUE: i32 = 701;
const PANIC: i32 = 801;
const OPEN_CASKET: i32 = 1201;
/// A cycle with each window up: the coordinates read, the emote clue read, the casket opened.
const COORDINATES_SHOWN: i32 = 380;
const EMOTE_CLUE_SHOWN: i32 = 740;
const REWARDS_SHOWN: i32 = 1350;
/// The backpack slots of the two clues (after the spade, sextant, watch and chart).
const MEDIUM_SLOT: i32 = 4;
const EASY_SLOT: i32 = 5;
/// The cache's emote list (enum 3874): its index 18 is Panic.
const PANIC_INDEX: i32 = 18;

fn mounted(replay: &mut Replay, id: InterfaceId) -> bool {
    ui(replay)
        .state
        .layout
        .subs
        .iter()
        .any(|&(_, sub)| sub == id.id())
}

/// Every text drawn under interface `id`, empty ones left out.
fn texts_under(replay: &mut Replay, id: InterfaceId) -> Vec<String> {
    super::scenario_woodcutting::all_components(ui(replay))
        .into_iter()
        .filter(|c| (c.borrow().f.parentlayer as u32 >> 16) as i32 == id.id())
        .filter_map(|c| c.borrow().f.text.as_deref().map(String::from_utf16_lossy))
        .filter(|t| !t.is_empty())
        .collect()
}

/// The objs of an inventory as the client holds it, by slot (-1: empty).
fn inventory(replay: &mut Replay, id: i32) -> Vec<(i32, i32)> {
    ui(replay)
        .engine
        .inv_cache
        .inventory(id, false)
        .map(|i| {
            i.obj_ids
                .iter()
                .copied()
                .zip(i.counts.iter().copied())
                .collect()
        })
        .unwrap_or_default()
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

/// A medium coordinate trail and an easy emote trail of one step each, from the real client's own input:
///
/// - the coordinate clue's Read (`IF_BUTTON1` on its backpack slot): the clue scroll window (345) shows the two bearings
///   the server worked out from the clue's dig tile; its Dig (`IF_BUTTON2`): the player digs (the spade's sequence) and
///   the medium reward casket takes the clue's slot;
/// - the emote clue's Read: its words on the scroll; the chat window's Emotes tab shows the emotes window (590), whose
///   list the client draws from the cache's emote list; the Panic emote (`IF_BUTTON1` on 590:8, the emote's index as
///   the slot): the player panics and Uri comes; his phrase in the NPC chat interface is answered with Continue and the
///   easy casket takes the clue's slot;
/// - the medium casket's Open: the reward window (364) draws the rewards from inventory 141, the casket leaves the
///   backpack and the medium trails completed (varp 7803) is 1.
///
/// Expected values, none from the client under test: the cache (the clue objs, interfaces, emote list, Uri, the
/// sequences), the dated wiki (the bearings' list, the clue's words, the reward counts) and `record.sh` (the cycles).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_trails_read_dig_coordinates_emote_for_uri_and_open_the_casket() -> anyhow::Result<()> {
    use super::session_replay::{client_frames, mask_wall_clock};
    use crate::proto::client as cp;
    let trace = Trace::load(&fixtures().join("trails/session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut sent: Vec<(i32, u8, Vec<u8>)> = Vec::new();
    let (mut coordinates, mut emote_clue, mut rewards) = (Vec::new(), Vec::new(), Vec::new());
    let mut anims: Vec<i32> = Vec::new();
    let mut uri_seen = false;
    let mut chatbox_lines: Vec<String> = Vec::new();
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
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
            if [cp::IF_BUTTON1, cp::IF_BUTTON2, cp::RESUME_PAUSEBUTTON].contains(&op) {
                sent.push((cycle, op, payload));
            }
        }
        let seen = observe(replay.game());
        if let Some(local) = seen.local {
            if anims.last() != Some(&local.main_anim) {
                anims.push(local.main_anim);
            }
        }
        uri_seen |= seen.npcs.values().any(|(t, _)| *t == npc::URI.id());
        if mounted(&mut replay, interface::NPC_CHAT) {
            for text in texts_under(&mut replay, interface::NPC_CHAT) {
                if !chatbox_lines.contains(&text) {
                    chatbox_lines.push(text);
                }
            }
        }
        match cycle {
            COORDINATES_SHOWN => coordinates = texts_under(&mut replay, interface::CLUE_SCROLL),
            EMOTE_CLUE_SHOWN => emote_clue = texts_under(&mut replay, interface::CLUE_SCROLL),
            REWARDS_SHOWN => {
                assert!(
                    mounted(&mut replay, interface::CLUE_REWARD_WINDOW),
                    "the reward window is up"
                );
                rewards = inventory(&mut replay, inv::CLUE_REWARD_ITEMS.id());
            }
            _ => {}
        }
    }

    // The client's own operations: the two Reads, the Dig, the Panic and the casket's Open, then the Continue.
    let buttons: Vec<(i32, u8, i32, i32)> = sent
        .iter()
        .filter(|(_, op, _)| *op != cp::RESUME_PAUSEBUTTON)
        .map(|(cycle, op, p)| {
            (
                *cycle,
                *op,
                i32::from_be_bytes([p[4], p[5], p[6], p[7]]),
                (i32::from(p[2]) << 8) | ((i32::from(p[3]) - 128) & 0xff),
            )
        })
        .collect();
    let backpack = component::backpack::SLOTS.packed();
    assert_eq!(
        buttons,
        [
            (READ_COORDINATES, cp::IF_BUTTON1, backpack, MEDIUM_SLOT),
            (DIG, cp::IF_BUTTON2, backpack, MEDIUM_SLOT),
            (READ_EMOTE_CLUE, cp::IF_BUTTON1, backpack, EASY_SLOT),
            (
                PANIC,
                cp::IF_BUTTON1,
                component::emotes::LIST.packed(),
                PANIC_INDEX
            ),
            (OPEN_CASKET, cp::IF_BUTTON1, backpack, MEDIUM_SLOT)
        ]
    );
    assert_eq!(
        sent.iter()
            .filter(|(_, op, _)| *op == cp::RESUME_PAUSEBUTTON)
            .count(),
        1
    );

    // The scroll: the coordinates as the dated wiki's list writes them, then the emote clue's words (wrapped).
    for line in ["01 degrees 26 minutes north", "08 degrees 01 minutes east"] {
        assert!(
            coordinates.iter().any(|t| t == line),
            "the scroll lacks {line:?}: {coordinates:?}"
        );
    }
    // The window's own title (the cache's interface) follows the server's lines.
    assert_eq!(
        emote_clue.last().map(String::as_str),
        Some("Mysterious Clue Scroll")
    );
    assert_eq!(
        emote_clue[..emote_clue.len() - 1].join(" "),
        "Panic on the pier where you catch the Fishing Trawler. Have nothing equipped at all when you do."
    );

    // The dig and the panic played; Uri came and spoke.
    assert!(
        anims.contains(&seq::DIG_WITH_SPADE.id()),
        "no dig: {anims:?}"
    );
    assert!(
        anims.contains(&seq::PANIC_EMOTE.id()),
        "no panic: {anims:?}"
    );
    assert!(uri_seen, "Uri never came");
    assert!(
        chatbox_lines.iter().any(|t| t == "Uri"),
        "Uri's name: {chatbox_lines:?}"
    );

    // The rewards: three to five (the dated wiki's medium count) in the reward window's inventory; the easy casket is
    // held, the medium one is gone, and the client's count of medium trails is 1.
    let given = rewards.iter().filter(|&&(id, _)| id >= 0).count();
    assert!((3..=5).contains(&given), "rewards: {rewards:?}");
    let bag = inventory(&mut replay, inv::BACKPACK.id());
    assert_eq!(bag[EASY_SLOT as usize].0, obj::REWARD_CASKET_EASY.id());
    assert!(bag
        .iter()
        .all(|&(id, _)| id != obj::REWARD_CASKET_MEDIUM.id()));
    assert_eq!(varp_value(&mut replay, varp::MEDIUM_TRAILS_COMPLETED)?, 1);
    Ok(())
}
