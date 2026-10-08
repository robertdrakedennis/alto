//! Whole ordinary-client encounter recordings, native input and independently observed lives.
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Replay, Trace,
    AUTHENTICATED_HEADLESS_BACKEND, INITIAL_RECORDING_OUTPUT_CYCLE, OBSERVED_BACKEND_HEAD,
};
use crate::client_core::input_event::{InputEvent, RECORD_TAG};
use anyhow::{ensure, Context};
use rs910_symbols::npc;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const CHAOS_FIXTURE: &str =
    "fixtures/session-replay/chaos-elemental/recorded-full-life-loot-retreat";
const CHAOS_FIRST_CYCLE: i32 = 1;
const CHAOS_FIRST: usize = 0;
const CHAOS_ONE: usize = 1;
const CHAOS_ZERO: i64 = 0;
const CHAOS_TILE_FIELDS: usize = 3;
const CHAOS_HIT_DAMAGE: usize = 1;
const CHAOS_HEALTH_BAR_END: usize = 2;
const CHAOS_TARGET_COUNT: usize = 1;
const CHAOS_WEAPON_TIER: i64 = 90;
const CHAOS_FULL_LIFE: i64 = 17250;
const CHAOS_INITIAL_LEVEL: i64 = 99;
const CHAOS_CLOSING_PUBLICATION_LIMIT: usize = 1;

fn read_chaos_json(root: &Path, name: &str) -> anyhow::Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(root.join(name))?)?)
}
fn read_chaos_rows(root: &Path, name: &str) -> anyhow::Result<Vec<Value>> {
    let source = std::fs::read_to_string(root.join(name))?;
    ensure!(source.ends_with('\n'), "complete evidence rows");
    Ok(source
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?)
}
fn chaos_integer_field(value: &Value, field: &str) -> anyhow::Result<i64> {
    value[field]
        .as_i64()
        .with_context(|| format!("{field}: {value}"))
}
fn chaos_route_stage<'a>(journal: &'a [Value], name: &str) -> anyhow::Result<&'a Value> {
    let found: Vec<_> = journal.iter().filter(|row| row["kind"] == name).collect();
    ensure!(found.len() == CHAOS_ONE, "one completed {name} checkpoint");
    Ok(found[CHAOS_FIRST])
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_chaos_elemental_public_full_life_loot_and_retreat() -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let root = rs910_core::test_support::client_dir().join(CHAOS_FIXTURE);
    let execution = read_chaos_json(&root, "execution.json")?;
    assert_eq!(
        execution["status"],
        "completed_ordinary_chaos_public_full_life_loot_retreat"
    );
    assert_eq!(execution["captureOwnerPassed"], true);
    assert_eq!(execution["allOriginalChildrenWaited"], true);
    let plan = read_chaos_json(&root, "plan.json")?;
    assert_eq!(plan["target"]["npc"], npc::CHAOS_ELEMENTAL.id());
    assert_eq!(plan["loadout"]["tier"], CHAOS_WEAPON_TIER);
    assert_eq!(plan["loadout"]["style"], "melee");
    assert_eq!(plan["loadout"]["hands"], "dual-wield");
    assert_eq!(plan["loadout"]["blocked"], serde_json::json!([]));
    let initial = read_chaos_json(&root, "initial-fixture.json")?;
    ensure!(
        initial["account"].get("resources").is_none(),
        "no resource fixture"
    );
    ensure!(
        initial["account"]["skills"]
            .as_array()
            .context("native skill seed")?
            .iter()
            .all(|row| row["level"] == CHAOS_INITIAL_LEVEL),
        "full native skills"
    );
    let journal = read_chaos_rows(&root, "driver-journal.jsonl")?;
    let combat = read_chaos_rows(&root, "combat-receipts.jsonl")?;
    let backend = read_chaos_json(&root, "backend-inputs.json")?;
    assert_eq!(backend["closure"]["closed"], true);
    assert_eq!(backend["closure"]["coreClosed"], true);
    assert_eq!(backend["closure"]["errors"], serde_json::json!([]));
    assert_eq!(
        backend["closure"]["socketPromises"]["pending"],
        serde_json::json!([])
    );
    let entered = chaos_route_stage(&journal, "enter")?;
    let died = chaos_route_stage(&journal, "boss_death")?;
    let left = chaos_route_stage(&journal, "leave")?;
    let complete = chaos_route_stage(&journal, "complete")?;
    assert_eq!(
        complete["status"],
        "ordinary_chaos_public_full_life_loot_retreat_pass"
    );
    for stage in [entered, left] {
        assert_eq!(stage["passive"]["instance"], Value::Null);
    }
    let definitions: BTreeSet<_> = [i64::from(npc::CHAOS_ELEMENTAL.id())].into_iter().collect();
    assert_eq!(died["fullLife"]["hitpoints"], CHAOS_FULL_LIFE);
    assert_eq!(plan["target"]["profile"]["hitpoints"], CHAOS_FULL_LIFE);
    let target_index = chaos_integer_field(&died["fullLife"], "id")?;
    let generation = &died["fullLife"]["generation"];
    let player_index = &entered["passive"]["pid"];
    let selected: Vec<_> = combat
        .iter()
        .filter(|row| {
            row["kind"] == "loot_table_roll"
                && row["tick"] == died["reward"]["tick"]
                && row["ownerId"] == *player_index
        })
        .collect();
    ensure!(
        selected.len() == CHAOS_ONE,
        "one actual ordinary Chaos table roll"
    );
    let selection = selected[CHAOS_FIRST];
    let members = selection["ownerMembers"]
        .as_bool()
        .context("actual reward-owner membership field")?;
    let branch = if members { "member" } else { "free" };
    let expected_table = format!(
        "wilderness:chaos-elemental:{}:{branch}",
        npc::CHAOS_ELEMENTAL.id()
    );
    assert_eq!(selection["table"].as_str(), Some(expected_table.as_str()));
    assert_eq!(
        chaos_integer_field(selection, "mainRolls")?,
        i64::try_from(CHAOS_ONE)?
    );
    assert_eq!(
        selection["ownerGeneration"],
        died["reward"]["owner"]["generation"]
    );
    assert_eq!(selection["ownerMembers"], died["reward"]["ownerMembers"]);
    assert_eq!(selection["rolled"], died["reward"]["loot"]);
    let natural: BTreeSet<_> = journal
        .iter()
        .filter(|row| row["kind"] == "natural_special")
        .map(|row| row["effect"].as_str().context("native natural special"))
        .collect::<anyhow::Result<_>>()?;
    let possible: BTreeSet<_> = ["equipment-removal", "displacement"].into_iter().collect();
    ensure!(
        natural.is_subset(&possible),
        "only qualified natural branches"
    );
    assert_eq!(complete["scope"], "full-life-loot-retreat");
    assert_eq!(
        complete["naturalEffectsObserved"],
        serde_json::to_value(&natural)?
    );
    assert_eq!(
        complete["naturalEffectsUnobserved"],
        serde_json::to_value(possible.difference(&natural).collect::<BTreeSet<_>>())?
    );
    for row in journal
        .iter()
        .filter(|row| row["kind"] == "natural_special")
    {
        ensure!(
            combat.contains(&row["delivery"]),
            "ordinary passive delivered effect"
        );
        assert_eq!(row["delivery"]["source"]["id"].as_i64(), Some(target_index));
        assert_eq!(row["delivery"]["source"]["generation"], *generation);
    }
    let deaths: Vec<_> = combat
        .iter()
        .filter(|row| {
            row["kind"] == "death"
                && row["target"]["id"].as_i64() == Some(target_index)
                && row["target"]["generation"] == *generation
        })
        .collect();
    ensure!(deaths.len() == CHAOS_ONE, "one actual full boss life dies");
    assert_eq!(deaths[CHAOS_FIRST]["source"]["id"], *player_index);
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
        .map(|row| chaos_integer_field(row, "nativeDamage"))
        .collect::<anyhow::Result<_>>()?;
    for hit in &hits {
        ensure!(
            chaos_integer_field(hit, "nativeDamage")? >= chaos_integer_field(hit, "actualDamage")?,
            "native overkill retains its hitsplat while credited loss is clamped"
        );
    }
    let damage_total = hits.iter().try_fold(CHAOS_ZERO, |sum, row| {
        Ok::<_, anyhow::Error>(sum + chaos_integer_field(row, "actualDamage")?)
    })?;
    assert_eq!(
        damage_total,
        chaos_integer_field(&plan["target"]["profile"], "hitpoints")?
    );
    ensure!(
        damages.iter().any(|damage| *damage > CHAOS_ZERO),
        "ordinary positive impacts"
    );
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
            i32::try_from(chaos_integer_field(receipt, "cycle")?)?
        );
        assert_eq!(
            record.bytes,
            serde_json::from_value::<Vec<u8>>(receipt["event"].clone())?
        );
        assert_eq!(control["response"]["cycle"], record.cycle);
        let action = &control["request"]["action"];
        ensure!(
            action["kind"] != "loc",
            "public encounter has no loc action in its qualified scope"
        );
        if action["kind"] == "npc" || action["kind"] == "object" {
            let command = if action["kind"] == "npc" {
                "scan_npcs"
            } else {
                "scan_objects"
            };
            let action_index = journal
                .iter()
                .position(|row| std::ptr::eq(row, control))
                .context("original accepted journal row")?;
            let scan = journal[..action_index]
                .iter()
                .rev()
                .find(|row| {
                    row["kind"] == "control"
                        && row["request"]["command"] == command
                        && row["response"]["status"] == "observed"
                        && row["response"]["cycle"] == control["request"]["observed_cycle"]
                        && row["response"]["data"]["map"] == control["request"]["map"]
                })
                .context("accepted scene action retains its exact preceding native scan")?;
            let fields: &[&str] = if command == "scan_npcs" {
                &["index", "definition", "update_serial"]
            } else {
                &[
                    "definition",
                    "x",
                    "z",
                    "level",
                    "stack_index",
                    "count",
                    "object_revision",
                ]
            };
            let entries = scan["response"]["data"]["scan"]["entries"]
                .as_array()
                .context("native scene scan rows")?;
            let matched: Vec<_> = entries
                .iter()
                .filter(|row| fields.iter().all(|field| row[*field] == action[*field]))
                .collect();
            ensure!(
                matched.len() == CHAOS_ONE,
                "one exactly bound accepted scene placement"
            );
            if command == "scan_npcs" {
                assert_eq!(
                    matched[CHAOS_FIRST]["base_definition"],
                    npc::CHAOS_ELEMENTAL.id()
                );
            }
        }
        ensure!(
            super::scenario_god_wars::input_matches(
                action,
                &control["request"]["map"],
                action["definition"].as_i64(),
                &InputEvent::decode(&record.bytes)?
            ),
            "actual accepted ordinary input"
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
                .entry(i32::try_from(chaos_integer_field(
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
        (CHAOS_FIRST, CHAOS_FIRST, CHAOS_FIRST, CHAOS_FIRST);
    let mut native_impacts = BTreeSet::new();
    let mut native_kings = BTreeSet::new();
    let mut target_seen = false;
    let mut zero_health_bar = false;
    let mut corpse_removed = false;
    for cycle in CHAOS_FIRST_CYCLE..=trace.last_cycle() {
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
                <= CHAOS_ONE
                && fresh
                    .iter()
                    .filter(|frame| frame.opcode == sp::NPC_INFO)
                    .count()
                    <= CHAOS_ONE,
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
                    expected[..CHAOS_TILE_FIELDS]
                );
                player_updates += CHAOS_ONE;
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
                npc_updates += CHAOS_ONE;
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
            checked_snapshots += CHAOS_ONE;
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
            if enemy.type_id == npc::CHAOS_ELEMENTAL.id() {
                target_seen = true;
                if let Some(combat) = &enemy.path.combat {
                    zero_health_bar |= combat.bars.iter().any(|bar| {
                        bar.updates
                            .iter()
                            .any(|update| i64::from(update[CHAOS_HEALTH_BAR_END]) == CHAOS_ZERO)
                    });
                    for hit in &combat.hits {
                        let damage = i64::from(hit[CHAOS_HIT_DAMAGE]);
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
        zero_health_bar && corpse_removed,
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
            && players.len() - player_updates <= CHAOS_CLOSING_PUBLICATION_LIMIT
            && npcs.len() - npc_updates <= CHAOS_CLOSING_PUBLICATION_LIMIT,
        "only one unreceived closing actor tick may remain in the complete server log"
    );
    let final_player = players
        .get(
            player_updates
                .checked_sub(CHAOS_ONE)
                .context("received player publication")?,
        )
        .context("final received player row")?;
    let final_npcs = npcs
        .get(
            npc_updates
                .checked_sub(CHAOS_ONE)
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
        checked_snapshots > CHAOS_FIRST
            && player_updates > CHAOS_FIRST
            && npc_updates > CHAOS_FIRST,
        "ordinary input, UI and actors are installed"
    );
    assert_eq!(native_kings.len(), CHAOS_TARGET_COUNT);
    assert_eq!(
        native_impacts, damages,
        "every published hitsplat value appears in decoded native NPC state"
    );
    Ok(())
}
