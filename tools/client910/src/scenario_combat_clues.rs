//! Combat clues through one ordinary client session: private foes, prayer,
//! re-digging, Uri, saved master selection and an ordinary medium key drop.
use super::scenario_woodcutting::message_game;
use super::session_replay::{arrivals, client_frames, mask_wall_clock, Replay, Trace};
use rs910_symbols::{component, inv, npc, obj, varbit};
use std::collections::BTreeSet;

const FIXTURE: &str = "fixtures/session-replay/combat-clues";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const ONE: i32 = 1;
const HARD_CASKETS: i32 = 3;
const WIZARD_PHASE_END: i32 = 2150;
const COMPONENT_OFFSET: usize = 4;
const COMPONENT_END: usize = 8;
const LOCKED_MESSAGE: &str = "This is locked. You need to find its key.";

fn held_count(replay: &Replay, item: i32) -> i32 {
    replay
        .ui()
        .engine
        .inv_cache
        .inventory(inv::BACKPACK.id(), false)
        .map(|inventory| {
            inventory
                .obj_ids
                .iter()
                .zip(&inventory.counts)
                .filter(|(id, _)| **id == item)
                .map(|(_, count)| *count)
                .sum()
        })
        .unwrap_or(ZERO)
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_combat_clues_fight_redig_claim_uri_and_unlock_medium_search() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut replay_pings = Vec::new();
    let mut recorded_pings = Vec::new();
    let mut seen = BTreeSet::new();
    let mut buttons = BTreeSet::new();
    let mut hard_defeated = false;
    let mut master_defeated = false;
    let mut protected = false;
    let mut projectile = false;
    let mut key_seen = false;
    let locked = arrivals(&trace)?.iter().any(|frame| {
        frame.opcode == crate::proto::server::MESSAGE_GAME
            && message_game(&frame.payload).1.ends_with(LOCKED_MESSAGE)
    });
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        // The headless shell omits redraw camera/collision work. Camera telemetry
        // is outside this combat proof; all other gameplay bytes match per cycle.
        // Worker ICMP reports retain stream-order equality.
        let gameplay =
            |bytes: &[u8], pings: &mut Vec<Vec<u8>>| -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
                Ok(mask_wall_clock(bytes)?
                    .into_iter()
                    .filter(|(opcode, payload)| {
                        if *opcode == crate::proto::client::PING_STATISTICS {
                            pings.push(payload.clone());
                            false
                        } else {
                            *opcode != crate::proto::client::NO_TIMEOUT
                                && *opcode != crate::proto::client::EVENT_CAMERA_POSITION
                        }
                    })
                    .collect())
            };
        assert_eq!(
            gameplay(&out.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: gameplay packets differ"
        );
        for (opcode, payload) in client_frames(&out.written)? {
            if [
                crate::proto::client::IF_BUTTON1,
                crate::proto::client::IF_BUTTON2,
            ]
            .contains(&opcode)
            {
                buttons.insert(i32::from_be_bytes(
                    payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?,
                ));
            }
        }
        let game = replay.game();
        let state = &game.runtime.feed.state;
        for enemy in state.npcs.entities.values() {
            seen.insert(enemy.type_id);
        }
        assert!(
            state
                .npcs
                .entities
                .values()
                .filter(|enemy| enemy.type_id == npc::CLUE_SARADOMIN_WIZARD.id())
                .count()
                <= ONE as usize,
            "duplicate private wizard"
        );
        if cycle < WIZARD_PHASE_END {
            projectile |= !state.zones.transients.projectiles.is_empty();
        }
        let bit = |id| {
            game.varbit_value(id)
                .map_err(|error| anyhow::anyhow!("{error:?}"))
        };
        hard_defeated |= bit(varbit::HARD_CLUE_FOE_DEFEATED.id() as u16)? != ZERO;
        master_defeated |= bit(varbit::MASTER_CLUE_FOE_DEFEATED.id() as u16)? != ZERO;
        protected |= bit(varbit::ACTIVE_PRAYER_MAGIC_GUARD.id() as u16)? != ZERO;
        key_seen |= held_count(&replay, obj::MEDIUM_CHICKEN_CLUE_KEY.id()) > ZERO;
    }
    assert_eq!(
        replay_pings, recorded_pings,
        "ICMP report bytes in stream order"
    );
    for required in [
        npc::CLUE_SARADOMIN_WIZARD,
        npc::CLUE_ZAMORAK_WIZARD,
        npc::CLUE_DOUBLE_AGENT_HARD,
        npc::CLUE_DOUBLE_AGENT_MASTER,
        npc::URI,
    ] {
        assert!(
            seen.contains(&required.id()),
            "clue NPC {required:?} never reached the client: {seen:?}"
        );
    }
    for required in [
        component::backpack::SLOTS,
        component::emotes::LIST,
        component::worn_equipment::SLOTS,
        component::prayer_book::PRAYER_BUTTONS,
    ] {
        assert!(
            buttons.contains(&required.packed()),
            "ordinary interface operation {required:?} missing"
        );
    }
    assert!(
        hard_defeated && master_defeated,
        "owned kill flags: hard={hard_defeated}, master={master_defeated}"
    );
    assert!(
        projectile && protected,
        "wizard projectile={projectile}, protect magic={protected}"
    );
    assert!(
        key_seen && locked,
        "ordinary key drop={key_seen}, locked search={locked}"
    );
    for (item, expected) in [
        (obj::REWARD_CASKET_HARD, HARD_CASKETS),
        (obj::REWARD_CASKET_MASTER, ONE),
        (obj::REWARD_CASKET_MEDIUM, ONE),
        (obj::MEDIUM_CHICKEN_CLUE_KEY, ZERO),
        (obj::MASTER_EMOTE_CLUE, ZERO),
        (obj::MEDIUM_CLUE_CHICKEN_KEY, ZERO),
    ] {
        assert_eq!(
            held_count(&replay, item.id()),
            expected,
            "final backpack count for {item:?}"
        );
    }
    Ok(())
}
