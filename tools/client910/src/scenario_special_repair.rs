//! Whole native Bob repair, cold outside count reset and timed melee field recording.
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Replay, Trace,
    AUTHENTICATED_HEADLESS_BACKEND, INITIAL_RECORDING_OUTPUT_CYCLE, OBSERVED_BACKEND_HEAD,
};
use crate::client_core::input_event::{InputEvent, RECORD_TAG};
use anyhow::{ensure, Context};
use rs910_symbols::obj;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const SPECIAL_FIXTURE: &str = "fixtures/session-replay/special-repair/recorded";
const SPECIAL_FIRST_CYCLE: i32 = 1;
const SPECIAL_FIRST: usize = 0;
const SPECIAL_ONE: usize = 1;
const SPECIAL_ZERO: i64 = 0;
const SPECIAL_TILE_FIELDS: usize = 3;
const SPECIAL_HIT_DAMAGE: usize = 1;
const SPECIAL_INSTANCE_INDEX: usize = 2;
const SPECIAL_WEAPON_TIER: i64 = 92;
const SPECIAL_ARMOUR_TIER: i64 = 90;
const SPECIAL_INITIAL_LEVEL: i64 = 99;

fn special_json(root: &Path, name: &str) -> anyhow::Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(root.join(name))?)?)
}
fn special_rows(root: &Path, name: &str) -> anyhow::Result<Vec<Value>> {
    let text = std::fs::read_to_string(root.join(name))?;
    ensure!(text.ends_with('\n'), "complete passive evidence rows");
    Ok(text
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?)
}
fn special_integer(row: &Value, field: &str) -> anyhow::Result<i64> {
    row[field]
        .as_i64()
        .with_context(|| format!("{field}: {row}"))
}
fn special_stage<'a>(journal: &'a [Value], name: &str) -> anyhow::Result<&'a Value> {
    let found: Vec<_> = journal.iter().filter(|row| row["kind"] == name).collect();
    ensure!(found.len() == SPECIAL_ONE, "one actual {name} checkpoint");
    Ok(found[SPECIAL_FIRST])
}
fn special_instance<'a>(slots: &'a Value, key: &Value) -> anyhow::Result<&'a Value> {
    let matches: Vec<_> = slots
        .as_array()
        .context("physical inventory slots")?
        .iter()
        .filter(|slot| slot[SPECIAL_INSTANCE_INDEX]["key"] == *key)
        .collect();
    ensure!(matches.len() == SPECIAL_ONE, "one physical UUID");
    Ok(matches[SPECIAL_FIRST])
}
fn special_input(action: &Value, map: &Value, event: &InputEvent) -> bool {
    match (action["kind"].as_str(), event) {
        (
            Some("ui"),
            InputEvent::Component {
                parent,
                child,
                operation,
            },
        ) => {
            action["target"]["parent"].as_i64() == Some(i64::from(*parent))
                && action["target"]["child"].as_i64() == Some(i64::from(*child))
                && action["operation"].as_i64() == Some(i64::from(*operation))
        }
        (Some("npc"), InputEvent::Menu { choice, .. }) => {
            choice.enabled
                && action["index"].as_i64() == Some(choice.entity_id)
                && action["operation"]
                    .as_str()
                    .is_some_and(|label| choice.op.eq_ignore_ascii_case(label))
        }
        (Some("walk"), InputEvent::Menu { choice, .. }) => {
            choice.enabled
                && choice.action == crate::ui_scene_options::WALK_ACTION
                && action["x"]
                    .as_i64()
                    .zip(map["base_x"].as_i64())
                    .is_some_and(|(x, base)| i64::from(choice.tile_x) == x - base)
                && action["z"]
                    .as_i64()
                    .zip(map["base_z"].as_i64())
                    .is_some_and(|(z, base)| i64::from(choice.tile_z) == z - base)
        }
        _ => false,
    }
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn special_recorded_native_bob_repair_outside_gwd_and_timed_melee_sample() -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let root = rs910_core::test_support::client_dir().join(SPECIAL_FIXTURE);
    let execution = special_json(&root, "execution.json")?;
    assert_eq!(execution["status"], "completed_ordinary_special_repair");
    assert_eq!(execution["captureOwnerPassed"], true);
    assert_eq!(execution["allOriginalChildrenWaited"], true);
    assert_eq!(execution["rendered"], false);
    assert_eq!(execution["periodicDamageExercised"], false);
    assert_eq!(execution["presentationQualified"], false);
    let plan = special_json(&root, "plan.json")?;
    assert_eq!(plan["loadout"]["tier"], SPECIAL_WEAPON_TIER);
    assert_eq!(plan["loadout"]["armourTier"], SPECIAL_ARMOUR_TIER);
    assert_eq!(plan["loadout"]["armour"], "tank");
    assert_eq!(plan["loadout"]["blocked"], serde_json::json!([]));
    assert_eq!(plan["special"]["rule"]["weapon"], obj::ZAROS_GODSWORD.id());
    assert_eq!(plan["special"]["activationOrder"], "self-before-Attack");
    let initial = special_json(&root, "initial-fixture.json")?;
    assert_eq!(initial["fixtureOnly"], true);
    ensure!(
        initial["account"].get("resources").is_none(),
        "no initial resource override"
    );
    ensure!(
        initial["account"]["skills"]
            .as_array()
            .context("native initial skills")?
            .iter()
            .all(|row| row["level"] == SPECIAL_INITIAL_LEVEL),
        "native99 pre-login fixture"
    );
    let journal = special_rows(&root, "driver-journal.jsonl")?;
    let combat = special_rows(&root, "combat-receipts.jsonl")?;
    let backend = special_json(&root, "backend-inputs.json")?;
    assert_eq!(backend["closure"]["closed"], true);
    assert_eq!(backend["closure"]["coreClosed"], true);
    assert_eq!(backend["closure"]["errors"], serde_json::json!([]));
    assert_eq!(
        backend["closure"]["socketPromises"]["pending"],
        serde_json::json!([])
    );
    let outside = special_stage(&journal, "outside_counts")?;
    assert_eq!(outside["liveCountWrites"], false);
    assert_eq!(
        outside["initialSavedVarps"],
        plan["gwd"]["initialSavedVarps"]
    );
    for count in plan["gwd"]["counts"]
        .as_array()
        .context("native count fields")?
    {
        let native: Vec<_> = outside["native"]["varbits"]
            .as_array()
            .context("native varbit readback")?
            .iter()
            .filter(|row| row["id"] == count["varbit"])
            .collect();
        ensure!(native.len() == SPECIAL_ONE, "native count denominator");
        assert_eq!(native[SPECIAL_FIRST]["value"], SPECIAL_ZERO);
    }
    let repair = special_stage(&journal, "durable_repair")?;
    let commits: Vec<_> = combat
        .iter()
        .filter(|row| row["kind"] == "repair_commit")
        .collect();
    ensure!(
        commits.len() == SPECIAL_ONE,
        "one guarded repair transaction"
    );
    let commit = commits[SPECIAL_FIRST];
    assert_eq!(repair["commit"], *commit);
    assert_eq!(commit["method"], "prepareCoinRepairBatch");
    assert_eq!(
        special_integer(&commit["before"], "coins")? - special_integer(&commit["after"], "coins")?,
        special_integer(commit, "price")?
    );
    assert_eq!(commit["after"]["coins"], commit["saved"]["coins"]);
    let key = &plan["repair"]["key"];
    let original = special_instance(&initial["account"]["backpack"], key)?;
    let repaired = special_instance(&commit["after"]["backpack"], key)?;
    let saved = special_instance(&commit["saved"]["backpack"], key)?;
    assert_eq!(
        original[SPECIAL_INSTANCE_INDEX]["charges"],
        plan["repair"]["initialCharges"]
    );
    assert_eq!(
        original[SPECIAL_FIRST],
        plan["repair"]["family"]["usedItem"]
    );
    assert_eq!(repaired, saved);
    assert_eq!(
        repaired[SPECIAL_FIRST],
        plan["repair"]["family"]["coinRepair"]["output"]
    );
    assert_eq!(
        repaired[SPECIAL_INSTANCE_INDEX]["charges"],
        plan["repair"]["family"]["capacity"]
    );
    let equipped = special_stage(&journal, "equipped")?;
    for item in plan["equipment"]
        .as_array()
        .context("native endgame equipment")?
    {
        ensure!(
            equipped["passive"]["worn"]
                .as_array()
                .context("ordinary worn slots")?
                .iter()
                .any(|slot| slot[SPECIAL_FIRST] == *item),
            "actual native endgame Wield/Wear"
        );
    }
    let activated = special_stage(&journal, "special_activated")?;
    let launch = &activated["launch"];
    let install = &activated["install"];
    assert_eq!(launch["outcome"], "launched");
    assert_eq!(activated["nativeEnergy"], launch["after"]["fine"]);
    assert_eq!(
        special_integer(&launch["before"], "fine")? - special_integer(&launch["after"], "fine")?,
        special_integer(&plan["special"]["rule"], "costFine")?
    );
    assert_eq!(install["accepted"], true);
    assert_eq!(install["primary"], Value::Null);
    assert_eq!(install["outcomes"], serde_json::json!([]));
    assert_eq!(
        install["durationTicks"],
        plan["special"]["field"]["durationTicks"]
    );
    let retired = special_stage(&journal, "field_retired")?;
    assert_eq!(retired["receipt"]["cancelled"], false);
    assert_eq!(
        special_integer(&retired["receipt"], "tick")?,
        special_integer(install, "tick")? + special_integer(install, "durationTicks")?
    );
    let sample = special_stage(&journal, "ordinary_sample")?;
    assert_eq!(
        sample["fullLife"]["hitpoints"],
        plan["target"]["profile"]["hitpoints"]
    );
    assert_eq!(sample["fullLife"]["definition"], plan["target"]["npc"]);
    let target_index = special_integer(&sample["fullLife"], "id")?;
    let hits = sample["landed"]
        .as_array()
        .context("ordinary impact receipts")?;
    let damage = hits.iter().try_fold(SPECIAL_ZERO, |sum, row| {
        Ok::<_, anyhow::Error>(sum + special_integer(row, "actualDamage")?)
    })?;
    assert_eq!(damage, special_integer(&sample["fullLife"], "hitpoints")?);
    ensure!(damage > SPECIAL_ZERO, "ordinary positive actual damage");
    for hit in hits {
        assert_eq!(hit["target"]["id"], target_index);
        assert_eq!(
            hit["target"]["generation"],
            sample["fullLife"]["generation"]
        );
        ensure!(
            special_integer(hit, "tick")? >= special_integer(install, "tick")?
                && special_integer(hit, "tick")? < special_integer(&retired["receipt"], "tick")?,
            "impact within real field lifetime"
        );
        ensure!(
            combat.iter().any(|row| row["kind"] == "launch"
                && row["target"]["id"] == hit["target"]["id"]
                && row["target"]["generation"] == hit["target"]["generation"]
                && row["damage"] == hit["launchedDamage"]
                && row["source"]["id"] == hit["source"]["id"]),
            "ordinary queued launch owns each impact"
        );
        ensure!(
            combat.iter().any(|row| row == hit),
            "unaltered passive landed receipt"
        );
    }
    let damages: BTreeSet<_> = hits
        .iter()
        .map(|hit| special_integer(hit, "nativeDamage"))
        .collect::<anyhow::Result<_>>()?;
    let complete = special_stage(&journal, "complete")?;
    assert_eq!(complete["status"], "ordinary_special_repair_pass");
    let trace = Trace::load(&root.join("session.rtr"))?;
    assert_eq!(
        trace.head(OBSERVED_BACKEND_HEAD)?,
        AUTHENTICATED_HEADLESS_BACKEND
    );
    assert_eq!(
        execution["wholeRecords"].as_u64(),
        Some(u64::try_from(trace.records.len())?)
    );
    assert_eq!(
        execution["wholeBytes"].as_u64(),
        Some(std::fs::metadata(root.join("session.rtr"))?.len())
    );
    ensure!(
        !trace.head.iter().any(|(name, _)| name == "server_command"),
        "no developer commands"
    );
    let inputs: Vec<_> = trace
        .records
        .iter()
        .filter(|row| row.tag == *RECORD_TAG)
        .collect();
    let applied = backend["inputs"]
        .as_array()
        .context("actual applied native inputs")?;
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
    assert_eq!(
        complete["actions"].as_u64(),
        Some(u64::try_from(inputs.len())?)
    );
    for ((record, receipt), control) in inputs.iter().zip(applied).zip(accepted) {
        assert_eq!(
            record.cycle,
            i32::try_from(special_integer(receipt, "cycle")?)?
        );
        assert_eq!(
            record.bytes,
            serde_json::from_value::<Vec<u8>>(receipt["event"].clone())?
        );
        assert_eq!(control["response"]["cycle"], record.cycle);
        ensure!(
            special_input(
                &control["request"]["action"],
                &control["request"]["map"],
                &InputEvent::decode(&record.bytes)?
            ),
            "exact accepted native input"
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
                .entry(i32::try_from(special_integer(&row["response"], "cycle")?)?)
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
        (SPECIAL_FIRST, SPECIAL_FIRST, SPECIAL_FIRST, SPECIAL_FIRST);
    let mut native_impacts = BTreeSet::new();
    for cycle in SPECIAL_FIRST_CYCLE..=trace.last_cycle() {
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
        let state = observe(replay.game());
        for frame in &frames[done..next] {
            if frame.opcode == sp::PLAYER_INFO {
                let expected = players
                    .get(player_updates)
                    .context("ordered native PLAYER publication")?;
                assert_eq!(
                    state.local.as_ref().context("local player")?.tile,
                    expected[..SPECIAL_TILE_FIELDS]
                );
                player_updates += SPECIAL_ONE;
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
                npc_updates += SPECIAL_ONE;
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
            checked_snapshots += SPECIAL_ONE;
        }
        if let Some(enemy) = replay
            .game()
            .runtime
            .feed
            .state
            .npcs
            .entities
            .get(&(usize::try_from(target_index)?))
        {
            if i64::from(enemy.type_id) == special_integer(&plan["target"], "npc")? {
                if let Some(combat) = &enemy.path.combat {
                    for hit in &combat.hits {
                        let damage = i64::from(hit[SPECIAL_HIT_DAMAGE]);
                        if damages.contains(&damage) {
                            native_impacts.insert(damage);
                        }
                    }
                }
            }
        }
    }
    assert_eq!(done, frames.len());
    assert_eq!(player_updates, players.len());
    assert_eq!(npc_updates, npcs.len());
    assert_eq!(checked_snapshots, wanted_snapshots);
    ensure!(
        checked_snapshots > SPECIAL_FIRST
            && player_updates > SPECIAL_FIRST
            && npc_updates > SPECIAL_FIRST,
        "whole native UI, variables and rosters"
    );
    assert_eq!(
        native_impacts, damages,
        "all actual native hitsplats appear in decoded client state"
    );
    Ok(())
}
