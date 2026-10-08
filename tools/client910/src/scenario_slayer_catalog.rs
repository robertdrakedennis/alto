//! Ordinary full-life Slayer credit, native assignment input and durable reward exchange.
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Replay, Trace,
    AUTHENTICATED_HEADLESS_BACKEND, OBSERVED_BACKEND_HEAD,
};
use crate::client_core::input_event::{InputEvent, RECORD_TAG};
use anyhow::{ensure, Context};
use rs910_symbols::{component, varbit, varp};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const SLAYER_REPLAY_FIXTURE: &str = "fixtures/session-replay/slayer-catalog";
const SLAYER_REPLAY_FIRST_CYCLE: i32 = 1;
const SLAYER_REPLAY_INITIAL_CYCLE: i32 = 0;
const SLAYER_REPLAY_FIRST: usize = 0;
const SLAYER_REPLAY_ONE: usize = 1;
const SLAYER_REPLAY_SLOT_QUANTITY: usize = 1;
const SLAYER_REPLAY_TILE_FIELDS: usize = 3;
const SLAYER_REPLAY_COMPONENT_OFFSET: usize = 4;
const SLAYER_REPLAY_COMPONENT_END: usize = 8;
const SLAYER_REPLAY_LOWER_TIER: i64 = 90;
const SLAYER_REPLAY_SLAYER_SKILL: usize = 18;
const SLAYER_REPLAY_HIT_DAMAGE: usize = 1;

const SLAYER_REPLAY_LOCATION_ID_HIGH_HALF_BITS: u32 = 32;

fn slayer_fixture_json(root: &Path, name: &str) -> anyhow::Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(root.join(name))?)?)
}

fn slayer_fixture_rows(root: &Path, name: &str) -> anyhow::Result<Vec<Value>> {
    let text = std::fs::read_to_string(root.join(name))?;
    ensure!(text.ends_with('\n'), "complete evidence rows");
    Ok(text
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?)
}

fn slayer_inventory_count(slots: &Value, item: &Value) -> anyhow::Result<i64> {
    slots
        .as_array()
        .context("recorded inventory")?
        .iter()
        .filter(|slot| slot[SLAYER_REPLAY_FIRST] == *item)
        .map(|slot| {
            slot[SLAYER_REPLAY_SLOT_QUANTITY]
                .as_i64()
                .context("recorded quantity")
        })
        .sum()
}

fn slayer_fixture_integer(value: &Value, field: &str) -> anyhow::Result<i64> {
    value[field]
        .as_i64()
        .with_context(|| format!("{field}: {value}"))
}

fn slayer_native_input_matches(action: &Value, map: &Value, event: &InputEvent) -> bool {
    match (action["kind"].as_str(), event) {
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
        (Some("loc"), InputEvent::Menu { choice, .. }) => {
            choice.enabled
                && action["definition"].as_i64()
                    == Some(
                        (choice.entity_id >> SLAYER_REPLAY_LOCATION_ID_HIGH_HALF_BITS)
                            & i64::from(i32::MAX),
                    )
                && action["operation"]
                    .as_str()
                    .is_some_and(|label| choice.op.eq_ignore_ascii_case(label))
                && action["x"]
                    .as_i64()
                    .zip(map["base_x"].as_i64())
                    .is_some_and(|(x, base)| i64::from(choice.tile_x) == x - base)
                && action["z"]
                    .as_i64()
                    .zip(map["base_z"].as_i64())
                    .is_some_and(|(z, base)| i64::from(choice.tile_z) == z - base)
        }
        (Some("npc"), InputEvent::Menu { choice, .. }) => {
            choice.enabled
                && action["index"].as_i64() == Some(choice.entity_id)
                && action["operation"]
                    .as_str()
                    .is_some_and(|label| choice.op.eq_ignore_ascii_case(label))
        }
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
        _ => false,
    }
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_slayer_catalog_ordinary_kill_task_and_purchase() -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let root = rs910_core::test_support::client_dir().join(SLAYER_REPLAY_FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    assert_eq!(
        trace.head(OBSERVED_BACKEND_HEAD)?,
        AUTHENTICATED_HEADLESS_BACKEND
    );
    let execution = slayer_fixture_json(&root, "execution.json")?;
    assert_eq!(execution["status"], "completed_ordinary_slayer_catalog");
    assert_eq!(execution["captureOwnerPassed"], true);
    assert_eq!(execution["allOriginalChildrenWaited"], true);
    assert_eq!(execution["rendered"], false);
    let plan = slayer_fixture_json(&root, "plan.json")?;
    let initial = slayer_fixture_json(&root, "initial-fixture.json")?;
    let saved = slayer_fixture_json(&root, "final-saved-state.json")?;
    let receipts = slayer_fixture_rows(&root, "combat-receipts.jsonl")?;
    let journal = slayer_fixture_rows(&root, "driver-journal.jsonl")?;
    let backend = slayer_fixture_json(&root, "backend-inputs.json")?;
    assert_eq!(backend["closure"]["closed"], true);
    assert_eq!(backend["closure"]["coreClosed"], true);
    assert_eq!(backend["closure"]["errors"], serde_json::json!([]));
    assert_eq!(
        backend["closure"]["socketPromises"]["pending"],
        serde_json::json!([])
    );
    let input_records: Vec<_> = trace
        .records
        .iter()
        .filter(|row| row.tag == *RECORD_TAG)
        .collect();
    let backend_inputs = backend["inputs"].as_array().context("applied inputs")?;
    let actions: Vec<_> = journal
        .iter()
        .filter(|row| {
            row["kind"] == "control"
                && row["request"]["command"] == "action"
                && row["response"]["status"] == "accepted"
        })
        .collect();
    assert_eq!(input_records.len(), backend_inputs.len());
    assert_eq!(input_records.len(), actions.len());
    for ((record, applied), accepted) in input_records.iter().zip(backend_inputs).zip(&actions) {
        assert_eq!(
            record.cycle,
            i32::try_from(slayer_fixture_integer(applied, "cycle")?)?
        );
        assert_eq!(
            record.bytes,
            serde_json::from_value::<Vec<u8>>(applied["event"].clone())?
        );
        assert_eq!(accepted["response"]["cycle"], record.cycle);
        ensure!(
            slayer_native_input_matches(
                &accepted["request"]["action"],
                &accepted["request"]["map"],
                &InputEvent::decode(&record.bytes)?
            ),
            "normal accepted native input"
        );
    }
    let complete = journal
        .iter()
        .find(|row| row["kind"] == "complete")
        .context("actual completed native route")?;
    assert_eq!(complete["status"], "ordinary_slayer_pass");
    assert_eq!(
        slayer_fixture_integer(complete, "actions")?,
        i64::try_from(input_records.len())?
    );
    let credited = journal
        .iter()
        .find(|row| row["kind"] == "slayer_completed")
        .context("actual ordinary kill credit")?;
    let full = &credited["fullHealth"];
    ensure!(
        receipts.iter().any(|row| row["kind"] == "state"
            && row["enemies"]
                .as_array()
                .is_some_and(|enemies| enemies.iter().any(|enemy| enemy == full))),
        "unmodified full-life passive publication"
    );
    assert_eq!(full["hitpoints"], plan["target"]["profile"]["hitpoints"]);
    assert_eq!(full["hitpoints"], full["configuredMaximumLife"]);
    assert_eq!(full["definition"], plan["target"]["npc"]);
    let died = receipts
        .iter()
        .find(|row| {
            row["kind"] == "death"
                && row["target"]["id"] == full["id"]
                && row["target"]["generation"] == full["generation"]
                && row["source"]["id"] == credited["passive"]["pid"]
        })
        .context("same ordinary full-health life killed by the credited player")?;
    ensure!(
        receipts.iter().any(|row| row["kind"] == "landed"
            && row["target"]["id"] == died["target"]["id"]
            && row["target"]["generation"] == died["target"]["generation"]
            && row["source"]["id"] == died["source"]["id"]
            && row["style"] == "melee"
            && row["actualDamage"]
                .as_i64()
                .is_some_and(|damage| damage > SLAYER_REPLAY_INITIAL_CYCLE.into())),
        "positive ordinary melee impact"
    );
    let impacts: Vec<_> = receipts
        .iter()
        .filter(|row| {
            row["kind"] == "landed"
                && row["target"]["id"] == full["id"]
                && row["target"]["generation"] == full["generation"]
                && row["source"]["id"] == died["source"]["id"]
        })
        .collect();
    let actual_damage = impacts
        .iter()
        .try_fold(i64::from(SLAYER_REPLAY_INITIAL_CYCLE), |total, row| {
            Ok::<_, anyhow::Error>(total + slayer_fixture_integer(row, "actualDamage")?)
        })?;
    assert_eq!(actual_damage, slayer_fixture_integer(full, "hitpoints")?);
    let mut expected_hits: BTreeSet<_> = impacts
        .iter()
        .map(|row| slayer_fixture_integer(row, "nativeDamage"))
        .collect::<anyhow::Result<_>>()?;
    expected_hits.retain(|damage| *damage > i64::from(SLAYER_REPLAY_INITIAL_CYCLE));
    ensure!(
        !expected_hits.is_empty(),
        "actual positive native target impacts"
    );
    for item in plan["equipmentFacts"]
        .as_array()
        .context("qualified full loadout")?
    {
        ensure!(
            item["tier"].as_i64().context("qualified item tier")? >= SLAYER_REPLAY_LOWER_TIER,
            "end-game tier"
        );
        ensure!(
            item["forms"]
                .as_array()
                .context("physical forms")?
                .iter()
                .any(
                    |identity| slayer_inventory_count(&credited["passive"]["worn"], identity)
                        .is_ok_and(|quantity| quantity > SLAYER_REPLAY_INITIAL_CYCLE.into())
                ),
            "every qualified loadout item worn through ordinary input"
        );
    }
    assert!(credited["passive"]["savedSlayer"]["assignment"].is_null());
    assert_eq!(
        credited["passive"]["savedSlayer"]["points"].as_i64(),
        Some(
            plan["initialPoints"].as_i64().context("initial points")?
                + plan["expectedCompletionPoints"]
                    .as_i64()
                    .context("dated completion award")?
        )
    );
    ensure!(
        credited["passive"]["slayerXp"]
            .as_i64()
            .context("credited Slayer XP")?
            > initial["account"]["skills"][SLAYER_REPLAY_SLAYER_SKILL]["xp"]
                .as_i64()
                .context("initial Slayer XP")?,
        "ordinary Slayer experience awarded"
    );
    assert_eq!(saved["slayer"], complete["passive"]["savedSlayer"]);
    assert_eq!(
        saved["slayer"]["assignment"]["master"],
        plan["master"]["code"]
    );
    assert_eq!(
        saved["slayer"]["points"].as_i64(),
        Some(
            plan["initialPoints"].as_i64().context("initial points")?
                + plan["expectedCompletionPoints"]
                    .as_i64()
                    .context("completion award")?
                - plan["reward"]["cost"]
                    .as_i64()
                    .context("dated reward price")?
        )
    );
    assert_eq!(
        slayer_inventory_count(
            &saved["backpack"],
            &plan["reward"]["items"][SLAYER_REPLAY_FIRST]["item"]
        )? - slayer_inventory_count(
            &initial["account"]["backpack"],
            &plan["reward"]["items"][SLAYER_REPLAY_FIRST]["item"]
        )?,
        plan["reward"]["items"][SLAYER_REPLAY_FIRST]["count"]
            .as_i64()
            .context("native purchase quantity")?
    );
    let checkpoints: Vec<_> = journal
        .iter()
        .filter(|row| {
            matches!(
                row["kind"].as_str(),
                Some("slayer_completed" | "task_assigned" | "complete")
            )
        })
        .collect();
    for kind in ["slayer_completed", "task_assigned", "complete"] {
        ensure!(
            checkpoints.iter().filter(|row| row["kind"] == kind).count() == SLAYER_REPLAY_ONE,
            "one actual {kind} checkpoint"
        );
    }
    for checkpoint in &checkpoints {
        let cycle = checkpoint["state"]["cycle"]
            .as_i64()
            .context("native checkpoint cycle")?;
        ensure!(
            (i64::from(SLAYER_REPLAY_FIRST_CYCLE)..=i64::from(trace.last_cycle())).contains(&cycle),
            "every native checkpoint belongs to the whole recorded trace"
        );
    }
    let frames = arrivals(&trace)?;
    let (players, npcs) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let snapshots: BTreeMap<i32, Vec<&Value>> = journal
        .iter()
        .filter(|row| {
            row["kind"] == "control"
                && row["request"]["command"] == "snapshot"
                && row["response"]["data"]["ready"] == true
        })
        .try_fold(BTreeMap::<i32, Vec<&Value>>::new(), |mut grouped, row| {
            grouped
                .entry(i32::try_from(slayer_fixture_integer(
                    &row["response"],
                    "cycle",
                )?)?)
                .or_default()
                .push(row);
            Ok::<_, anyhow::Error>(grouped)
        })?;
    let required_snapshots: usize = snapshots.values().map(Vec::len).sum();
    let task_operations = [cp::OPNPC1, cp::OPNPC2, cp::OPNPC3, cp::OPNPC4, cp::OPNPC5];
    let task_operation = *task_operations
        .get(usize::try_from(
            slayer_fixture_integer(&plan["master"], "taskOperation")?
                - i64::try_from(SLAYER_REPLAY_ONE)?,
        )?)
        .context("native Get-task operation")?;
    let mut replay = Replay::start(&trace)?;
    assert_eq!(
        replay.io.take_written(),
        trace.bytes(b"OUT ", SLAYER_REPLAY_INITIAL_CYCLE)
    );
    let mut done = SLAYER_REPLAY_FIRST;
    let mut player_updates = SLAYER_REPLAY_FIRST;
    let mut npc_updates = SLAYER_REPLAY_FIRST;
    let mut task_request = false;
    let mut purchase = false;
    let mut observed_checkpoints = SLAYER_REPLAY_FIRST;
    let mut checked_snapshots = SLAYER_REPLAY_FIRST;
    let mut observed_hits = BTreeSet::new();
    for cycle in SLAYER_REPLAY_FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        assert_eq!(
            mask_wall_clock(&output.written)?,
            mask_wall_clock(&trace.bytes(b"OUT ", cycle))?,
            "cycle{cycle}: whole client packet order and bytes"
        );
        for (opcode, payload) in client_frames(&output.written)? {
            assert_ne!(opcode, cp::CLIENT_CHEAT, "ordinary input only");
            task_request |= opcode == task_operation;
            if opcode == cp::IF_BUTTON1 && payload.len() >= SLAYER_REPLAY_COMPONENT_END {
                let packed = i32::from_be_bytes(
                    payload[SLAYER_REPLAY_COMPONENT_OFFSET..SLAYER_REPLAY_COMPONENT_END]
                        .try_into()?,
                );
                purchase |= packed == component::slayer_rewards::BROAD_BOLTS_BUY.packed();
            }
        }
        let next = replay.processed(&frames);
        let fresh = &frames[done..next];
        ensure!(
            fresh
                .iter()
                .filter(|frame| frame.opcode == sp::PLAYER_INFO)
                .count()
                <= SLAYER_REPLAY_ONE
                && fresh
                    .iter()
                    .filter(|frame| frame.opcode == sp::NPC_INFO)
                    .count()
                    <= SLAYER_REPLAY_ONE,
            "every applied actor tick remains independently observable"
        );
        let observed = observe(replay.game());
        for frame in fresh {
            if frame.opcode == sp::PLAYER_INFO {
                assert_eq!(
                    observed.local.as_ref().context("native local player")?.tile,
                    players
                        .get(player_updates)
                        .context("independent player row")?[..SLAYER_REPLAY_TILE_FIELDS]
                );
                player_updates += SLAYER_REPLAY_ONE;
            }
            if frame.opcode == sp::NPC_INFO {
                let expected: BTreeMap<_, _> = npcs
                    .get(npc_updates)
                    .context("independent NPC row")?
                    .iter()
                    .map(|&[index, definition, x, z, level]| {
                        (index as usize, (definition, [x, z, level]))
                    })
                    .collect();
                let actual: BTreeMap<_, _> = observed
                    .npcs
                    .iter()
                    .map(|(&index, (definition, actor))| (index, (*definition, actor.tile)))
                    .collect();
                assert_eq!(actual, expected);
                npc_updates += SLAYER_REPLAY_ONE;
            }
        }
        done = next;
        for row in snapshots.get(&cycle).into_iter().flatten() {
            let recorded = &row["response"]["data"];
            let session = replay.core.session.as_ref().context("ordinary session")?;
            let query = serde_json::from_value(row["request"]["query"].clone())?;
            let actual = crate::live_control::observed_snapshot_for_test(session, cycle, query)?;
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
                assert_eq!(actual[key], recorded[key], "cycle{cycle}: native {key}");
            }
            for key in [
                "base_x",
                "base_z",
                "level",
                "width",
                "height",
                "terrain_generation",
            ] {
                assert_eq!(actual["map"][key], recorded["map"][key]);
            }
            checked_snapshots += SLAYER_REPLAY_ONE;
        }
        if let Some(enemy) = replay
            .game()
            .runtime
            .feed
            .state
            .npcs
            .entities
            .get(&usize::try_from(slayer_fixture_integer(full, "id")?)?)
        {
            if let Some(combat) = &enemy.path.combat {
                for hit in &combat.hits {
                    let damage = i64::from(hit[SLAYER_REPLAY_HIT_DAMAGE]);
                    if expected_hits.contains(&damage) {
                        observed_hits.insert(damage);
                    }
                }
            }
        }
        for checkpoint in checkpoints
            .iter()
            .filter(|row| row["state"]["cycle"].as_i64() == Some(cycle.into()))
        {
            observed_checkpoints += SLAYER_REPLAY_ONE;
            let vars = replay
                .game()
                .runtime
                .feed
                .state
                .varps
                .as_ref()
                .context("native Slayer variables")?;
            let remaining = checkpoint["passive"]["slayer"]["assignment"]["remaining"]
                .as_i64()
                .unwrap_or(SLAYER_REPLAY_INITIAL_CYCLE.into());
            assert_eq!(
                i64::from(
                    vars.get(varp::SLAYER_TASK_COUNT.id())
                        .map_err(|error| anyhow::anyhow!("{error:?}"))?
                ),
                remaining
            );
            assert_eq!(
                replay
                    .game()
                    .varbit_value(u16::try_from(varbit::LEGACY_COMBAT_ACTIVE.id())?)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?,
                SLAYER_REPLAY_FIRST_CYCLE
            );
        }
    }
    ensure!(
        task_request && purchase,
        "native task request and Buy packets"
    );
    assert_eq!(done, frames.len());
    assert_eq!(observed_checkpoints, checkpoints.len());
    assert_eq!(checked_snapshots, required_snapshots);
    ensure!(
        checked_snapshots > SLAYER_REPLAY_FIRST,
        "actual native snapshots"
    );
    assert_eq!(
        observed_hits, expected_hits,
        "all actual positive native target impacts"
    );
    assert_eq!(player_updates, players.len());
    assert_eq!(npc_updates, npcs.len());
    Ok(())
}
