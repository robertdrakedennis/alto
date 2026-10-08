//! An ordinary Legacy Mole instance: full-health fight, chamber retreats,
//! native actor/effect/death packets, ground loot, exit and re-entry.
//! Initial skills and random draws are fixture inputs. Recorded selection
//! commands delegate normal owners; the socket cases cover native walk/picks.
use super::scenario_tests::ui;
use super::scenario_woodcutting::{all_components, if_opensub};
use super::session_replay::{arrivals, client_frames, mask_wall_clock, Replay, Trace};
use anyhow::Context;
use rs910_symbols::{component, interface, inv, npc, obj, seq, spot, varbit};
use serde_json::Value;
use std::collections::BTreeSet;

const FIXTURE: &str = "fixtures/session-replay/giant-mole";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const FIRST_PLAYER: usize = 0;
const PATH_HEAD: usize = 0;
const HIT_DAMAGE: usize = 1;
const HEALTH_BAR_END: usize = 2;
const STATIC_COMPONENT_CHILD: i32 = -1;
const FULL_MOLE_LIFE: i64 = 78_000;
const MAXIMUM_PLAYER_LIFE: i32 = 9900;
const CORPSE_TICKS: i64 = 3;
const REQUIRED_CHAMBERS_AND_CENTRE: usize = 5;
const REQUIRED_INSTANCE_TRANSITIONS: usize = 4;
const FIRST_INSTANCE: usize = 1;
const OUTSIDE_AGAIN: usize = 2;
const REJOINED_INSTANCE: usize = 3;
const EXPECTED_BOSS_DEATHS: usize = 1;
const SELECTION_LABEL: &str = "1";
const STACK_ITEM: usize = 0;
const STACK_QUANTITY: usize = 1;
const ADJACENT_WORLD_TICK: i64 = 1;

fn food_count(player: &Value) -> anyhow::Result<i64> {
    player["backpack"]
        .as_array()
        .context("backpack observations")?
        .iter()
        .filter(|stack| stack[STACK_ITEM] == obj::SHARK.id())
        .map(|stack| stack[STACK_QUANTITY].as_i64().context("food quantity"))
        .sum()
}

fn is_mole(definition: i32) -> bool {
    definition == npc::GIANT_MOLE.id() || definition == npc::GIANT_MOLE_ENRAGED.id()
}

fn inventory(replay: &mut Replay, id: i32) -> Vec<(i32, i32)> {
    ui(replay)
        .engine
        .inv_cache
        .inventory(id, false)
        .map(|inventory| {
            inventory
                .obj_ids
                .iter()
                .copied()
                .zip(inventory.counts.iter().copied())
                .collect()
        })
        .unwrap_or_default()
}

fn integer(row: &Value, field: &str) -> anyhow::Result<i64> {
    row[field]
        .as_i64()
        .with_context(|| format!("{field}: {row}"))
}

fn server_fight(root: &std::path::Path) -> anyhow::Result<()> {
    let plan: Value = serde_json::from_slice(&std::fs::read(root.join("plan.json"))?)?;
    let expected_homes: BTreeSet<_> = plan["boss"]["homes"]
        .as_array()
        .context("named chamber anchors")?
        .iter()
        .map(|home| Ok((integer(home, "x")?, integer(home, "z")?)))
        .collect::<anyhow::Result<_>>()?;
    let rows: Vec<Value> = std::fs::read_to_string(root.join("combat-receipts.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let deaths: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["kind"] == "death"
                && row["targetDefinition"]
                    .as_i64()
                    .is_some_and(|definition| is_mole(definition as i32))
        })
        .collect();
    assert_eq!(deaths.len(), EXPECTED_BOSS_DEATHS);
    let death = deaths.first().context("Mole death")?;
    assert_eq!(
        integer(death, "hideTick")? - integer(death, "tick")?,
        CORPSE_TICKS,
        "the ordinary body lifetime precedes reward release"
    );
    let target_id = &death["targetId"];
    let mut full_health = false;
    let mut dying_body = false;
    let mut removed_body = false;
    let mut homes = BTreeSet::new();
    let mut membership = Vec::new();
    let mut ground_before_hide = false;
    let mut ground_after_hide = false;
    let mut previous_player: Option<(&Value, i64)> = None;
    let mut consumed_food = false;
    let mut food_healed = false;
    for row in rows.iter().filter(|row| row["kind"] == "state") {
        let at = integer(row, "tick")?;
        for enemy in row["enemies"].as_array().context("enemy observations")? {
            if enemy["id"] != *target_id {
                continue;
            }
            full_health |= integer(enemy, "hitpoints")? == FULL_MOLE_LIFE;
            if at <= integer(death, "tick")? {
                homes.insert((integer(&enemy["home"], "x")?, integer(&enemy["home"], "z")?));
            }
            if at == integer(death, "tick")? && row["phase"] == "players" {
                dying_body = enemy["visible"] == true && enemy["hitpoints"] == ZERO;
                assert_eq!(enemy["configuredDeathTicks"], CORPSE_TICKS);
                let modes = enemy["animation"]["modes"]
                    .as_array()
                    .context("death sequence")?;
                assert!(!modes.is_empty());
                assert!(modes.iter().all(|mode| mode == seq::GIANT_MOLE_DEATH.id()));
            }
            if at == integer(death, "hideTick")? && row["phase"] == "npcs" {
                removed_body = enemy["visible"] == false && enemy["loot"].is_null();
            }
        }
        let bones_on_ground = row["ground"]
            .as_array()
            .context("ground observations")?
            .iter()
            .any(|drop| drop["id"] == obj::BIG_BONES.id());
        if at >= integer(death, "tick")? && at < integer(death, "hideTick")? {
            ground_before_hide |= bones_on_ground;
        }
        if at == integer(death, "hideTick")? && row["phase"] == "npcs" {
            ground_after_hide |= bones_on_ground;
        }
        let player = row["players"]
            .as_array()
            .context("player observations")?
            .get(FIRST_PLAYER)
            .context("recorded player")?;
        assert!(
            integer(player, "life")? > i64::from(ZERO),
            "no fixture heal or death"
        );
        if row["phase"] == "npcs" {
            if let Some((prior, prior_tick)) = previous_player {
                if !player["instance"].is_null() && player["instance"] == prior["instance"] {
                    let eaten = food_count(prior)? > food_count(player)?;
                    consumed_food |= eaten;
                    food_healed |= eaten
                        && at - prior_tick <= ADJACENT_WORLD_TICK
                        && integer(player, "life")? - integer(prior, "life")?
                            > integer(player, "regeneration")?;
                }
            }
            previous_player = Some((player, at));
        }
        if membership.last() != Some(&player["instance"]) {
            membership.push(player["instance"].clone());
        }
    }
    assert!(full_health && dying_body && removed_body);
    assert!(
        expected_homes.is_subset(&homes),
        "all named chambers and centre visited during the defeated life"
    );
    assert!(!ground_before_hide && ground_after_hide);
    assert!(
        consumed_food && food_healed,
        "native food removal contributes beyond ordinary regeneration"
    );
    assert_eq!(membership.len(), REQUIRED_INSTANCE_TRANSITIONS);
    assert!(membership[FIRST_PLAYER].is_null());
    assert!(membership[FIRST_INSTANCE].is_string());
    assert!(membership[OUTSIDE_AGAIN].is_null());
    assert_eq!(membership[FIRST_INSTANCE], membership[REJOINED_INSTANCE]);
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_giant_mole_full_fight_loot_exit_and_rejoin() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    server_fight(&root)?;
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut sequences = BTreeSet::new();
    let mut positions = BTreeSet::new();
    let mut tile_spots = BTreeSet::new();
    let mut minion = false;
    let mut seen = false;
    let mut dead = false;
    let mut gone = false;
    let mut damaging_hit = false;
    let mut empty_health_bar = false;
    let mut ground_loot = false;
    let mut cached_selection = false;
    let mut region_transitions = Vec::new();
    let mut attack = false;
    let mut walk = false;
    let mut take = false;
    let mut minimum_life = MAXIMUM_PLAYER_LIFE;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    let mut previous_food = ZERO;
    let mut previous_life = ZERO;
    let mut native_food_removed = false;
    let mut native_food_healed = false;
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
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
            "cycle {cycle}: gameplay packets"
        );
        for (opcode, payload) in client_frames(&out.written)? {
            if opcode == crate::proto::client::CLIENT_CHEAT {
                let command = String::from_utf8_lossy(&payload);
                attack |= command.contains("opnpc ");
                walk |= command.contains("walk ");
                take |= command.contains("takeobj ");
                assert!(
                    !command.contains("hit ")
                        && !command.contains("invset ")
                        && !command.contains("tele "),
                    "fight uses ordinary movement, combat and loot owners"
                );
            }
        }
        let current_food: i32 = inventory(&mut replay, inv::BACKPACK.id())
            .iter()
            .filter(|(id, _)| *id == obj::SHARK.id())
            .map(|(_, count)| *count)
            .sum();
        let game = replay.game();
        let installed = game.runtime.installed_region.is_some();
        if region_transitions.last() != Some(&installed) {
            region_transitions.push(installed);
        }
        let state = &game.runtime.feed.state;
        for enemy in state.npcs.entities.values() {
            minion |= enemy.type_id == npc::MOLE_MINION.id();
            if !is_mole(enemy.type_id) {
                continue;
            }
            seen = true;
            let sequence = enemy.path.animation.main.id();
            sequences.insert(sequence);
            positions.insert((
                enemy.path.x[PATH_HEAD] + game.runtime.map.base_x,
                enemy.path.z[PATH_HEAD] + game.runtime.map.base_z,
            ));
            dead |= sequence == seq::GIANT_MOLE_DEATH.id();
            if let Some(combat) = &enemy.path.combat {
                damaging_hit |= combat.hits.iter().any(|hit| hit[HIT_DAMAGE] > ZERO);
                empty_health_bar |= combat.bars.iter().any(|bar| {
                    bar.updates
                        .iter()
                        .any(|update| update[HEALTH_BAR_END] == ZERO)
                });
            }
        }
        gone |= dead
            && !state
                .npcs
                .entities
                .values()
                .any(|enemy| is_mole(enemy.type_id));
        tile_spots.extend(
            state
                .zones
                .transients
                .spots
                .iter()
                .map(|effect| effect.effect),
        );
        ground_loot |= state
            .zones
            .objects
            .stacks
            .values()
            .any(|stack| stack.iter().any(|item| item.id == obj::BIG_BONES.id()));
        if seen {
            let current_life = game
                .varbit_value(varbit::CURRENT_LIFE_POINTS.id() as u16)
                .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            let food_removed = previous_food > current_food;
            native_food_removed |= food_removed;
            native_food_healed |= food_removed && current_life > previous_life;
            minimum_life = minimum_life.min(current_life);
            previous_life = current_life;
        }
        previous_food = current_food;
        cached_selection |= all_components(ui(&mut replay)).iter().any(|component| {
            let component = component.borrow();
            component.f.parentlayer == component::encounter_setup::LIMIT_VALUE.packed()
                && component.f.id == STATIC_COMPONENT_CHILD
                && component
                    .f
                    .text
                    .as_deref()
                    .map(String::from_utf16_lossy)
                    .is_some_and(|text| text == SELECTION_LABEL)
        });
    }
    assert_eq!(replay_pings, recorded_pings, "ICMP bytes in stream order");
    assert!(seen && dead && gone && minion && damaging_hit && empty_health_bar);
    assert!(positions.len() >= REQUIRED_CHAMBERS_AND_CENTRE);
    for sequence in [
        seq::GIANT_MOLE_BURROW,
        seq::GIANT_MOLE_EMERGE,
        seq::GIANT_MOLE_ENRAGE,
    ] {
        assert!(
            sequences.contains(&sequence.id()),
            "native sequence {}",
            sequence.id()
        );
    }
    assert!(
        tile_spots.contains(&spot::GIANT_MOLE_TREMOR.id()),
        "native tiled effect"
    );
    assert!(ground_loot && attack && walk && take && cached_selection);
    assert!(minimum_life > ZERO, "ordinary player survived");
    assert!(
        native_food_removed && native_food_healed,
        "ordinary inventory and life consumers applied native eating"
    );
    assert_eq!(region_transitions, [false, true, false, true]);
    let packets = arrivals(&trace)?;
    assert!(packets.iter().any(|packet| {
        packet.opcode == crate::proto::server::IF_OPENSUB
            && if_opensub(&packet.payload)[FIRST_INSTANCE] == interface::ENCOUNTER_SETUP.id()
    }));
    assert!(inventory(&mut replay, inv::BACKPACK.id())
        .iter()
        .any(|(id, count)| *id == obj::BIG_BONES.id() && *count > ZERO));
    let worn = inventory(&mut replay, inv::WORN_EQUIPMENT.id());
    for item in [obj::ABYSSAL_WHIP, obj::WOODEN_SHIELD] {
        assert!(
            worn.iter().any(|(id, _)| *id == item.id()),
            "native equipment transaction"
        );
    }
    Ok(())
}
