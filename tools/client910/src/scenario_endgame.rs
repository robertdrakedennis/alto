//! Recorded native T92 melee equipment, statistics and ordinary training hits.
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Replay, Trace,
    AUTHENTICATED_HEADLESS_BACKEND, INITIAL_RECORDING_OUTPUT_CYCLE, OBSERVED_BACKEND_HEAD,
};
use crate::client_core::input_event::{InputEvent, RECORD_TAG};
use anyhow::{ensure, Context};
use rs910_symbols::{npc, obj};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const FIXTURE: &str = "fixtures/session-replay/endgame-gear";
const FIRST_CYCLE: i32 = 1;
const FIRST: usize = 0;
const ONE: usize = 1;
const ZERO: i64 = 0;
const REQUIRED_IMPACTS: usize = 3;
const HIT_DAMAGE: usize = 1;
const TILE_FIELDS: usize = 3;
const HIGHER_TIER: i64 = 92;
const ARMOUR_TIER: i64 = 90;
const ABORTED_CORE_EXIT: i64 = 101; // not a content id

fn json(root: &Path, name: &str) -> anyhow::Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(root.join(name))?)?)
}
fn rows(root: &Path, name: &str) -> anyhow::Result<Vec<Value>> {
    let text = std::fs::read_to_string(root.join(name))?;
    ensure!(text.ends_with('\n'), "complete evidence rows");
    Ok(text
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?)
}
fn integer(value: &Value, field: &str) -> anyhow::Result<i64> {
    value[field]
        .as_i64()
        .with_context(|| format!("{field}: {value}"))
}
fn same_input(action: &Value, event: &InputEvent) -> bool {
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
                && action["operation"].as_str() == Some(choice.op.as_str())
        }
        _ => false,
    }
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_endgame_melee_equipment_stats_and_ordinary_impacts() -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let execution = json(&root, "execution.json")?;
    assert_eq!(execution["status"], "completed_ordinary_endgame_training");
    assert_eq!(execution["rendered"], false);
    assert_eq!(execution["originalCaptureOwnerPassed"], false);
    assert_eq!(execution["originalCoreExit"], ABORTED_CORE_EXIT);
    assert_eq!(execution["allOriginalChildrenWaited"], true);
    let plan = json(&root, "plan.json")?;
    assert_eq!(plan["loadout"]["tier"], HIGHER_TIER);
    assert_eq!(plan["loadout"]["armourTier"], ARMOUR_TIER);
    assert_eq!(plan["loadout"]["hands"], "two-handed");
    assert_eq!(plan["equipment"][FIRST], obj::ZAROS_GODSWORD.id());
    let initial = json(&root, "initial-fixture.json")?;
    let target = &initial["trainingTarget"];
    assert_eq!(target["npc"], npc::GOBLIN_LEVEL_2.id());
    assert_eq!(initial["fixtureOnly"], true);
    let combat = rows(&root, "combat-receipts.jsonl")?;
    let journal = rows(&root, "driver-journal.jsonl")?;
    let saved = json(&root, "final-saved-state.json")?;
    let backend = json(&root, "backend-inputs.json")?;
    assert_eq!(backend["closure"]["closed"], true);
    assert_eq!(backend["closure"]["errors"], serde_json::json!([]));
    assert_eq!(
        backend["closure"]["socketPromises"]["pending"],
        serde_json::json!([])
    );
    let completed: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == "complete")
        .collect();
    assert_eq!(completed.len(), ONE);
    let complete = completed[FIRST];
    assert_eq!(complete["status"], "ordinary_endgame_training_pass");
    assert_eq!(complete["passive"]["worn"], saved["worn"]);
    assert_eq!(complete["passive"]["life"], saved["resources"]["life"]);
    assert_eq!(
        complete["passive"]["maximumLife"],
        plan["expected"]["maxLife"]
    );
    let impacts: Vec<_> = combat
        .iter()
        .filter(|row| row["kind"] == "landed")
        .collect();
    let launches: Vec<_> = combat
        .iter()
        .filter(|row| row["kind"] == "launch")
        .collect();
    ensure!(
        impacts.len() >= REQUIRED_IMPACTS && impacts.len() == launches.len(),
        "real ordinary impacts"
    );
    let maximum = integer(&plan["expected"], "maximumLegacyHit")?;
    let expected_hits: BTreeSet<_> = impacts
        .iter()
        .map(|hit| integer(hit, "actualDamage"))
        .collect::<anyhow::Result<_>>()?;
    for (hit, launch) in impacts.iter().zip(&launches) {
        let damage = integer(hit, "actualDamage")?;
        ensure!(
            damage > ZERO && damage <= maximum,
            "independent Legacy damage bound"
        );
        assert_eq!(hit["target"]["id"], target["nid"]);
        assert_eq!(hit["target"]["generation"], complete["life"]["generation"]);
        assert_eq!(hit["target"]["definition"], target["npc"]);
        assert_eq!(hit["actualDamage"], launch["damage"]);
        assert_eq!(launch["hand"], "main");
    }
    let trace = Trace::load(&root.join("session.rtr"))?;
    assert_eq!(
        trace.head(OBSERVED_BACKEND_HEAD)?,
        AUTHENTICATED_HEADLESS_BACKEND
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
    for ((record, applied), accepted) in input_records.iter().zip(backend_inputs).zip(actions) {
        assert_eq!(record.cycle, integer(applied, "cycle")? as i32);
        assert_eq!(
            record.bytes,
            serde_json::from_value::<Vec<u8>>(applied["event"].clone())?
        );
        assert_eq!(accepted["response"]["cycle"], record.cycle);
        ensure!(
            same_input(
                &accepted["request"]["action"],
                &InputEvent::decode(&record.bytes)?
            ),
            "normal accepted native input"
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
                .entry(i32::try_from(integer(&row["response"], "cycle")?)?)
                .or_default()
                .push(row);
            Ok::<_, anyhow::Error>(grouped)
        })?;
    let required_snapshots: usize = snapshots.values().map(Vec::len).sum();
    let mut replay = Replay::start(&trace)?;
    assert_eq!(
        replay.io.take_written(),
        trace.bytes(b"OUT ", INITIAL_RECORDING_OUTPUT_CYCLE)
    );
    let (mut done, mut player_updates, mut npc_updates, mut checked_snapshots) =
        (FIRST, FIRST, FIRST, FIRST);
    let mut observed_hits = BTreeSet::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
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
                "no developer commands"
            );
        }
        let next = replay.processed(&frames);
        let fresh = &frames[done..next];
        let observed = observe(replay.game());
        for frame in fresh {
            if frame.opcode == sp::PLAYER_INFO {
                let expected = players
                    .get(player_updates)
                    .context("ordered PLAYER publication")?;
                assert_eq!(
                    observed.local.as_ref().context("native player")?.tile,
                    expected[..TILE_FIELDS]
                );
                player_updates += ONE;
            }
            if frame.opcode == sp::NPC_INFO {
                let expected: BTreeMap<_, _> = npcs
                    .get(npc_updates)
                    .context("ordered NPC publication")?
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
                assert_eq!(actual, expected, "all native roster publications");
                npc_updates += ONE;
            }
        }
        done = next;
        for row in snapshots.get(&cycle).into_iter().flatten() {
            let recorded = &row["response"]["data"];
            let session = replay.core.session.as_ref().context("session")?;
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
            checked_snapshots += ONE;
        }
        if let Some(enemy) = replay
            .game()
            .runtime
            .feed
            .state
            .npcs
            .entities
            .get(&(integer(target, "nid")? as usize))
        {
            assert_eq!(enemy.type_id, npc::GOBLIN_LEVEL_2.id());
            if let Some(combat) = &enemy.path.combat {
                for hit in &combat.hits {
                    let damage = i64::from(hit[HIT_DAMAGE]);
                    if expected_hits.contains(&damage) {
                        observed_hits.insert(damage);
                    }
                }
            }
        }
    }
    assert_eq!(done, frames.len());
    assert_eq!(player_updates, players.len());
    assert_eq!(npc_updates, npcs.len());
    ensure!(
        player_updates > FIRST && npc_updates > FIRST,
        "real native publications"
    );
    assert_eq!(checked_snapshots, required_snapshots);
    ensure!(checked_snapshots > FIRST, "native equipment/stat snapshots");
    assert_eq!(
        observed_hits, expected_hits,
        "all actual positive damage appears in native NPC state"
    );
    Ok(())
}
