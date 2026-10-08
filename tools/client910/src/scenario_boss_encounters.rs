//! Whole ordinary-client encounter recordings, native input and independently observed lives.
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Replay, Trace,
    AUTHENTICATED_HEADLESS_BACKEND, INITIAL_RECORDING_OUTPUT_CYCLE, OBSERVED_BACKEND_HEAD,
};
use crate::client_core::input_event::{InputEvent, RECORD_TAG};
use anyhow::{ensure, Context};
use rs910_symbols::{loc, npc};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const BOSS_FIXTURE: &str = "fixtures/session-replay/boss-encounters/dagannoth-kings";
const BOSS_FIRST_CYCLE: i32 = 1;
const BOSS_FIRST: usize = 0;
const BOSS_ONE: usize = 1;
const BOSS_ZERO: i64 = 0;
const BOSS_TILE_FIELDS: usize = 3;
const BOSS_HIT_DAMAGE: usize = 1;
const BOSS_HEALTH_BAR_END: usize = 2;
const BOSS_KING_COUNT: usize = 3;
const BOSS_HIGHER_TIER: i64 = 92;
const BOSS_ARMOUR_TIER: i64 = 90;
const BOSS_INITIAL_LEVEL: i64 = 99;
const BOSS_CLOSING_PUBLICATION_LIMIT: usize = 1;

pub(super) fn read_boss_json(root: &Path, name: &str) -> anyhow::Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(root.join(name))?)?)
}
pub(super) fn read_boss_rows(root: &Path, name: &str) -> anyhow::Result<Vec<Value>> {
    let source = std::fs::read_to_string(root.join(name))?;
    ensure!(source.ends_with('\n'), "complete evidence rows");
    Ok(source
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?)
}
pub(super) fn boss_integer_field(value: &Value, field: &str) -> anyhow::Result<i64> {
    value[field]
        .as_i64()
        .with_context(|| format!("{field}: {value}"))
}
pub(super) fn boss_route_stage<'a>(journal: &'a [Value], name: &str) -> anyhow::Result<&'a Value> {
    let found: Vec<_> = journal.iter().filter(|row| row["kind"] == name).collect();
    ensure!(found.len() == BOSS_ONE, "one completed {name} checkpoint");
    Ok(found[BOSS_FIRST])
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_dagannoth_kings_native_full_fight_loot_exit_and_rejoin() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(BOSS_FIXTURE);
    let execution = read_boss_json(&root, "execution.json")?;
    assert_eq!(execution["status"], "completed_ordinary_boss_encounter");
    assert_eq!(execution["rendered"], false);
    assert_eq!(execution["captureOwnerPassed"], true);
    assert_eq!(execution["allOriginalChildrenWaited"], true);
    let plan = read_boss_json(&root, "plan.json")?;
    assert_eq!(plan["room"]["id"], "dagannoth-kings");
    assert_eq!(
        plan["room"]["entryLoc"],
        loc::DAGANNOTH_DUNGEON_ENTRY_LADDER.id()
    );
    assert_eq!(
        plan["room"]["exitLoc"],
        loc::DAGANNOTH_LAIR_EXIT_LADDER.id()
    );
    assert_eq!(plan["target"]["npc"], npc::DAGANNOTH_SUPREME.id());
    assert_eq!(plan["loadout"]["tier"], BOSS_HIGHER_TIER);
    assert_eq!(plan["loadout"]["armourTier"], BOSS_ARMOUR_TIER);
    assert_eq!(plan["loadout"]["style"], "melee");
    assert_eq!(plan["loadout"]["blocked"], serde_json::json!([]));
    let kings = plan["kings"].as_array().context("qualified inhabitants")?;
    assert_eq!(kings.len(), BOSS_KING_COUNT);
    let definitions: BTreeSet<_> = kings
        .iter()
        .map(|row| boss_integer_field(row, "npc"))
        .collect::<anyhow::Result<_>>()?;
    assert_eq!(
        definitions,
        [
            npc::DAGANNOTH_SUPREME,
            npc::DAGANNOTH_PRIME,
            npc::DAGANNOTH_REX
        ]
        .map(|id| i64::from(id.id()))
        .into_iter()
        .collect()
    );
    let initial = read_boss_json(&root, "initial-fixture.json")?;
    assert_eq!(initial["fixtureOnly"], true);
    ensure!(
        initial["account"].get("resources").is_none(),
        "initial fixture does not assist resources"
    );
    ensure!(
        initial["account"]["skills"]
            .as_array()
            .context("initial native skills")?
            .iter()
            .all(|row| row["level"] == BOSS_INITIAL_LEVEL),
        "qualified initial skill levels"
    );
    let journal = read_boss_rows(&root, "driver-journal.jsonl")?;
    let combat = read_boss_rows(&root, "combat-receipts.jsonl")?;
    let backend = read_boss_json(&root, "backend-inputs.json")?;
    assert_eq!(backend["closure"]["closed"], true);
    assert_eq!(backend["closure"]["coreClosed"], true);
    assert_eq!(backend["closure"]["errors"], serde_json::json!([]));
    assert_eq!(
        backend["closure"]["socketPromises"]["pending"],
        serde_json::json!([])
    );
    let entered = boss_route_stage(&journal, "enter")?;
    let died = boss_route_stage(&journal, "boss_death")?;
    let looted = boss_route_stage(&journal, "loot")?;
    let left = boss_route_stage(&journal, "leave")?;
    let rejoined = boss_route_stage(&journal, "rejoin")?;
    let complete = boss_route_stage(&journal, "complete")?;
    assert_eq!(complete["status"], "ordinary_encounter_pass");
    let instance = &entered["passive"]["instance"];
    assert_eq!(instance, &rejoined["passive"]["instance"]);
    assert_eq!(instance, &complete["instance"]);
    assert_eq!(left["passive"]["instance"], Value::Null);
    assert_eq!(
        died["fullLife"]["hitpoints"],
        plan["target"]["profile"]["hitpoints"]
    );
    assert_eq!(died["fullLife"]["definition"], plan["target"]["npc"]);
    for king in kings {
        ensure!(
            combat
                .iter()
                .filter(|row| row["kind"] == "state")
                .any(|row| row["enemies"]
                    .as_array()
                    .is_some_and(|enemies| enemies
                        .iter()
                        .any(|enemy| enemy["definition"] == king["npc"]
                            && enemy["hitpoints"] == king["maximumLife"]
                            && enemy["instance"] == *instance))),
            "each ordinary king starts at qualified full health"
        );
    }
    let target_index = boss_integer_field(&died["fullLife"], "id")?;
    let generation = &died["fullLife"]["generation"];
    let player_index = &entered["passive"]["pid"];
    let deaths: Vec<_> = combat
        .iter()
        .filter(|row| {
            row["kind"] == "death"
                && row["target"]["id"].as_i64() == Some(target_index)
                && row["target"]["generation"] == *generation
        })
        .collect();
    ensure!(deaths.len() == BOSS_ONE, "one actual full boss life dies");
    assert_eq!(deaths[BOSS_FIRST]["source"]["id"], *player_index);
    let hits: Vec<_> = combat
        .iter()
        .filter(|row| {
            row["kind"] == "landed"
                && row["source"]["id"] == *player_index
                && row["target"]["id"].as_i64() == Some(target_index)
                && row["target"]["generation"] == *generation
        })
        .collect();
    let damages: BTreeSet<_> = hits
        .iter()
        .map(|row| boss_integer_field(row, "nativeDamage"))
        .collect::<anyhow::Result<_>>()?;
    for hit in &hits {
        ensure!(
            boss_integer_field(hit, "nativeDamage")? >= boss_integer_field(hit, "actualDamage")?,
            "native overkill retains its hitsplat while credited loss is clamped"
        );
    }
    let damage_total = hits.iter().try_fold(BOSS_ZERO, |sum, row| {
        Ok::<_, anyhow::Error>(sum + boss_integer_field(row, "actualDamage")?)
    })?;
    assert_eq!(
        damage_total,
        boss_integer_field(&plan["target"]["profile"], "hitpoints")?
    );
    ensure!(
        damages.iter().any(|damage| *damage > BOSS_ZERO),
        "ordinary positive impacts"
    );
    assert_eq!(looted["item"], plan["loot"]);
    replay_ordinary_encounter(EncounterReplay {
        root: &root,
        journal: &journal,
        backend: &backend,
        complete,
        target_index,
        target_definition: npc::DAGANNOTH_SUPREME.id(),
        definitions: &definitions,
        damages: &damages,
        placed_locations: BTreeMap::new(),
    })
}

struct EncounterReplay<'a> {
    root: &'a Path,
    journal: &'a [Value],
    backend: &'a Value,
    complete: &'a Value,
    target_index: i64,
    target_definition: i32,
    definitions: &'a BTreeSet<i64>,
    damages: &'a BTreeSet<i64>,
    placed_locations: BTreeMap<i64, i64>,
}

fn replay_ordinary_encounter(recording: EncounterReplay<'_>) -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let EncounterReplay {
        root,
        journal,
        backend,
        complete,
        target_index,
        target_definition,
        definitions,
        damages,
        placed_locations,
    } = recording;
    let trace = Trace::load(&root.join("session.rtr"))?;
    assert_eq!(
        trace.head(OBSERVED_BACKEND_HEAD)?,
        AUTHENTICATED_HEADLESS_BACKEND
    );
    let inputs: Vec<_> = trace
        .records
        .iter()
        .filter(|row| row.tag == *RECORD_TAG)
        .collect();
    let applied = backend["inputs"]
        .as_array()
        .context("actual applied inputs")?;
    let accepted: Vec<_> = journal
        .iter()
        .filter(|row| {
            row["kind"] == "control"
                && row["request"]["command"] == "action"
                && row["response"]["status"] == "accepted"
        })
        .collect();
    assert_eq!(inputs.len(), applied.len());
    assert_eq!(inputs.len(), accepted.len());
    assert_eq!(complete["actions"].as_u64(), Some(inputs.len() as u64));
    for ((record, receipt), control) in inputs.iter().zip(applied).zip(accepted) {
        assert_eq!(
            record.cycle,
            i32::try_from(boss_integer_field(receipt, "cycle")?)?
        );
        assert_eq!(
            record.bytes,
            serde_json::from_value::<Vec<u8>>(receipt["event"].clone())?
        );
        assert_eq!(control["response"]["cycle"], record.cycle);
        let action = &control["request"]["action"];
        let expected_parent = action["definition"].as_i64().map(|definition| {
            placed_locations
                .get(&definition)
                .copied()
                .unwrap_or(definition)
        });
        let placed_parent = if action["kind"] == "loc" {
            let observed = journal
                .iter()
                .take_while(|row| !std::ptr::eq(*row, control))
                .filter(|row| {
                    row["kind"] == "control"
                        && row["request"]["command"] == "scan_locs"
                        && row["response"]["status"] == "observed"
                        && row["response"]["data"]["map"] == control["request"]["map"]
                })
                .flat_map(|row| {
                    row["response"]["data"]["scan"]["entries"]
                        .as_array()
                        .into_iter()
                        .flatten()
                })
                .filter(|row| {
                    ["definition", "level", "x", "z", "shape", "angle"]
                        .into_iter()
                        .all(|field| row[field] == action[field])
                })
                .last()
                .and_then(|row| row["base_definition"].as_i64());
            ensure!(
                observed.is_some() && observed == expected_parent,
                "accepted loc retains its exact observed and symbol-qualified native parent"
            );
            observed
        } else {
            expected_parent
        };
        ensure!(
            super::scenario_god_wars::input_matches(
                action,
                &control["request"]["map"],
                placed_parent,
                &InputEvent::decode(&record.bytes)?
            ),
            "actual accepted ordinary input at {}: {action}",
            record.cycle
        );
    }
    let frames = arrivals(&trace)?;
    let (players, npcs) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let snapshots = journal
        .iter()
        .filter(|row| {
            row["kind"] == "control"
                && row["request"]["command"] == "snapshot"
                && row["response"]["data"]["ready"] == true
        })
        .try_fold(BTreeMap::<i32, Vec<&Value>>::new(), |mut grouped, row| {
            grouped
                .entry(i32::try_from(boss_integer_field(
                    &row["response"],
                    "cycle",
                )?)?)
                .or_default()
                .push(row);
            Ok::<_, anyhow::Error>(grouped)
        })?;
    let wanted_snapshots: usize = snapshots.values().map(Vec::len).sum();
    let mut replay = Replay::start(&trace)?;
    assert_eq!(
        replay.io.take_written(),
        trace.bytes(b"OUT ", INITIAL_RECORDING_OUTPUT_CYCLE)
    );
    let (mut done, mut player_updates, mut npc_updates, mut checked_snapshots) =
        (BOSS_FIRST, BOSS_FIRST, BOSS_FIRST, BOSS_FIRST);
    let mut native_impacts = BTreeSet::new();
    let mut native_kings = BTreeSet::new();
    let mut target_seen = false;
    let mut target_definition_seen = false;
    let mut zero_health_bar = false;
    let mut corpse_removed = false;
    for cycle in BOSS_FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        let expected = trace.bytes(b"OUT ", cycle);
        assert_eq!(
            mask_wall_clock(&output.written)?,
            mask_wall_clock(&expected)?,
            "cycle{cycle}: whole ordered normal output"
        );
        for bytes in [&output.written, &expected] {
            ensure!(
                client_frames(bytes)?
                    .iter()
                    .all(|(opcode, _)| *opcode != cp::CLIENT_CHEAT),
                "no cheat input"
            );
        }
        let next = replay.processed(&frames);
        let fresh = &frames[done..next];
        ensure!(
            fresh
                .iter()
                .filter(|frame| frame.opcode == sp::PLAYER_INFO)
                .count()
                <= BOSS_ONE
                && fresh
                    .iter()
                    .filter(|frame| frame.opcode == sp::NPC_INFO)
                    .count()
                    <= BOSS_ONE,
            "each applied actor tick is independently observable"
        );
        let state = observe(replay.game());
        for frame in fresh {
            if frame.opcode == sp::PLAYER_INFO {
                let expected = players
                    .get(player_updates)
                    .context("ordered native PLAYER publication")?;
                assert_eq!(
                    state.local.as_ref().context("local player")?.tile,
                    expected[..BOSS_TILE_FIELDS]
                );
                player_updates += BOSS_ONE;
            }
            if frame.opcode == sp::NPC_INFO {
                let expected: BTreeMap<_, _> = npcs
                    .get(npc_updates)
                    .context("ordered native NPC publication")?
                    .iter()
                    .map(|&[index, definition, x, z, level]| {
                        (index as usize, (definition, [x, z, level]))
                    })
                    .collect();
                let actual: BTreeMap<_, _> = state
                    .npcs
                    .iter()
                    .map(|(&index, (definition, actor))| (index, (*definition, actor.tile)))
                    .collect();
                assert_eq!(actual, expected, "all native roster publications");
                npc_updates += BOSS_ONE;
            }
        }
        done = next;
        for row in snapshots.get(&cycle).into_iter().flatten() {
            let session = replay.core.session.as_ref().context("ordinary session")?;
            let actual = crate::live_control::observed_snapshot_for_test(
                session,
                cycle,
                serde_json::from_value(row["request"]["query"].clone())?,
            )?;
            for key in [
                "ready",
                "player",
                "varps",
                "varbits",
                "client_varbits",
                "inventories",
                "components",
                "stats",
            ] {
                assert_eq!(
                    actual[key], row["response"]["data"][key],
                    "cycle{cycle}: native {key}"
                );
            }
            checked_snapshots += BOSS_ONE;
        }
        for enemy in replay.game().runtime.feed.state.npcs.entities.values() {
            if definitions.contains(&i64::from(enemy.type_id)) {
                native_kings.insert(enemy.type_id);
            }
        }
        if let Some(enemy) = replay
            .game()
            .runtime
            .feed
            .state
            .npcs
            .entities
            .get(&(target_index as usize))
        {
            if definitions.contains(&i64::from(enemy.type_id)) {
                target_seen = true;
                target_definition_seen |= enemy.type_id == target_definition;
                if let Some(combat) = &enemy.path.combat {
                    zero_health_bar |= combat.bars.iter().any(|bar| {
                        bar.updates
                            .iter()
                            .any(|update| i64::from(update[BOSS_HEALTH_BAR_END]) == BOSS_ZERO)
                    });
                    for hit in &combat.hits {
                        let damage = i64::from(hit[BOSS_HIT_DAMAGE]);
                        if damages.contains(&damage) {
                            native_impacts.insert(damage);
                        }
                    }
                }
            }
        } else if target_seen {
            corpse_removed = true;
        }
    }
    ensure!(
        target_definition_seen && zero_health_bar && corpse_removed,
        "ordinary native death bar and corpse removal"
    );
    assert_eq!(done, frames.len());
    assert_eq!(
        player_updates,
        frames
            .iter()
            .filter(|frame| frame.opcode == sp::PLAYER_INFO)
            .count()
    );
    assert_eq!(
        npc_updates,
        frames
            .iter()
            .filter(|frame| frame.opcode == sp::NPC_INFO)
            .count()
    );
    ensure!(
        players.len() >= player_updates
            && npcs.len() >= npc_updates
            && players.len() - player_updates <= BOSS_CLOSING_PUBLICATION_LIMIT
            && npcs.len() - npc_updates <= BOSS_CLOSING_PUBLICATION_LIMIT,
        "only one unreceived closing actor tick may remain in the complete server log"
    );
    let final_player = players
        .get(
            player_updates
                .checked_sub(BOSS_ONE)
                .context("received player publication")?,
        )
        .context("final received player row")?;
    let final_npcs = npcs
        .get(
            npc_updates
                .checked_sub(BOSS_ONE)
                .context("received NPC publication")?,
        )
        .context("final received NPC row")?;
    ensure!(
        players[player_updates..]
            .iter()
            .all(|row| row == final_player)
            && npcs[npc_updates..].iter().all(|row| row == final_npcs),
        "unreceived closing publications duplicate the final received actor state"
    );
    assert_eq!(checked_snapshots, wanted_snapshots);
    ensure!(
        checked_snapshots > BOSS_FIRST && player_updates > BOSS_FIRST && npc_updates > BOSS_FIRST,
        "ordinary input, UI and actors are installed"
    );
    assert_eq!(native_kings.len(), definitions.len());
    assert_eq!(
        &native_impacts, damages,
        "every published hitsplat value appears in decoded native NPC state"
    );
    Ok(())
}

const QUEEN_FIXTURE: &str = "fixtures/session-replay/kalphite-queen/recorded";
const QUEEN_FULL_FORM_LIFE: i64 = 40000;

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_kalphite_queen_native_two_lives_switch_loot_exit_and_rejoin() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(QUEEN_FIXTURE);
    let execution = read_boss_json(&root, "execution.json")?;
    assert_eq!(execution["status"], "completed_ordinary_queen_full_route");
    assert_eq!(execution["rendered"], false);
    assert_eq!(execution["captureOwnerPassed"], true);
    assert_eq!(execution["allOriginalChildrenWaited"], true);
    let plan = read_boss_json(&root, "plan.json")?;
    assert_eq!(plan["room"]["id"], "kalphite-queen");
    assert_eq!(
        plan["room"]["entryLoc"],
        loc::KALPHITE_QUEEN_ENTRY_ROPED.id()
    );
    assert_eq!(
        plan["room"]["entryParent"],
        loc::KALPHITE_QUEEN_ENTRY_PARENT.id()
    );
    assert_eq!(plan["room"]["exitLoc"], loc::KALPHITE_QUEEN_EXIT_ROPE.id());
    assert_eq!(plan["ground"]["npc"], npc::KALPHITE_QUEEN_GROUND.id());
    assert_eq!(plan["flying"]["npc"], npc::KALPHITE_QUEEN_FLYING.id());
    let initial = read_boss_json(&root, "initial-fixture.json")?;
    ensure!(
        initial["fixtureOnly"] == true && initial["account"].get("resources").is_none(),
        "ordinary initial fixture"
    );
    ensure!(
        initial["account"]["skills"]
            .as_array()
            .context("initial skills")?
            .iter()
            .all(|row| row["level"] == BOSS_INITIAL_LEVEL),
        "full native initial skill levels"
    );
    let journal = read_boss_rows(&root, "driver-journal.jsonl")?;
    let combat = read_boss_rows(&root, "combat-receipts.jsonl")?;
    let backend = read_boss_json(&root, "backend-inputs.json")?;
    assert_eq!(backend["closure"]["closed"], true);
    assert_eq!(backend["closure"]["coreClosed"], true);
    assert_eq!(backend["closure"]["errors"], serde_json::json!([]));
    assert_eq!(
        backend["closure"]["socketPromises"]["pending"],
        serde_json::json!([])
    );
    let entered = boss_route_stage(&journal, "enter")?;
    let transformed = boss_route_stage(&journal, "full_ground_to_flying")?;
    let finished = boss_route_stage(&journal, "full_flying_death")?;
    let complete = boss_route_stage(&journal, "complete")?;
    assert_eq!(complete["status"], "ordinary_queen_full_pass");
    let actor = boss_integer_field(&transformed["fullLife"], "id")?;
    let ground_generation = boss_integer_field(transformed, "oldGeneration")?;
    assert_eq!(
        boss_integer_field(transformed, "newGeneration")?,
        ground_generation + BOSS_ONE as i64
    );
    assert_eq!(finished["fullLife"]["id"], actor);
    assert_eq!(
        finished["fullLife"]["generation"],
        ground_generation + BOSS_ONE as i64
    );
    assert_eq!(
        entered["passive"]["instance"],
        finished["fullLife"]["instance"]
    );
    assert_eq!(entered["passive"]["xp"], transformed["passive"]["xp"]);
    let player = &entered["passive"]["pid"];
    let mut damages = BTreeSet::new();
    for (phase, generation, stage, style) in [
        ("ground", ground_generation, transformed, "melee"),
        (
            "flying",
            ground_generation + BOSS_ONE as i64,
            finished,
            "ranged",
        ),
    ] {
        assert_eq!(stage["fullLife"]["definition"], plan[phase]["npc"]);
        assert_eq!(stage["fullLife"]["hitpoints"], QUEEN_FULL_FORM_LIFE);
        let hits: Vec<_> = combat
            .iter()
            .filter(|row| {
                row["kind"] == "landed"
                    && row["source"]["id"] == *player
                    && row["target"]["id"].as_i64() == Some(actor)
                    && row["target"]["generation"].as_i64() == Some(generation)
            })
            .collect();
        let total = hits.iter().try_fold(BOSS_ZERO, |sum, row| {
            assert_eq!(row["style"], style);
            let actual = boss_integer_field(row, "actualDamage")?;
            let native = boss_integer_field(row, "nativeDamage")?;
            ensure!(native >= actual, "ordinary overkill clamping");
            damages.insert(native);
            Ok::<_, anyhow::Error>(sum + actual)
        })?;
        assert_eq!(total, QUEEN_FULL_FORM_LIFE);
    }
    let deaths: Vec<_> = combat
        .iter()
        .filter(|row| row["kind"] == "death" && row["target"]["id"].as_i64() == Some(actor))
        .collect();
    let rewards: Vec<_> = combat
        .iter()
        .filter(|row| row["kind"] == "reward_owner" && row["target"]["id"].as_i64() == Some(actor))
        .collect();
    assert_eq!(deaths.len(), BOSS_ONE);
    assert_eq!(rewards.len(), BOSS_ONE);
    assert_eq!(
        deaths[BOSS_FIRST]["target"]["generation"],
        ground_generation + BOSS_ONE as i64
    );
    assert_eq!(rewards[BOSS_FIRST]["owner"]["id"], *player);
    assert_eq!(
        boss_route_stage(&journal, "leave")?["passive"]["instance"],
        Value::Null
    );
    assert_eq!(
        boss_route_stage(&journal, "rejoin")?["passive"]["instance"],
        entered["passive"]["instance"]
    );
    for phase in ["ground", "flying"] {
        assert_eq!(
            plan["kits"][phase]["loadout"]["blocked"],
            serde_json::json!([])
        );
        let equipped = boss_route_stage(&journal, &format!("equipped_{phase}"))?;
        assert_eq!(equipped["equipment"], plan["kits"][phase]["equipment"]);
    }
    let definitions = [npc::KALPHITE_QUEEN_GROUND, npc::KALPHITE_QUEEN_FLYING]
        .map(|id| i64::from(id.id()))
        .into_iter()
        .collect();
    replay_ordinary_encounter(EncounterReplay {
        root: &root,
        journal: &journal,
        backend: &backend,
        complete,
        target_index: actor,
        target_definition: npc::KALPHITE_QUEEN_FLYING.id(),
        definitions: &definitions,
        damages: &damages,
        placed_locations: BTreeMap::from([(
            i64::from(loc::KALPHITE_QUEEN_ENTRY_ROPED.id()),
            i64::from(loc::KALPHITE_QUEEN_ENTRY_PARENT.id()),
        )]),
    })
}
