//! Ordinary Corp recording through the production owners and independent event queues.
use super::scenario_boss_encounters::{
    boss_integer_field, boss_route_stage, read_boss_json, read_boss_rows,
};
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

const CORP_FIXTURE: &str = "fixtures/session-replay/boss-encounters/corporeal-beast-melee-sample";
const CORP_FIRST: usize = 0;
const CORP_ONE: usize = 1;
const CORP_ZERO: i64 = 0;
const CORP_FIRST_CYCLE: i32 = 1;
const CORP_FULL_BOSS_LIFE: i64 = 100000;
const CORP_FULL_CORE_LIFE: i64 = 3500;
const CORP_WEAPON_TIER: i64 = 92;
const CORP_ADMITTED_KIT_COUNT: usize = 13;
const CORP_ARMOUR_TIER: i64 = 90;
const CORP_INITIAL_LEVEL: i64 = 99;
const CORP_MAXIMUM_LAUNCHES: i64 = 4;
const CORP_TILE_FIELDS: usize = 3;
const CORP_HIT_DAMAGE: usize = 1;
const CORP_HIT_TYPE: usize = 0;
const CORP_HIT_EXPIRY: usize = 4;
const CORP_HEADBAR_START: usize = 1;
const CORP_HEADBAR_END: usize = 2;
const CORP_HEADBAR_DURATION: usize = 3;
const CORP_HEADBAR_FILL_MAX: i64 = 255;
const CORP_UNRECEIVED_CLOSING_LIMIT: usize = 1;
const CORP_HALF_NUMERATOR: i64 = 1;
const CORP_HALF_DENOMINATOR: i64 = 2;
const CORP_CORE_MAXIMUM_DAMAGE: i64 = 600;
const CORP_GAME_TICK_MILLISECONDS: i64 = 600;

fn corp_same_actor(left: &Value, right: &Value) -> bool {
    [
        "kind",
        "id",
        "generation",
        "definition",
        "instance",
        "actorToken",
    ]
    .iter()
    .all(|field| left[*field] == right[*field])
}
fn corp_fields<'a>(value: &'a Value, field: &str) -> anyhow::Result<&'a Vec<Value>> {
    value[field]
        .as_array()
        .with_context(|| format!("ordinary {field}"))
}
struct CorpQueueProof<'a> {
    published_life: i64,
    native_hits: &'a Vec<Value>,
}

fn corp_published_life(actor: &Value, tick: i64, combat: &[Value]) -> anyhow::Result<i64> {
    // A later natural regeneration/resource publication does not replace a
    // previously queued headbar. Retain the actual last health-producing event.
    for row in combat.iter().rev().filter(|row| row["tick"] == tick) {
        if row["kind"] == "landed" && corp_same_actor(&row["target"], actor) {
            return boss_integer_field(&row["target"], "currentLife");
        }
        if row["kind"] == "npc_healing" && corp_same_actor(&row["target"], actor) {
            return boss_integer_field(row, "afterLife");
        }
    }
    boss_integer_field(actor, "hitpoints")
}

fn corp_queues(
    actor: &Value,
    native: Option<&rs910_game::entities910::combat::Combat>,
    previous: &mut BTreeMap<(bool, usize), Vec<[i32; 5]>>,
    is_npc: bool,
    index: usize,
    proof: CorpQueueProof<'_>,
) -> anyhow::Result<()> {
    let expected_hits = corp_fields(actor, "hits")?;
    let expected_bars = corp_fields(actor, "headbars")?;
    let Some(native) = native else {
        ensure!(
            expected_hits.is_empty() && expected_bars.is_empty(),
            "published combat queue absent in native actor"
        );
        return Ok(());
    };
    let old = previous.get(&(is_npc, index));
    let mut arrived: Vec<(i64, i64)> = native
        .hits
        .iter()
        .enumerate()
        .filter(|(slot, hit)| {
            hit[CORP_HIT_EXPIRY] > CORP_ZERO as i32
                && old.and_then(|rows| rows.get(*slot)) != Some(*hit)
        })
        .map(|(_, hit)| {
            (
                i64::from(hit[CORP_HIT_TYPE]),
                i64::from(hit[CORP_HIT_DAMAGE]),
            )
        })
        .collect();
    ensure!(
        arrived.len() == expected_hits.len(),
        "every native hit slot change belongs to this ordinary publication"
    );
    ensure!(
        proof.native_hits.len() == expected_hits.len(),
        "pure encoder type denominator"
    );
    for (hit, encoded) in expected_hits.iter().zip(proof.native_hits) {
        assert_eq!(hit["damage"], encoded["damage"]);
        assert_eq!(hit["source"], encoded["source"]);
        let damage = boss_integer_field(hit, "damage")?;
        let native_type = boss_integer_field(encoded, "type")?;
        let slot = arrived
            .iter()
            .position(|value| *value == (native_type, damage))
            .context("published native hitsplat type/damage multiplicity")?;
        arrived.remove(slot);
    }
    let latest: BTreeMap<i64, &Value> = expected_bars
        .iter()
        .map(|bar| Ok((boss_integer_field(bar, "id")?, bar)))
        .collect::<anyhow::Result<_>>()?;
    for (id, expected) in latest {
        let bar = native
            .bars
            .iter()
            .find(|bar| i64::from(bar.id) == id)
            .context("published native bar type")?;
        ensure!(
            bar.updates
                .iter()
                .any(
                    |update| i64::from(update[CORP_HEADBAR_START]) == expected["start"]
                        && i64::from(update[CORP_HEADBAR_END]) == expected["end"]
                        && i64::from(update[CORP_HEADBAR_DURATION]) == expected["duration"]
                ),
            "published native HP headbar update"
        );
    }
    if let Some(expected) = expected_bars.last() {
        let life = proof.published_life;
        let maximum = boss_integer_field(actor, "maximumLife")?;
        ensure!(
            maximum > CORP_ZERO
                && expected["end"]
                    == life.clamp(CORP_ZERO, maximum) * CORP_HEADBAR_FILL_MAX / maximum,
            "normal headbar quantisation follows actual last health publication point"
        );
    }
    previous.insert((is_npc, index), native.hits.clone());
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_corporeal_beast_native_ordinary_melee_sample() -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let root = rs910_core::test_support::client_dir().join(CORP_FIXTURE);
    let hashes = read_boss_json(&root, "fixture-hashes.json")?;
    for (relative, expected) in hashes.as_object().context("whole frozen input hashes")? {
        ensure!(
            Path::new(relative).components().count() == CORP_ONE,
            "fixture hash escapes owner directory"
        );
        assert_eq!(
            native910::project::sha256(&std::fs::read(root.join(relative))?),
            expected.as_str().context("SHA256")?
        );
    }
    let execution = read_boss_json(&root, "execution.json")?;
    assert_eq!(execution["status"], "completed_ordinary_corp_melee_sample");
    assert_eq!(execution["captureOwnerPassed"], true);
    assert_eq!(execution["allOriginalChildrenWaited"], true);
    assert_eq!(execution["rendered"], false);
    assert_eq!(execution["wholeSha256"], hashes["session.rtr"]);
    let process = read_boss_json(&root, "process-closure.json")?;
    let result = &process["result"];
    for field in ["passed", "allDirectChildrenWaited", "sourceInputsUnchanged"] {
        assert_eq!(result[field], true);
    }
    assert_eq!(result["cleanupErrors"], serde_json::json!([]));
    assert_eq!(result["ownedSurvivors"], serde_json::json!([]));
    ensure!(
        boss_integer_field(result, "actualClientLaunchesThisRun")? == CORP_ONE as i64
            && boss_integer_field(result, "expectedPriorLaunches")? + CORP_ONE as i64
                <= CORP_MAXIMUM_LAUNCHES,
        "finite actual native launch ledger"
    );
    let processes = corp_fields(&process, "processes")?;
    ensure!(
        processes
            .iter()
            .all(|row| row["waited"] == true && row["exit"].as_i64().is_some()),
        "every original child reaped"
    );
    for name in ["core", "driver", "broker", "wired-guard"] {
        let matches: Vec<_> = processes
            .iter()
            .filter(|row| row["name"] == name && row["query"] != true)
            .collect();
        ensure!(
            matches.len() == CORP_ONE && matches[CORP_FIRST]["exit"] == CORP_ZERO,
            "one clean ordinary {name}"
        );
    }
    let build = read_boss_json(&root, "native-build.json")?;
    let epoch = read_boss_json(&root, "source-epoch.json")?;
    let adoption = read_boss_json(&root, "adoption.json")?;
    assert_eq!(build["sourceRoot"], adoption["root"]);
    assert_eq!(adoption["status"], "adopted_on_actual_main_after_dk_kq");
    assert_eq!(build["binary"], execution["sourceBindings"]["binary"]);
    assert_eq!(
        hashes["native-build.json"],
        execution["sourceBindings"]["binaryManifest"]["sha256"]
    );
    assert_eq!(hashes["source-epoch.json"], execution["sourceEpochSha256"]);
    assert_eq!(
        process["binaryManifestOriginalSha256"],
        hashes["native-build.json"]
    );
    ensure!(
        !build["sources"]
            .as_object()
            .context("compiled own inputs")?
            .is_empty(),
        "current source build manifest"
    );
    for object in [&adoption["sources"], &build["sources"]] {
        for (relative, expected) in object
            .as_object()
            .context("actual adopted/compiler source")?
        {
            assert_eq!(epoch[relative], *expected);
        }
    }
    let plan = read_boss_json(&root, "plan.json")?;
    let initial = read_boss_json(&root, "initial-fixture.json")?;
    assert_eq!(initial["fixtureOnly"], true);
    assert_eq!(
        initial["seedSourceSha256"],
        execution["sourceBindings"]["seed"]["sha256"]
    );
    ensure!(
        initial["account"].get("resources").is_none(),
        "no seeded live resources"
    );
    ensure!(
        corp_fields(&initial["account"], "skills")?
            .iter()
            .all(|row| row["level"] == CORP_INITIAL_LEVEL),
        "ordinary initial full99 fixture"
    );
    assert_eq!(
        plan["recordingScope"]["identity"],
        "native_ordinary_melee_sample"
    );
    assert_eq!(plan["room"]["id"], "corporeal-beast");
    assert_eq!(plan["room"]["entryLoc"], loc::CORPOREAL_LAIR_PASSAGE.id());
    assert_eq!(plan["room"]["exitLoc"], loc::CORPOREAL_LAIR_PASSAGE.id());
    assert_eq!(plan["target"]["npc"], npc::CORPOREAL_BEAST.id());
    assert_eq!(plan["core"]["npc"], npc::CORPOREAL_DARK_ENERGY_CORE.id());
    assert_eq!(plan["target"]["profile"]["hitpoints"], CORP_FULL_BOSS_LIFE);
    assert_eq!(plan["core"]["profile"]["hitpoints"], CORP_FULL_CORE_LIFE);
    assert_eq!(plan["core"]["maximumDamage"], CORP_CORE_MAXIMUM_DAMAGE);
    assert_eq!(plan["core"]["blocked"], serde_json::json!([]));
    assert_eq!(plan["loadout"]["tier"], CORP_WEAPON_TIER);
    assert_eq!(plan["loadout"]["armourTier"], CORP_ARMOUR_TIER);
    assert_eq!(plan["loadout"]["style"], "melee");
    assert_eq!(plan["loadout"]["blocked"], serde_json::json!([]));
    assert_eq!(plan["source"]["initialSkillsAndSavedPreferenceOnly"], true);
    assert_eq!(plan["lootPolicy"]["fixedItem"], Value::Null);
    let journal = read_boss_rows(&root, "driver-journal.jsonl")?;
    let combat = read_boss_rows(&root, "combat-receipts.jsonl")?;
    let backend = read_boss_json(&root, "backend-inputs.json")?;
    for field in ["closed", "coreClosed"] {
        assert_eq!(backend["closure"][field], true);
    }
    assert_eq!(backend["closure"]["errors"], serde_json::json!([]));
    assert_eq!(
        backend["closure"]["socketPromises"]["pending"],
        serde_json::json!([])
    );
    assert_eq!(process["backend"], backend["closure"]);
    let enter = boss_route_stage(&journal, "enter")?;
    let sample = boss_route_stage(&journal, "ordinary_melee_sample")?;
    let sample_tick = boss_integer_field(sample, "throughTick")?;
    let scoped: Vec<_> = combat
        .iter()
        .filter(|row| row["tick"].as_i64().is_some_and(|tick| tick <= sample_tick))
        .collect();
    let complete = boss_route_stage(&journal, "complete")?;
    assert_eq!(complete["status"], "ordinary_melee_sample_pass");
    let boss = &sample["fullLife"];
    let final_body = &sample["finalBody"];
    ensure!(
        corp_same_actor(boss, final_body),
        "sample retains original actor life"
    );
    let remaining = boss_integer_field(final_body, "hitpoints")?;
    ensure!(
        remaining > CORP_ZERO && remaining < CORP_FULL_BOSS_LIFE,
        "ordinary live damaged body sample"
    );
    let instance = &enter["passive"]["instance"];
    assert_eq!(boss["hitpoints"], CORP_FULL_BOSS_LIFE);
    assert_eq!(boss["maximumLife"], CORP_FULL_BOSS_LIFE);
    assert_eq!(boss["instance"], *instance);
    assert_eq!(enter["passive"]["membership"], true);
    assert_eq!(enter["passive"]["room"], plan["publicRoom"]);
    assert_eq!(enter["source"]["instance"], Value::Null);
    ensure!(
        boss_integer_field(&enter["source"], "x")? < boss_integer_field(&enter["loc"], "x")?
            && boss_integer_field(&enter["state"]["player"], "x")?
                > boss_integer_field(&enter["loc"], "x")?,
        "ordinary same-square directional entry"
    );
    let route: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == "leave" || row["kind"] == "rejoin")
        .collect();
    let narrowed: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == "scope_narrowed")
        .collect();
    if narrowed.is_empty() {
        ensure!(
            route.len() == CORP_ONE + CORP_ONE,
            "complete native exit/rejoin"
        );
        let leave = boss_route_stage(&journal, "leave")?;
        let rejoin = boss_route_stage(&journal, "rejoin")?;
        assert_eq!(leave["passive"]["instance"], Value::Null);
        assert_eq!(rejoin["passive"]["instance"], *instance);
        ensure!(
            boss_integer_field(&leave["source"], "x")? > boss_integer_field(&leave["loc"], "x")?
                && boss_integer_field(&leave["state"]["player"], "x")?
                    < boss_integer_field(&leave["loc"], "x")?,
            "ordinary directional exit"
        );
    } else {
        ensure!(
            narrowed.len() == CORP_ONE && route.is_empty(),
            "one explicit minimal scope, no partial lifecycle claim"
        );
        assert_eq!(
            narrowed[CORP_FIRST]["omitted"],
            serde_json::json!(["leave", "rejoin"])
        );
        assert_eq!(narrowed[CORP_FIRST]["socketLifecycleRequired"], true);
    }
    let player = &enter["passive"];
    ensure!(
        !scoped
            .iter()
            .any(|row| row["kind"] == "death" && corp_same_actor(&row["target"], boss)),
        "no full-kill sample relabel"
    );
    let hits: Vec<_> = scoped
        .iter()
        .filter(|row| row["kind"] == "landed" && corp_same_actor(&row["target"], boss))
        .collect();
    ensure!(!hits.is_empty(), "ordinary player impacts required");
    let restriction = &plan["restriction"];
    assert_eq!(restriction["scale"]["numerator"], CORP_HALF_NUMERATOR);
    assert_eq!(restriction["scale"]["denominator"], CORP_HALF_DENOMINATOR);
    let mut keys = BTreeSet::new();
    let mut loss = CORP_ZERO;
    for hit in hits {
        assert_eq!(hit["source"]["id"], player["pid"]);
        assert_eq!(hit["source"]["generation"], player["generation"]);
        let weapon = &hit["weapon"];
        assert_eq!(weapon["hand"], "main");
        ensure!(
            weapon["physicalKey"].is_string(),
            "actual frozen physical item key"
        );
        keys.insert(weapon["physicalKey"].as_str().context("physical key")?);
        ensure!(
            corp_fields(&restriction["selectedWeapon"], "forms")?
                .iter()
                .any(|row| row["item"] == weapon["item"] && row["category"] == weapon["category"]),
            "actual admitted native weapon form"
        );
        ensure!(
            !corp_fields(restriction, "fullDamageWeapons")?.contains(&weapon["item"])
                && !corp_fields(restriction, "fullDamageClasses")?.contains(&weapon["category"]),
            "no full-damage spear substitution"
        );
        let expected = boss_integer_field(hit, "launchedDamage")? * CORP_HALF_NUMERATOR
            / CORP_HALF_DENOMINATOR;
        assert_eq!(hit["nativeDamage"], expected);
        let actual = boss_integer_field(hit, "actualDamage")?;
        ensure!(
            (CORP_ZERO..=expected).contains(&actual),
            "actual HP loss is clamped native damage"
        );
        loss += actual;
    }
    ensure!(
        keys.len() == CORP_ONE,
        "same physical weapon throughout ordinary fight"
    );
    let mut healing = CORP_ZERO;
    for row in scoped
        .iter()
        .filter(|row| row["kind"] == "npc_healing" && corp_same_actor(&row["target"], boss))
    {
        let accepted = boss_integer_field(row, "acceptedHealing")?;
        ensure!(
            accepted
                == boss_integer_field(row, "afterLife")? - boss_integer_field(row, "beforeLife")?
                && (CORP_ZERO..=boss_integer_field(row, "requested")?).contains(&accepted)
                && boss_integer_field(row, "afterLife")? <= CORP_FULL_BOSS_LIFE,
            "normal clamped core healing"
        );
        healing += accepted;
    }
    assert_eq!(loss, CORP_FULL_BOSS_LIFE - remaining + healing);
    ensure!(
        scoped.iter().any(|row| row["kind"] == "landed"
            && corp_same_actor(&row["source"], boss)
            && row["target"]["id"] == player["pid"]
            && row["actualDamage"]
                .as_i64()
                .is_some_and(|damage| damage > CORP_ZERO)),
        "real ordinary body incoming damage"
    );
    ensure!(
        scoped.iter().any(|row| row["kind"] == "food_commit"),
        "actual ordinary food during sample"
    );
    ensure!(
        boss_integer_field(&sample["player"], "prayerFine")?
            < boss_integer_field(player, "prayerFine")?,
        "selected ordinary prayer consumes actual initial resource"
    );
    let sockets = read_boss_json(&root, "socket-qualification.json")?;
    assert_eq!(sockets["clientAdmissionClaimed"], false);
    assert_eq!(sockets["originalCohortPassed"], false);
    assert_eq!(sockets["targetedHardRepairPassed"], true);
    let kits = corp_fields(&sockets, "kits")?;
    assert_eq!(kits.len(), CORP_ADMITTED_KIT_COUNT);
    let mut admitted = BTreeSet::new();
    for kit in kits {
        assert_eq!(kit["fullLife"], CORP_FULL_BOSS_LIFE);
        assert_eq!(kit["coreSeen"], true);
        assert_eq!(
            boss_integer_field(kit, "acceptedBossLoss")?,
            CORP_FULL_BOSS_LIFE + boss_integer_field(kit, "acceptedHealing")?
        );
        ensure!(
            admitted.insert(kit["choice"].to_string()),
            "distinct ordinary admitted kit"
        );
    }
    let hit_types = read_boss_json(&root, "native-hit-types.json")?;
    assert_eq!(
        hit_types["status"],
        "derived_from_captured_immutable_queue_inputs_through_original_pure_encoder"
    );
    assert_eq!(hit_types["gameplayWrites"], CORP_ZERO);
    assert_eq!(
        hit_types["originalCombatSha256"],
        hashes["combat-receipts.jsonl"]
    );
    assert_eq!(hit_types["originalRtrSha256"], hashes["session.rtr"]);
    let encoder_relative = hit_types["owner"]["relative"]
        .as_str()
        .context("actual encoder owner")?;
    assert_eq!(epoch[encoder_relative], hit_types["owner"]["sha256"]);
    let hit_publications = corp_fields(&hit_types, "publications")?;
    let publications: Vec<_> = combat
        .iter()
        .filter(|row| row["kind"] == "native_publication")
        .collect();
    ensure!(
        !publications.is_empty(),
        "ordinary before-flush publication queues"
    );
    assert_eq!(hit_publications.len(), publications.len());
    let mut natural_cores = BTreeSet::new();
    for row in combat.iter().filter(|row| row["kind"] == "state") {
        for enemy in corp_fields(row, "enemies")? {
            if enemy["definition"] == plan["core"]["npc"] && enemy["instance"] == *instance {
                let life = (
                    boss_integer_field(enemy, "id")?,
                    boss_integer_field(enemy, "generation")?,
                    boss_integer_field(enemy, "actorToken")?,
                );
                if natural_cores.insert(life) {
                    assert_eq!(enemy["maximumLife"], CORP_FULL_CORE_LIFE);
                    assert_eq!(enemy["hitpoints"], CORP_FULL_CORE_LIFE);
                }
            }
        }
    }
    let states: Vec<_> = combat.iter().filter(|row| row["kind"] == "state").collect();
    let core_hits: Vec<_> = combat
        .iter()
        .filter(|row| {
            row["kind"] == "landed"
                && row["source"]["definition"] == plan["core"]["npc"]
                && row["source"]["instance"] == *instance
        })
        .collect();
    for hit in &core_hits {
        let source = &hit["source"];
        let target = &hit["target"];
        assert_eq!(target["id"], player["pid"]);
        assert_eq!(target["generation"], player["generation"]);
        assert_eq!(target["instance"], *instance);
        assert_eq!(source["level"], target["level"]);
        let distance = (boss_integer_field(source, "x")? - boss_integer_field(target, "x")?)
            .abs()
            .max((boss_integer_field(source, "z")? - boss_integer_field(target, "z")?).abs());
        ensure!(
            distance <= boss_integer_field(&plan["core"], "radiusTiles")?
                && (CORP_ZERO..=CORP_CORE_MAXIMUM_DAMAGE)
                    .contains(&boss_integer_field(hit, "launchedDamage")?),
            "natural3x3/native core cap"
        );
        let matches: Vec<_> = combat
            .iter()
            .filter(|row| {
                row["kind"] == "npc_healing"
                    && corp_same_actor(&row["target"], boss)
                    && row["tick"] == hit["tick"]
                    && row["requested"] == hit["actualDamage"]
            })
            .collect();
        ensure!(
            matches.len() == CORP_ONE,
            "actual core accepted loss has one ordinary healing owner"
        );
    }
    for hit in combat.iter().filter(|row| {
        row["kind"] == "landed"
            && row["target"]["definition"] == plan["core"]["npc"]
            && row["actualDamage"]
                .as_i64()
                .is_some_and(|damage| damage > CORP_ZERO)
    }) {
        let first_tick = boss_integer_field(hit, "tick")?;
        let until = first_tick
            + (boss_integer_field(&plan["core"], "pauseMilliseconds")?
                + CORP_GAME_TICK_MILLISECONDS
                - CORP_ONE as i64)
                / CORP_GAME_TICK_MILLISECONDS;
        let complete_window = states.iter().any(|row| {
            row["tick"] == until - CORP_ONE as i64
                && row["enemies"].as_array().is_some_and(|enemies| {
                    enemies.iter().any(|enemy| {
                        corp_same_actor(enemy, &hit["target"])
                            && enemy["currentLife"]
                                .as_i64()
                                .is_some_and(|life| life > CORP_ZERO)
                    })
                })
        });
        if complete_window {
            ensure!(
                !core_hits
                    .iter()
                    .any(|row| corp_same_actor(&row["source"], &hit["target"])
                        && row["tick"]
                            .as_i64()
                            .is_some_and(|tick| tick > first_tick && tick < until)),
                "naturally complete positive-hit pause"
            );
        }
    }
    for state in &states {
        let tick = boss_integer_field(state, "tick")?;
        for enemy in corp_fields(state, "enemies")?.iter().filter(|enemy| {
            enemy["definition"] == plan["core"]["npc"]
                && enemy["poisoned"] == true
                && enemy["currentLife"]
                    .as_i64()
                    .is_some_and(|life| life > CORP_ZERO)
        }) {
            let previous = states.iter().any(|row| {
                row["tick"] == tick - CORP_ONE as i64
                    && row["enemies"].as_array().is_some_and(|enemies| {
                        enemies
                            .iter()
                            .any(|other| corp_same_actor(other, enemy) && other["poisoned"] == true)
                    })
            });
            if previous {
                ensure!(
                    !core_hits
                        .iter()
                        .any(|hit| hit["tick"] == tick && corp_same_actor(&hit["source"], enemy)),
                    "natural consecutive poison phases suppress drain"
                );
            }
        }
    }
    for food in combat.iter().filter(|row| row["kind"] == "food_commit") {
        ensure!(
            boss_integer_field(food, "healed")? > CORP_ZERO
                && boss_integer_field(food, "healed")?
                    == boss_integer_field(food, "lifeAfter")?
                        - boss_integer_field(food, "lifeBefore")?,
            "actual ordinary food healing"
        );
        assert_eq!(food["beforeCount"], CORP_ONE);
        assert_eq!(food["afterCount"], CORP_ZERO);
    }
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
    let accepted: Vec<_> = journal
        .iter()
        .filter(|row| {
            row["kind"] == "control"
                && row["request"]["command"] == "action"
                && row["response"]["status"] == "accepted"
        })
        .collect();
    let applied = corp_fields(&backend, "inputs")?;
    assert_eq!(inputs.len(), accepted.len());
    assert_eq!(inputs.len(), applied.len());
    assert_eq!(complete["actions"].as_u64(), Some(inputs.len() as u64));
    for ((record, actual), control) in inputs.iter().zip(applied).zip(accepted) {
        assert_eq!(
            record.cycle,
            i32::try_from(boss_integer_field(actual, "cycle")?)?
        );
        assert_eq!(
            record.bytes,
            serde_json::from_value::<Vec<u8>>(actual["event"].clone())?
        );
        assert_eq!(control["response"]["cycle"], record.cycle);
        let action = &control["request"]["action"];
        ensure!(
            super::scenario_god_wars::input_matches(
                action,
                &control["request"]["map"],
                action["definition"].as_i64(),
                &InputEvent::decode(&record.bytes)?
            ),
            "actual accepted ordinary native action"
        );
    }
    let frames = arrivals(&trace)?;
    let (players, npcs) = server_trace_file(&root.join("server-trace.jsonl"))?;
    assert_eq!(
        publications.len(),
        players.len(),
        "one passive queue row per actual publication tick"
    );
    assert_eq!(publications.len(), npcs.len());
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
        (CORP_FIRST, CORP_FIRST, CORP_FIRST, CORP_FIRST);
    let mut queues = BTreeMap::new();
    let mut boss_seen = false;
    let mut damaged_bar = false;
    let mut native_core_lives = BTreeSet::new();
    let target_index = usize::try_from(boss_integer_field(boss, "id")?)?;
    for cycle in CORP_FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        let expected = trace.bytes(b"OUT ", cycle);
        assert_eq!(
            mask_wall_clock(&output.written)?,
            mask_wall_clock(&expected)?,
            "cycle{cycle}: complete normal outgoing bytes"
        );
        for bytes in [&output.written, &expected] {
            ensure!(
                client_frames(bytes)?
                    .iter()
                    .all(|(opcode, _)| *opcode != cp::CLIENT_CHEAT),
                "no cheat packets"
            );
        }
        let next = replay.processed(&frames);
        let fresh = &frames[done..next];
        ensure!(
            fresh
                .iter()
                .filter(|frame| frame.opcode == sp::PLAYER_INFO)
                .count()
                <= CORP_ONE
                && fresh
                    .iter()
                    .filter(|frame| frame.opcode == sp::NPC_INFO)
                    .count()
                    <= CORP_ONE,
            "one independently observable actor tick"
        );
        let state = observe(replay.game());
        for frame in fresh {
            if frame.opcode == sp::PLAYER_INFO {
                let expected = players
                    .get(player_updates)
                    .context("ordered PLAYER publication")?;
                assert_eq!(
                    state.local.as_ref().context("native player")?.tile,
                    expected[..CORP_TILE_FIELDS]
                );
                let publication = publications
                    .get(player_updates)
                    .context("corresponding passive PLAYER queue")?;
                assert_eq!(
                    expected[..CORP_TILE_FIELDS],
                    [
                        i32::try_from(boss_integer_field(&publication["player"], "x")?)?,
                        i32::try_from(boss_integer_field(&publication["player"], "z")?)?,
                        i32::try_from(boss_integer_field(&publication["player"], "level")?)?
                    ],
                    "passive actual publication position"
                );
                let player_index =
                    usize::try_from(boss_integer_field(&publication["player"], "id")?)?;
                let actual = replay
                    .game()
                    .runtime
                    .feed
                    .state
                    .players
                    .players
                    .get(player_index)
                    .and_then(Option::as_ref)
                    .context("published local native actor")?;
                let encoded = &hit_publications[player_updates];
                assert_eq!(encoded["tick"], publication["tick"]);
                for field in ["id", "generation", "actorToken", "definition"] {
                    assert_eq!(encoded["player"][field], publication["player"][field]);
                }
                corp_queues(
                    &publication["player"],
                    actual.combat.as_ref(),
                    &mut queues,
                    false,
                    player_index,
                    CorpQueueProof {
                        published_life: corp_published_life(
                            &publication["player"],
                            boss_integer_field(publication, "tick")?,
                            &combat,
                        )?,
                        native_hits: corp_fields(
                            &hit_publications[player_updates]["player"],
                            "hits",
                        )?,
                    },
                )?;
                player_updates += CORP_ONE;
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
                let actual: BTreeMap<_, _> = state
                    .npcs
                    .iter()
                    .map(|(&index, (definition, actor))| (index, (*definition, actor.tile)))
                    .collect();
                assert_eq!(actual, expected, "every actual native roster/type/tile");
                let publication = publications
                    .get(npc_updates)
                    .context("corresponding passive NPC queues")?;
                for enemy in corp_fields(publication, "enemies")? {
                    let index = usize::try_from(boss_integer_field(enemy, "id")?)?;
                    if !expected.contains_key(&index) {
                        continue;
                    }
                    let actual = replay
                        .game()
                        .runtime
                        .feed
                        .state
                        .npcs
                        .entities
                        .get(&index)
                        .context("ordinary native boss/core")?;
                    assert_eq!(
                        i64::from(actual.type_id),
                        boss_integer_field(enemy, "definition")?
                    );
                    assert_eq!(
                        expected[&index].1,
                        [
                            i32::try_from(boss_integer_field(enemy, "x")?)?,
                            i32::try_from(boss_integer_field(enemy, "z")?)?,
                            i32::try_from(boss_integer_field(enemy, "level")?)?
                        ],
                        "passive NPC publication position"
                    );
                    let encoded = corp_fields(&hit_publications[npc_updates], "enemies")?
                        .iter()
                        .find(|row| {
                            row["id"] == enemy["id"]
                                && row["generation"] == enemy["generation"]
                                && row["actorToken"] == enemy["actorToken"]
                        })
                        .context("pure encoder actor cohort")?;
                    assert_eq!(hit_publications[npc_updates]["tick"], publication["tick"]);
                    corp_queues(
                        enemy,
                        actual.path.combat.as_ref(),
                        &mut queues,
                        true,
                        index,
                        CorpQueueProof {
                            published_life: corp_published_life(
                                enemy,
                                boss_integer_field(publication, "tick")?,
                                &combat,
                            )?,
                            native_hits: corp_fields(encoded, "hits")?,
                        },
                    )?;
                    if actual.type_id == npc::CORPOREAL_DARK_ENERGY_CORE.id()
                        && enemy["instance"] == *instance
                    {
                        native_core_lives.insert((
                            boss_integer_field(enemy, "id")?,
                            boss_integer_field(enemy, "generation")?,
                            boss_integer_field(enemy, "actorToken")?,
                        ));
                    }
                }
                queues.retain(|(is_npc, index), _| !*is_npc || expected.contains_key(index));
                npc_updates += CORP_ONE;
            }
        }
        done = next;
        for row in snapshots.get(&cycle).into_iter().flatten() {
            let actual = crate::live_control::observed_snapshot_for_test(
                replay.core.session.as_ref().context("ordinary session")?,
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
            checked_snapshots += CORP_ONE;
        }
        if let Some(enemy) = replay
            .game()
            .runtime
            .feed
            .state
            .npcs
            .entities
            .get(&target_index)
        {
            if enemy.type_id == npc::CORPOREAL_BEAST.id() {
                boss_seen = true;
                if let Some(combat) = &enemy.path.combat {
                    damaged_bar |= combat.bars.iter().any(|bar| {
                        bar.updates.iter().any(|update| {
                            update[CORP_HEADBAR_END] > CORP_ZERO as i32
                                && i64::from(update[CORP_HEADBAR_END]) < CORP_HEADBAR_FILL_MAX
                        })
                    });
                }
            }
        }
    }
    ensure!(
        boss_seen && damaged_bar,
        "actual native living Corp and ordinary damaged HP publication"
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
            && players.len() - player_updates <= CORP_UNRECEIVED_CLOSING_LIMIT
            && npcs.len() - npc_updates <= CORP_UNRECEIVED_CLOSING_LIMIT,
        "bounded unreceived closing actor suffix"
    );
    let final_player = players
        .get(
            player_updates
                .checked_sub(CORP_ONE)
                .context("received PLAYER")?,
        )
        .context("final PLAYER")?;
    let final_npcs = npcs
        .get(npc_updates.checked_sub(CORP_ONE).context("received NPC")?)
        .context("final NPC")?;
    ensure!(
        players[player_updates..]
            .iter()
            .all(|row| row == final_player)
            && npcs[npc_updates..].iter().all(|row| row == final_npcs),
        "closing suffix contains duplicate-only actor publications"
    );
    assert_eq!(checked_snapshots, wanted_snapshots);
    ensure!(
        checked_snapshots > CORP_FIRST && player_updates > CORP_FIRST && npc_updates > CORP_FIRST,
        "ordinary native input/UI/actors actually installed"
    );
    ensure!(
        native_core_lives.is_subset(&natural_cores),
        "every decoded native core belongs to a naturally full-health observed cohort"
    );
    eprintln!("Corp decoded native core lives: {} of {} passive cohorts; absence does not claim client admission", native_core_lives.len(), natural_cores.len());
    Ok(())
}
