//! Ranged, magic, basic abilities, prayer protection and player death through
//! a recorded ordinary client session. The fixture server uses fixed rolls
//! and synthetic goblin ratings to isolate protection arithmetic.
use super::scenario_woodcutting::{message_game, update_stat};
use super::session_replay::{arrivals, client_frames, mask_wall_clock, Replay, Trace};
use super::*;
use rs910_symbols::{component, interface, inv, location, npc, obj, seq, varbit, varp};
use std::collections::BTreeSet;

const FIXTURE: &str = "fixtures/session-replay/combat";
const FIRST_CYCLE: i32 = 1;
const MAGIC_EQUIP_CYCLE: i32 = 950;
const MELEE_EQUIP_CYCLE: i32 = 1850;
const FATAL_COMMAND_CYCLE: i32 = 2701;
const INITIAL_XP: i32 = 40_000;
const RANGED_SKILL: i32 = 4;
const MAGIC_SKILL: i32 = 6;
const INITIAL_RESOURCES: i32 = 100;
const UNPROTECTED_DAMAGE: i32 = 200;
const PROTECTED_DAMAGE: i32 = 100;
const COMPONENT_OFFSET: usize = 4;
const COMPONENT_END: usize = 8;
const ZERO: i32 = 0;
const NO_GRAPHIC: i32 = -1;
const ACTION_BAR_READY_CYCLE: i32 = 340;

fn item_count(replay: &Replay, inventory: i32, item: i32) -> i32 {
    replay
        .ui()
        .engine
        .inv_cache
        .inventory(inventory, false)
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
fn recorded_combat_session_kills_with_range_and_magic_protects_and_respawns() -> anyhow::Result<()>
{
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut buttons = BTreeSet::new();
    let mut replay_pings = Vec::new();
    let mut recorded_pings = Vec::new();
    let mut ranged_death = false;
    let mut magic_death = false;
    let mut ranged_projectile = false;
    let mut magic_projectile = false;
    let mut ammo_spent = false;
    let mut rune_spent = false;
    let mut unprotected_hit = false;
    let mut protected_hit = false;
    let mut death_sequence = false;
    let mut empty_life = false;
    let mut adrenaline_gain = false;
    let mut grave_timer = false;
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        // ICMP measurement completion runs on a worker in the live session.
        // Compare its report bytes in stream order, while gameplay packets
        // retain exact per-cycle equality. Keepalives contain no gameplay
        // payload; their schedule shifts when the worker writes its report.
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
                        }
                    })
                    .collect())
            };
        assert_eq!(
            gameplay(&out.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: gameplay packets differ from the recording"
        );
        for (opcode, payload) in client_frames(&out.written)? {
            if [
                crate::proto::client::IF_BUTTON1,
                crate::proto::client::IF_BUTTON2,
            ]
            .contains(&opcode)
            {
                let bytes: [u8; COMPONENT_END - COMPONENT_OFFSET] =
                    payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?;
                buttons.insert(i32::from_be_bytes(bytes));
            }
        }
        if cycle == ACTION_BAR_READY_CYCLE {
            let action_bar = replay
                .ui()
                .store
                .interfaces
                .get(&interface::ACTION_BAR.id())
                .context("mounted action bar")?
                .borrow();
            for (slot, name, operation) in [
                (component::action_bar::SLICE_SLOT, "Slice", "Activate"),
                (
                    component::action_bar::PIERCING_SHOT_SLOT,
                    "Piercing Shot",
                    "Activate",
                ),
                (component::action_bar::WRACK_SLOT, "Wrack", "Cast"),
            ] {
                let reference = action_bar.get(slot.packed())?.context("ability slot")?;
                let ability = reference.borrow();
                assert_ne!(ability.f.graphic, NO_GRAPHIC, "{name} has no cache graphic");
                let names: Vec<_> = ability
                    .ops
                    .iter()
                    .flatten()
                    .flatten()
                    .map(|text| String::from_utf16_lossy(text))
                    .collect();
                assert!(
                    names.iter().any(|text| text == operation),
                    "{name} operation missing: {names:?}"
                );
                let label = ability
                    .f
                    .opbase
                    .as_ref()
                    .map(|text| String::from_utf16_lossy(text))
                    .unwrap_or_default();
                assert!(label.contains(name), "{name} label missing: {label}");
            }
        }
        let game = replay.game();
        let state = &game.runtime.feed.state;
        let local = state.players.players[game.runtime.map.local]
            .as_ref()
            .context("local player")?;
        for enemy in state
            .npcs
            .entities
            .values()
            .filter(|enemy| enemy.type_id == npc::CHICKEN.id())
        {
            if enemy.path.animation.main.id() == seq::CHICKEN_DEATH.id() {
                ranged_death |= cycle < MAGIC_EQUIP_CYCLE;
                magic_death |= (MAGIC_EQUIP_CYCLE..MELEE_EQUIP_CYCLE).contains(&cycle);
            }
        }
        let projectile = !state.zones.transients.projectiles.is_empty();
        ranged_projectile |= projectile && cycle < MAGIC_EQUIP_CYCLE;
        magic_projectile |= projectile && (MAGIC_EQUIP_CYCLE..MELEE_EQUIP_CYCLE).contains(&cycle);
        let protected = game
            .varbit_value(varbit::ACTIVE_PRAYER_MELEE_GUARD.id() as u16)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            != ZERO;
        if let Some(combat) = &local.combat {
            for hit in combat.hits.iter().filter(|hit| hit[4] != ZERO) {
                unprotected_hit |= !protected && hit[1] == UNPROTECTED_DAMAGE;
                protected_hit |= protected && hit[1] == PROTECTED_DAMAGE;
            }
        }
        death_sequence |= local.animation.main.id() == seq::PLAYER_DIES.id();
        empty_life |= game
            .varbit_value(varbit::CURRENT_LIFE_POINTS.id() as u16)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            == ZERO;
        adrenaline_gain |= state
            .varps
            .as_ref()
            .context("player variables")?
            .get(varp::ADRENALINE_FINE.id())
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            > ZERO;
        grave_timer |= replay
            .ui()
            .state
            .layout
            .subs
            .iter()
            .any(|(_, mounted)| *mounted == interface::GRAVE_TIMER.id());
        if cycle < FATAL_COMMAND_CYCLE {
            let arrows = item_count(&replay, inv::WORN_EQUIPMENT.id(), obj::BRONZE_ARROW.id());
            ammo_spent |= arrows > ZERO && arrows < INITIAL_RESOURCES;
            let runes = item_count(&replay, inv::BACKPACK.id(), obj::AIR_RUNE.id());
            rune_spent |= runes > ZERO && runes < INITIAL_RESOURCES;
        }
    }
    assert_eq!(
        replay_pings, recorded_pings,
        "ping report bytes in stream order"
    );
    assert!(
        ranged_death && magic_death,
        "ranged kill={ranged_death}, magic kill={magic_death}"
    );
    assert!(
        ranged_projectile && magic_projectile,
        "projectiles reached both client style paths"
    );
    assert!(
        ammo_spent && rune_spent,
        "ammo spent={ammo_spent}, runes spent={rune_spent}"
    );
    assert!(
        unprotected_hit && protected_hit,
        "NPC hits before={unprotected_hit}, protected={protected_hit}"
    );
    assert!(
        death_sequence && empty_life && grave_timer,
        "death sequence={death_sequence}, zero life={empty_life}, grave timer={grave_timer}"
    );
    assert!(adrenaline_gain, "the action bar received adrenaline");
    for button in [
        component::action_bar::SLICE_SLOT,
        component::action_bar::PIERCING_SHOT_SLOT,
        component::action_bar::WRACK_SLOT,
        component::prayer_book::PRAYER_BUTTONS,
    ] {
        assert!(
            buttons.contains(&button.packed()),
            "no ordinary UI packet from {button:?}"
        );
    }
    let updates: Vec<_> = arrivals(&trace)?
        .into_iter()
        .filter(|frame| frame.opcode == crate::proto::server::UPDATE_STAT)
        .map(|frame| update_stat(&frame.payload))
        .collect();
    for skill in [RANGED_SKILL, MAGIC_SKILL] {
        assert!(
            updates
                .iter()
                .any(|stat| stat[0] == skill && stat[1] > INITIAL_XP),
            "no kill XP for skill {skill}"
        );
    }
    let messages: Vec<_> = arrivals(&trace)?
        .into_iter()
        .filter(|frame| frame.opcode == crate::proto::server::MESSAGE_GAME)
        .map(|frame| message_game(&frame.payload).1)
        .collect();
    assert!(messages
        .iter()
        .any(|message| message == "Oh dear, you are dead!"));
    let game = replay.game();
    let local = game.runtime.feed.state.players.players[game.runtime.map.local]
        .as_ref()
        .context("respawned player")?;
    let destination = location::STANDARD_LUMBRIDGE_TELEPORT;
    assert_eq!(
        [
            game.runtime.map.base_x + local.x[0],
            game.runtime.map.base_z + local.z[0],
            local.level
        ],
        [destination.x(), destination.z(), destination.level()]
    );
    assert!(
        game.varbit_value(varbit::CURRENT_LIFE_POINTS.id() as u16)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            > ZERO,
        "respawn restored life"
    );
    Ok(())
}
