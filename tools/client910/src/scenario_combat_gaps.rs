//! A developer injury after loading sets up a real fatal pickpocket; the player
//! respawns, enters the Abyss with and
//! without a worn bracelet, escapes three attacking monsters and fights a living rock striker.
//! UI operations are client input; developer interaction commands use the normal
//! content path. Socket E2E separately covers OPNPC and OPLOC input and saved skull risk.
use super::scenario_woodcutting::message_game;
use super::session_replay::{arrivals, mask_wall_clock, Replay, Trace};
use rs910_symbols::{location, npc, varbit};
use std::collections::BTreeSet;

const FIXTURE: &str = "fixtures/session-replay/combat-gaps";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const FIRST_SLOT: usize = 0;
const FATAL_PHASE_END: i32 = 700;
const SKULLED_ENTRY_CYCLE: i32 = 1800;
const ROCK_PHASE: i32 = 2800;
const MAX_LIFE: i32 = 9900;
const HEAD_BAR_END: usize = 2;

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_fatal_thieving_abyss_skull_inner_escape_and_living_rock_combat() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut seen = BTreeSet::new();
    let mut fatal_thieving = false;
    let mut respawn = false;
    let mut unskulled_entry = false;
    let mut skulled_entry = false;
    let mut inner_ring = false;
    let mut abyss_damage = false;
    let mut rock_damage = false;
    let mut previous_life = None;
    let mut rock_dead = false;
    let mut rock_gone = false;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        // Headless redraw omits camera collision; gameplay bytes remain equal.
        // Asynchronous ICMP completions retain exact bytes in stream order.
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
        let game = replay.game();
        let state = &game.runtime.feed.state;
        for enemy in state.npcs.entities.values() {
            seen.insert(enemy.type_id);
            if enemy.type_id == npc::LIVING_ROCK_STRIKER.id()
                && enemy.path.combat.as_ref().is_some_and(|combat| {
                    combat.bars.iter().any(|bar| {
                        bar.updates
                            .iter()
                            .any(|update| update[HEAD_BAR_END] == ZERO)
                    })
                })
            {
                rock_dead = true;
            }
        }
        if rock_dead
            && !state
                .npcs
                .entities
                .values()
                .any(|enemy| enemy.type_id == npc::LIVING_ROCK_STRIKER.id())
        {
            rock_gone = true;
        }
        let life = game
            .varbit_value(varbit::CURRENT_LIFE_POINTS.id() as u16)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        fatal_thieving |= cycle < FATAL_PHASE_END && life == ZERO;
        respawn |= fatal_thieving && cycle < FATAL_PHASE_END && life == MAX_LIFE;
        if previous_life.is_some_and(|previous| previous > life) {
            abyss_damage |= (SKULLED_ENTRY_CYCLE..ROCK_PHASE).contains(&cycle);
            rock_damage |= cycle >= ROCK_PHASE;
        }
        previous_life = Some(life);
        if let Some(me) = state
            .players
            .players
            .get(game.runtime.map.local)
            .and_then(Option::as_ref)
        {
            let skull = me.appearance.head_ids[FIRST_SLOT] >= ZERO
                && me.appearance.head_groups[FIRST_SLOT] >= ZERO;
            unskulled_entry |= cycle < SKULLED_ENTRY_CYCLE
                && game.runtime.map.base_z + me.z[FIRST_SLOT] == location::ABYSS_SLOT_1_OUTSIDE.z()
                && !skull;
            skulled_entry |= cycle >= SKULLED_ENTRY_CYCLE && skull;
            let inside = location::ABYSS_SLOT_1_INSIDE;
            inner_ring |= game.runtime.map.base_x + me.x[FIRST_SLOT] == inside.x()
                && game.runtime.map.base_z + me.z[FIRST_SLOT] == inside.z()
                && me.level == inside.level();
        }
    }
    assert_eq!(
        replay_pings, recorded_pings,
        "ICMP report bytes in stream order"
    );
    let messages: Vec<_> = arrivals(&trace)?
        .into_iter()
        .filter(|arrival| arrival.opcode == crate::proto::server::MESSAGE_GAME)
        .map(|arrival| message_game(&arrival.payload).1)
        .collect();
    assert!(messages
        .iter()
        .any(|message| message == "Oh dear, you are dead!"));
    assert!(messages
        .iter()
        .any(|message| message == "You make it through into the inner ring of the Abyss."));
    assert!(
        fatal_thieving && respawn,
        "fatal theft={fatal_thieving}, restored life={respawn}"
    );
    assert!(
        unskulled_entry && skulled_entry && inner_ring,
        "bracelet entry={unskulled_entry}, skull={skulled_entry}, inner={inner_ring}"
    );
    assert!(
        abyss_damage && rock_damage,
        "Abyss damage={abyss_damage}, living rock damage={rock_damage}"
    );
    assert!(
        rock_dead && rock_gone,
        "striker death={rock_dead}, removal={rock_gone}"
    );
    for required in [
        npc::MENAPHITE_THUG_PICKPOCKET,
        npc::MAGE_OF_ZAMORAK_WILDERNESS,
        npc::ABYSSAL_LEECH,
        npc::ABYSSAL_GUARDIAN,
        npc::ABYSSAL_WALKER,
        npc::LIVING_ROCK_PROTECTOR,
        npc::LIVING_ROCK_STRIKER,
    ] {
        assert!(
            seen.contains(&required.id()),
            "NPC {required:?} absent: {seen:?}"
        );
    }
    Ok(())
}
