//! Recorded ordinary Barrows crypt combat, native puzzle and chest withdrawal.
//! Declared pre-login resources are fixture setup; route and outcomes are observed.
use super::scenario_legacy_combat::gameplay;
use super::scenario_tests::{ui, walk_components};
use super::scenario_woodcutting::all_components;
use super::session_replay::{
    arrivals, client_frames, observe, server_trace_file, Replay, Trace,
    AUTHENTICATED_HEADLESS_BACKEND, INITIAL_RECORDING_OUTPUT_CYCLE, OBSERVED_BACKEND_HEAD,
};
use crate::client_core::input_event::{InputEvent, RECORD_TAG};
use anyhow::Context;
use rs910_symbols::{component, interface, inv, obj, varbit, varp};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const FIXTURE: &str = "fixtures/session-replay/barrows";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const ONE: i32 = 1;
const RETALIATION_OFF: i32 = 1;
const RETALIATION_OFF_POSTCONDITION: &str = "native auto-retaliation OFF before tunnel navigation";
const FIRST_INDEX: usize = 0;
const QUANTITY_INDEX: usize = 1;
const INSTANCE_INDEX: usize = 2;
const REQUIRED_REPAIR_OWNERS: usize = 2;
const SHA256_HEX_BYTES: usize = 64;
const REPAIR_KINDS: [&str; REQUIRED_REPAIR_OWNERS] = [
    "Bob native Repair-all",
    "Installed armour stand native Repair",
];
const SEMANTIC_STATUS: &str = "semantic_pass_requires_recorded_replay_and_readback_qualification";
const HEADLESS_FULL_ROUTE_STATUS: &str = "full_route_headless_pass_requires_live_capture";
const HEADLESS_EXECUTION_MODE: &str = "headless_full_route";
const BROTHER_BITS: [(&str, i32); ORIGINAL_BROTHERS] = [
    ("ahrim", varbit::BARROWS_AHRIM_DEFEATED.id()),
    ("dharok", varbit::BARROWS_DHAROK_DEFEATED.id()),
    ("guthan", varbit::BARROWS_GUTHAN_DEFEATED.id()),
    ("karil", varbit::BARROWS_KARIL_DEFEATED.id()),
    ("torag", varbit::BARROWS_TORAG_DEFEATED.id()),
    ("verac", varbit::BARROWS_VERAC_DEFEATED.id()),
];
const SCRIPTED_INPUT_VARIABLES: &[&str] = &[
    "CLIENT910_UI_OPERATIONS",
    "CLIENT910_UI_CLICKS",
    "CLIENT910_UI_HOVER",
    "CLIENT910_KEY_INPUT",
    "CLIENT910_WINDOW_RESIZES",
    "CLIENT910_CONSOLE_COMMANDS",
    "CLIENT910_TYPE_INPUT",
    "CLIENT910_WHEEL_INPUT",
    "CLIENT910_TOOLKIT_INPUT",
    "CLIENT910_UI_INPUT",
    "CLIENT910_UI_CLICK",
    "CLIENT910_CUTSCENE",
    "CLIENT910_DROP_CONNECTION",
];
const ORIGINAL_BROTHERS: usize = 6;
const GROUP_BITS: i32 = 16;
const HIT_DAMAGE_INDEX: usize = 1;
const HEALTH_BAR_END: usize = 2;
const TILE_FIELDS: usize = 3;
const RECT_WIDTH: usize = 2;
const RECT_HEIGHT: usize = 3;

fn integer(row: &Value, field: &str) -> anyhow::Result<i64> {
    row[field]
        .as_i64()
        .with_context(|| format!("{field}: {row}"))
}

fn inventory(replay: &mut Replay, identity: i32) -> Vec<(i32, i32)> {
    ui(replay)
        .engine
        .inv_cache
        .inventory(identity, false)
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

fn bit(replay: &Replay, identity: i32) -> anyhow::Result<i32> {
    replay
        .game()
        .varbit_value(identity as u16)
        .map_err(|error| anyhow::anyhow!("{error:?}"))
}

fn visible_group(replay: &mut Replay, identity: i32) -> bool {
    let mut visible = false;
    walk_components(ui(replay), &mut |rect, component| {
        visible |= component.f.parentlayer >> GROUP_BITS == identity
            && rect[RECT_WIDTH] > ZERO
            && rect[RECT_HEIGHT] > ZERO;
    });
    visible
}

fn array<'a>(row: &'a Value, field: &str) -> anyhow::Result<&'a [Value]> {
    row[field]
        .as_array()
        .map(Vec::as_slice)
        .with_context(|| format!("{field}: {row}"))
}

fn read_json(root: &std::path::Path, name: &str) -> anyhow::Result<Value> {
    serde_json::from_slice(&std::fs::read(root.join(name))?).with_context(|| name.to_string())
}

fn read_rows(root: &std::path::Path, name: &str) -> anyhow::Result<Vec<Value>> {
    let text = std::fs::read_to_string(root.join(name))?;
    anyhow::ensure!(
        text.ends_with('\n'),
        "{name}: closed evidence has a partial final row"
    );
    text.lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .with_context(|| name.to_string())
}

fn stack_count(slots: &Value, item: i64) -> anyhow::Result<i64> {
    slots
        .as_array()
        .context("saved slots")?
        .iter()
        .filter(|slot| slot[FIRST_INDEX].as_i64() == Some(item))
        .map(|slot| slot[QUANTITY_INDEX].as_i64().context("saved quantity"))
        .sum()
}

fn physical<'a>(player: &'a Value, key: &str) -> anyhow::Result<&'a Value> {
    let matches: Vec<_> = array(player, "backpack")?
        .iter()
        .chain(array(player, "worn")?)
        .filter(|slot| slot[INSTANCE_INDEX]["key"].as_str() == Some(key))
        .collect();
    anyhow::ensure!(
        matches.len() == ONE as usize,
        "exactly one saved physical UUID {key}"
    );
    Ok(matches[FIRST_INDEX])
}

fn reward_counts(run: &Value) -> anyhow::Result<BTreeMap<i64, i64>> {
    let mut counts = BTreeMap::new();
    for reward in array(run, "rewards")? {
        let count = integer(reward, "count")?;
        anyhow::ensure!(count > i64::from(ZERO), "positive actual reward");
        *counts.entry(integer(reward, "item")?).or_default() += count;
    }
    Ok(counts)
}

fn final_player(rows: &[Value]) -> anyhow::Result<&Value> {
    rows.iter()
        .rev()
        .find(|row| row["kind"] == "state")
        .and_then(|row| row["players"].as_array())
        .and_then(|players| players.first())
        .context("final independent player state")
}

fn server_receipts(driver: &Value, rows: &[Value]) -> anyhow::Result<()> {
    let full_health = driver["six_full_health"]
        .as_object()
        .context("six observed full-health lives")?;
    assert_eq!(full_health.len(), ORIGINAL_BROTHERS);
    for state in rows.iter().filter(|row| row["kind"] == "state") {
        for player in array(state, "players")? {
            let run = &player["run"];
            if run.is_null() {
                continue;
            }
            assert_eq!(
                integer(player, "totalKills")?,
                i64::try_from(array(run, "killed")?.len())? + integer(run, "creatureCount")?,
                "independent displayed kills include each recorded brother and tunnel creature"
            );
        }
    }
    let brother_names: BTreeSet<_> = BROTHER_BITS.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        full_health
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        brother_names
    );
    let expected: BTreeMap<_, _> = full_health
        .values()
        .map(|life| Ok((integer(life, "definition")?, life)))
        .collect::<anyhow::Result<_>>()?;
    assert_eq!(expected.len(), ORIGINAL_BROTHERS);
    let pid = integer(final_player(rows)?, "pid")?;
    let old_generation = integer(driver, "old_generation")?;
    let new_generation = integer(driver, "new_generation")?;
    assert_ne!(
        old_generation, new_generation,
        "actual re-entry replaces the run"
    );
    let mut full = BTreeMap::new();
    let mut deaths = BTreeMap::new();
    let mut credited = false;
    let mut puzzle = false;
    let mut claimed_rewards = None;
    let mut withdrew = false;
    let mut outside = false;
    let mut reset = false;
    let mut previous: Option<&Value> = None;
    let mut transferred = BTreeMap::<i64, i64>::new();
    for row in rows {
        if row["kind"] == "death" && row["sourceId"].as_i64() == Some(pid) {
            if let Some(definition) = row["targetDefinition"]
                .as_i64()
                .filter(|id| expected.contains_key(id))
            {
                let spawn: &&Value = full
                    .get(&definition)
                    .context("full-HP life precedes credited death")?;
                assert_eq!(row["targetId"], spawn["id"]);
                assert_eq!(row["generation"], spawn["generation"]);
                assert_eq!(row["instance"], spawn["instance"]);
                assert!(integer(row, "tick")? >= integer(spawn, "tick")?);
                *deaths.entry(definition).or_insert(ZERO) += ONE;
            }
        }
        if row["kind"] != "state" {
            continue;
        }
        for enemy in array(row, "enemies")? {
            let definition = integer(enemy, "definition")?;
            if let Some(recorded) = expected.get(&definition) {
                if enemy["hitpoints"] == enemy["maximumHitpoints"]
                    && !full.contains_key(&definition)
                {
                    assert!(integer(enemy, "hitpoints")? > i64::from(ZERO));
                    assert_eq!(recorded["id"], enemy["id"]);
                    assert_eq!(recorded["generation"], enemy["generation"]);
                    assert_eq!(recorded["instance"], enemy["instance"]);
                    assert_eq!(recorded["hitpoints"], enemy["hitpoints"]);
                    assert_eq!(recorded["maximumHitpoints"], enemy["maximumHitpoints"]);
                    assert_eq!(recorded["tick"], row["tick"]);
                    full.insert(definition, *recorded);
                }
            }
        }
        for player in array(row, "players")? {
            if integer(player, "pid")? != pid {
                continue;
            }
            assert!(
                integer(player, "life")? > i64::from(ZERO),
                "ordinary fight and travel survive"
            );
            let run = &player["run"];
            if run.is_null() {
                continue;
            }
            let names: BTreeSet<_> = array(run, "killed")?
                .iter()
                .map(|name| name.as_str().context("distinct credit name"))
                .collect::<anyhow::Result<_>>()?;
            assert_eq!(
                names.len(),
                array(run, "killed")?.len(),
                "no duplicate credits"
            );
            credited |=
                run["generation"].as_i64() == Some(old_generation) && names == brother_names;
            puzzle |= player["puzzleOpen"] == true && !run["puzzle"].is_null();
            if run["generation"].as_i64() == Some(old_generation) && run["claimed"] == true {
                let pending = reward_counts(run)?;
                if !pending.is_empty() && claimed_rewards.is_none() {
                    assert!(
                        credited && player["rewardsOpen"] == true,
                        "six credits before native chest claim"
                    );
                    claimed_rewards = Some(pending.clone());
                }
                if let Some(before) = previous.filter(|before| {
                    before["run"]["generation"] == run["generation"]
                        && before["run"]["claimed"] == true
                }) {
                    for (item, count) in reward_counts(&before["run"])? {
                        let remaining = pending.get(&item).copied().unwrap_or_default();
                        assert!(
                            remaining <= count,
                            "pending loot cannot grow during withdrawal"
                        );
                        let removed = count - remaining;
                        if removed == i64::from(ZERO) {
                            continue;
                        }
                        let added = if item == i64::from(obj::COINS.id()) {
                            integer(player, "coins")? - integer(before, "coins")?
                        } else {
                            stack_count(&player["backpack"], item)?
                                - stack_count(&before["backpack"], item)?
                        };
                        assert_eq!(
                            added, removed,
                            "actual pending removal equals destination commit"
                        );
                        *transferred.entry(item).or_default() += removed;
                    }
                }
                withdrew |= claimed_rewards
                    .as_ref()
                    .is_some_and(|rewards| pending.is_empty() && *rewards == transferred);
                outside |= withdrew && player["instance"].is_null();
            }
            if outside
                && !player["instance"].is_null()
                && run["generation"].as_i64() == Some(new_generation)
            {
                reset |= names.is_empty()
                    && run["claimed"] == false
                    && array(run, "rewards")?.is_empty()
                    && integer(player, "totalKills")? == i64::from(ZERO)
                    && integer(player, "rewardOpened")? == i64::from(ZERO)
                    && array(player, "slain")?
                        .iter()
                        .all(|slain| slain["value"] == ZERO);
            }
            previous = Some(player);
        }
    }
    assert_eq!(
        full.keys().copied().collect::<BTreeSet<_>>(),
        expected.keys().copied().collect()
    );
    assert_eq!(
        deaths.len(),
        ORIGINAL_BROTHERS,
        "all six credited ordinary deaths"
    );
    assert!(deaths.values().all(|count| *count == ONE));
    assert!(
        credited && puzzle && withdrew && outside && reset,
        "six full lives, puzzle, actual loot, rope exit and new run"
    );
    assert_eq!(
        driver["claimed_run"]["generation"].as_i64(),
        Some(old_generation)
    );
    assert_eq!(driver["claimed_run"]["claimed"], true);
    assert!(array(&driver["claimed_run"], "rewards")?.is_empty());
    let mut reported = BTreeMap::<i64, i64>::new();
    for take in array(driver, "withdrawals")? {
        let before = integer(take, "before")?;
        let after = integer(take, "after")?;
        assert!(before > after && after >= i64::from(ZERO));
        assert_eq!(integer(take, "server_pending")?, after);
        *reported.entry(integer(take, "item")?).or_default() += before - after;
    }
    assert_eq!(
        reported, transferred,
        "actual native Takes cover the full claimed cohort"
    );
    Ok(())
}

fn food_receipts(driver: &Value, rows: &[Value]) -> anyhow::Result<usize> {
    let pid = integer(final_player(rows)?, "pid")?;
    let commits: Vec<_> = rows
        .iter()
        .filter(|row| row["kind"] == "food_commit" && row["pid"].as_i64() == Some(pid))
        .collect();
    assert!(!commits.is_empty(), "normal native food remains required");
    assert_eq!(array(driver, "food")?.len(), commits.len());
    let mut previous_tick = None;
    for commit in &commits {
        assert_eq!(integer(commit, "item")?, i64::from(obj::SHARK.id()));
        assert_eq!(integer(commit, "beforeItem")?, i64::from(obj::SHARK.id()));
        assert_eq!(integer(commit, "beforeCount")?, i64::from(ONE));
        assert_eq!(integer(commit, "afterCount")?, i64::from(ZERO));
        let healed = integer(commit, "healed")?;
        assert!(healed > i64::from(ZERO) && healed <= integer(commit, "maximumLife")?);
        assert_eq!(
            integer(commit, "lifeAfter")? - integer(commit, "lifeBefore")?,
            healed
        );
        assert!(integer(commit, "lifeAfter")? <= integer(commit, "maximumLife")?);
        if let Some(previous_tick) = previous_tick {
            assert!(integer(commit, "tick")? > previous_tick);
        }
        previous_tick = Some(integer(commit, "tick")?);
        assert_eq!(
            array(driver, "food")?
                .iter()
                .filter(|proof| proof["food_commit"] == **commit)
                .count(),
            ONE as usize
        );
    }
    Ok(commits.len())
}

fn repair_receipts(
    plan: &Value,
    driver: &Value,
    rows: &[Value],
    saved: &Value,
) -> anyhow::Result<()> {
    let proofs = array(driver, "repairs")?;
    assert_eq!(proofs.len(), REQUIRED_REPAIR_OWNERS);
    let key = plan["repair"]["hood"]["key"]
        .as_str()
        .context("initial physical hood UUID")?;
    let family = &plan["repair"]["hood"]["family"];
    let commits: Vec<_> = rows
        .iter()
        .filter(|row| row["kind"] == "repair_commit")
        .collect();
    assert_eq!(
        commits.len(),
        REQUIRED_REPAIR_OWNERS,
        "exactly two ordinary accepted repair saves"
    );
    let mut previous_tick = None;
    for (index, proof) in proofs.iter().enumerate() {
        assert_eq!(proof["kind"], REPAIR_KINDS[index]);
        let commit = &proof["commit"];
        assert_eq!(
            commit, commits[index],
            "passive wrapper observes the same original commit"
        );
        assert_eq!(commit["method"], "prepareCoinRepairBatch");
        let before = physical(&commit["before"], key)?;
        let after = physical(&commit["after"], key)?;
        let persisted = physical(&commit["saved"], key)?;
        assert_eq!(&proof["before_physical"], before);
        assert_eq!(after, persisted, "same UUID/output/full balance persisted");
        assert_eq!(after[FIRST_INDEX], family["coinRepair"]["output"]);
        assert_eq!(after[INSTANCE_INDEX]["charges"], family["capacity"]);
        assert_eq!(
            before[INSTANCE_INDEX]["resources"], after[INSTANCE_INDEX]["resources"],
            "other physical resources preserved"
        );
        assert_eq!(
            commit["before"]["life"], commit["after"]["life"],
            "same player life owns the transaction"
        );
        assert_eq!(commit["saved"]["backpack"], commit["after"]["backpack"]);
        assert_eq!(commit["saved"]["worn"], commit["after"]["worn"]);
        assert_eq!(commit["saved"]["coins"], commit["after"]["coins"]);
        assert_eq!(commit["saved"]["revision"], commit["after"]["revision"]);
        anyhow::ensure!(
            commit["savedSha256"]
                .as_str()
                .is_some_and(|hash| hash.len() == SHA256_HEX_BYTES),
            "actual committed account-byte receipt"
        );
        let tick = integer(commit, "tick")?;
        let price = integer(commit, "price")?;
        assert_eq!(proof["price"], commit["price"]);
        if index != FIRST_INDEX {
            let observed = rows
                .iter()
                .rev()
                .filter(|row| {
                    row["kind"] == "state" && row["tick"].as_i64().is_some_and(|at| at <= tick)
                })
                .flat_map(|row| row["players"].as_array().into_iter().flatten())
                .find(|player| player["pid"] == commit["pid"])
                .context("captured Smithing at stand admission")?;
            assert!(integer(observed, "smithing")? >= i64::from(ZERO));
            assert!(
                !observed["instance"].is_null(),
                "installed owned-house admission"
            );
            assert_eq!(observed["house"], plan["repair"]["house"]["saved"]);
        }
        assert_eq!(
            integer(&commit["before"], "coins")? - integer(&commit["after"], "coins")?,
            price
        );
        let quote = format!("Repair this equipment for {price} coins?");
        assert!(array(proof, "quote_texts")?
            .iter()
            .any(|text| text.as_str() == Some(quote.as_str())));
        if let Some(prior) = previous_tick {
            assert!(tick > prior);
            assert!(
                integer(&before[INSTANCE_INDEX], "charges")? < integer(family, "capacity")?,
                "normal wear between repairs"
            );
            assert!(
                rows.iter().any(|row| row["kind"] == "launch"
                    && row["source"] == "player"
                    && row["sourceId"] == commit["pid"]
                    && row["targetDefinition"] == plan["repair"]["wear"]["spawn"]["npc"]
                    && row["tick"]
                        .as_i64()
                        .is_some_and(|at| prior < at && at < tick)
                    && rows.iter().any(|death| death["kind"] == "death"
                        && death["sourceId"] == commit["pid"]
                        && death["targetId"] == row["targetId"]
                        && death["targetDefinition"] == row["targetDefinition"]
                        && death["tick"].as_i64().is_some_and(|at| integer(row, "tick")
                            .is_ok_and(|launch| launch <= at)
                            && at < tick))),
                "ordinary target launch and credited death own wear between repairs"
            );
        }
        previous_tick = Some(tick);
    }
    let final_state = final_player(rows)?;
    assert_eq!(saved["backpack"], final_state["backpack"]);
    assert_eq!(saved["worn"], final_state["worn"]);
    assert_eq!(saved["coins"], final_state["coins"]);
    assert_eq!(
        saved["barrows"], final_state["run"],
        "actual closed saved empty reset run"
    );
    assert_eq!(saved["house"], plan["repair"]["house"]["saved"]);
    assert_eq!(
        physical(saved, key)?[INSTANCE_INDEX]["charges"],
        family["capacity"]
    );
    assert_eq!(driver["final_repair_player"]["run"], final_state["run"]);
    Ok(())
}

fn input_matches(action: &Value, map: &Value, event: &InputEvent) -> bool {
    let same_tile = |choice: &crate::client_core::input_event::MenuChoice| -> bool {
        action["x"]
            .as_i64()
            .zip(map["base_x"].as_i64())
            .is_some_and(|(x, base)| i64::from(choice.tile_x) == x - base)
            && action["z"]
                .as_i64()
                .zip(map["base_z"].as_i64())
                .is_some_and(|(z, base)| i64::from(choice.tile_z) == z - base)
    };
    match (action["kind"].as_str(), event) {
        (Some("walk"), InputEvent::Menu { choice, .. }) => {
            choice.action == crate::ui_scene_options::WALK_ACTION && same_tile(choice)
        }
        (Some("loc"), InputEvent::Menu { choice, .. }) => {
            action["operation"]
                .as_str()
                .is_some_and(|label| choice.op.eq_ignore_ascii_case(label))
                && choice.enabled
                && same_tile(choice)
        }
        (Some("npc"), InputEvent::Menu { choice, .. }) => {
            action["operation"]
                .as_str()
                .is_some_and(|label| choice.op.eq_ignore_ascii_case(label))
                && choice.enabled
                && action["index"].as_i64() == Some(choice.entity_id)
        }
        (
            Some("ui"),
            InputEvent::Component {
                parent,
                child,
                operation,
            },
        ) => {
            i64::from(*parent) == action["target"]["parent"].as_i64().unwrap_or(i64::MIN)
                && i64::from(*child) == action["target"]["child"].as_i64().unwrap_or(i64::MIN)
                && i64::from(*operation) == action["operation"].as_i64().unwrap_or(i64::MIN)
        }
        (
            Some("key"),
            InputEvent::Key {
                code,
                pressed,
                text,
                ..
            },
        ) => {
            action["code"].as_i64() == Some(i64::from(*code))
                && action["pressed"].as_bool() == Some(*pressed)
                && match (text.as_deref(), action.get("text")) {
                    (None, Some(value)) => value.is_null(),
                    (Some(text), Some(value)) => value.as_str() == Some(text),
                    _ => false,
                }
        }
        _ => false,
    }
}

fn captured_inputs(trace: &Trace, journal: &[Value]) -> anyhow::Result<()> {
    assert!(
        !trace.head.iter().any(|(key, _)| key == "server_command"),
        "no scheduled developer commands"
    );
    let environment = trace.env();
    for name in SCRIPTED_INPUT_VARIABLES {
        assert!(
            environment.get(*name).is_none_or(String::is_empty),
            "no timed input injector {name}"
        );
    }
    let mut prior = None;
    let mut events = Vec::new();
    for record in &trace.records {
        if record.tag != *RECORD_TAG {
            continue;
        }
        if let Some(prior) = prior {
            assert!(record.cycle >= prior, "retained UIEV file order");
        }
        prior = Some(record.cycle);
        events.push((record.cycle, InputEvent::decode(&record.bytes)?));
    }
    assert!(!events.is_empty(), "actual interactive retained input");
    let accepted: Vec<_> = journal
        .iter()
        .filter(|row| {
            row["kind"] == "control"
                && row["request"]["command"] == "action"
                && row["response"]["status"] == "accepted"
        })
        .collect();
    assert!(
        !accepted.is_empty(),
        "actual controller acceptance receipts"
    );
    let mut cursor = FIRST_INDEX;
    let mut kinds = BTreeSet::new();
    for row in accepted {
        let cycle = i32::try_from(integer(&row["response"], "cycle")?)?;
        assert!(
            cycle < trace.last_cycle(),
            "input must have a following recorded logic cycle"
        );
        let action = &row["request"]["action"];
        let found = events[cursor..]
            .iter()
            .position(|(at, event)| {
                *at == cycle && input_matches(action, &row["request"]["map"], event)
            })
            .context("accepted action lacks canonical UIEV in the actual receipt order")?;
        cursor += found + ONE as usize;
        assert_eq!(
            row["response"]["data"]["map"], row["request"]["map"],
            "same captured map admission"
        );
        kinds.insert(action["kind"].as_str().context("action kind")?);
    }
    assert!(
        kinds.contains("walk")
            && kinds.contains("loc")
            && kinds.contains("npc")
            && kinds.contains("ui")
    );
    Ok(())
}

fn native_inventory_slots(replay: &mut Replay, id: i32, saved: &Value) -> anyhow::Result<()> {
    let expected: Vec<_> = saved
        .as_array()
        .context("saved slots")?
        .iter()
        .map(|slot| {
            Ok((
                i32::try_from(slot[FIRST_INDEX].as_i64().context("native item")?)?,
                i32::try_from(slot[QUANTITY_INDEX].as_i64().context("native count")?)?,
            ))
        })
        .collect::<anyhow::Result<_>>()?;
    assert_eq!(
        inventory(replay, id),
        expected,
        "native id/count publication matches saved physical container"
    );
    Ok(())
}

fn native_snapshot(replay: &mut Replay, snapshot: &Value) -> anyhow::Result<()> {
    let game = replay.game();
    let map = &game.runtime.map;
    assert!(
        game.runtime.map_request.is_none() && game.runtime.terrain.is_some(),
        "observed ready map stays installed"
    );
    assert_eq!(
        snapshot["map"]["level"],
        serde_json::json!(game.runtime.feed.state.players.current_level)
    );
    assert_eq!(
        snapshot["map"]["base_x"].as_i64(),
        Some(i64::from(map.base_x))
    );
    assert_eq!(
        snapshot["map"]["base_z"].as_i64(),
        Some(i64::from(map.base_z))
    );
    assert_eq!(snapshot["map"]["width"], serde_json::json!(map.width));
    assert_eq!(snapshot["map"]["height"], serde_json::json!(map.height));
    assert_eq!(
        snapshot["map"]["terrain_generation"],
        serde_json::json!(game.runtime.terrain_generation)
    );
    let local = game
        .runtime
        .feed
        .state
        .players
        .players
        .get(map.local)
        .and_then(Option::as_ref)
        .context("native physical player")?;
    assert_eq!(
        snapshot["player"]["fine_x"],
        serde_json::json!(local.fine_x)
    );
    assert_eq!(
        snapshot["player"]["fine_z"],
        serde_json::json!(local.fine_z)
    );
    assert_eq!(
        snapshot["player"]["route_length"],
        serde_json::json!(local.route_length)
    );
    assert_eq!(snapshot["terrain_present"], game.runtime.terrain.is_some());
    if let Some(region) = &game.runtime.installed_region {
        assert_eq!(
            snapshot["region"]["templates"],
            serde_json::to_value(&region.templates)?
        );
        assert_eq!(
            snapshot["region"]["chunks_x"],
            serde_json::json!(region.chunks_x)
        );
        assert_eq!(
            snapshot["region"]["chunks_z"],
            serde_json::json!(region.chunks_z)
        );
    } else {
        assert!(
            snapshot["region"].is_null(),
            "normal exterior installed map"
        );
    }
    for queried in array(snapshot, "varps")? {
        let identity = i32::try_from(integer(queried, "id")?)?;
        let actual = replay
            .game()
            .runtime
            .feed
            .state
            .varps
            .as_ref()
            .and_then(|vars| vars.get(identity).ok());
        assert_eq!(
            queried["value"],
            serde_json::to_value(actual)?,
            "actual queried PLAYER variable matches replay, including absence"
        );
    }
    for queried in array(snapshot, "varbits")? {
        if let Some(value) = queried["value"].as_i64() {
            assert_eq!(
                i64::from(bit(replay, i32::try_from(integer(queried, "id")?)?)?),
                value
            );
        }
    }
    for queried in array(snapshot, "inventories")? {
        if queried["value"].is_null() {
            continue;
        }
        assert_eq!(queried["value"]["truncated"], false);
        let ids = array(&queried["value"], "items")?;
        let counts = array(&queried["value"], "counts")?;
        assert_eq!(ids.len(), counts.len());
        let expected: Vec<_> = ids
            .iter()
            .zip(counts)
            .map(|(id, count)| {
                Ok((
                    i32::try_from(id.as_i64().context("snapshot item")?)?,
                    i32::try_from(count.as_i64().context("snapshot count")?)?,
                ))
            })
            .collect::<anyhow::Result<_>>()?;
        assert_eq!(
            inventory(replay, i32::try_from(integer(queried, "id")?)?),
            expected
        );
    }
    Ok(())
}

fn puzzle_models(replay: &mut Replay, driver: &Value) -> anyhow::Result<bool> {
    use component::barrows_pattern_choices as puzzle;
    let model = |id: i32, replay: &mut Replay| -> anyhow::Result<i32> {
        all_components(ui(replay))
            .iter()
            .find_map(|component| {
                let component = component.borrow();
                (component.f.parentlayer == id).then_some(component.f.model)
            })
            .context("actual native puzzle model component")
    };
    let sequence = [
        puzzle::SEQUENCE_FIRST.packed(),
        puzzle::SEQUENCE_SECOND.packed(),
        puzzle::SEQUENCE_THIRD.packed(),
    ]
    .into_iter()
    .map(|id| model(id, replay))
    .collect::<anyhow::Result<Vec<_>>>()?;
    let answers = [
        puzzle::ANSWER_FIRST.packed(),
        puzzle::ANSWER_SECOND.packed(),
        puzzle::ANSWER_THIRD.packed(),
    ]
    .into_iter()
    .map(|id| model(id, replay))
    .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(array(driver, "puzzles")?.iter().any(|facts| {
        facts["sequence"] == serde_json::json!(sequence)
            && facts["answers"] == serde_json::json!(answers)
    }))
}

fn visible_texts(replay: &mut Replay) -> Vec<String> {
    let mut texts = Vec::new();
    walk_components(ui(replay), &mut |rect, component| {
        if rect[RECT_WIDTH] > ZERO && rect[RECT_HEIGHT] > ZERO {
            texts.push(String::from_utf16_lossy(
                component.f.text.as_deref().unwrap_or(&[]),
            ));
        }
    });
    texts
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_barrows_six_brothers_puzzle_chest_and_new_run() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let plan = read_json(&root, "plan.json")?;
    let driver = read_json(&root, "driver-result.json")?;
    let saved = read_json(&root, "final-saved-state.json")?;
    let initial = read_json(&root, "initial-repair-fixture.json")?;
    let rows = read_rows(&root, "combat-receipts.jsonl")?;
    let journal = read_rows(&root, "driver-journal.jsonl")?;
    let trace = Trace::load(&root.join("session.rtr"))?;
    let observed_backend = trace.head.iter().any(|(key, value)| {
        key == OBSERVED_BACKEND_HEAD && value == AUTHENTICATED_HEADLESS_BACKEND
    });
    if observed_backend {
        assert_eq!(
            driver["status"], HEADLESS_FULL_ROUTE_STATUS,
            "preserve the actual headless milestone, not a relabelled native result"
        );
        assert_eq!(driver["driverSemanticStatus"], SEMANTIC_STATUS);
        assert_eq!(driver["executionMode"], HEADLESS_EXECUTION_MODE);
        assert_eq!(
            driver["rendered"], false,
            "no native window or rendered proof"
        );
    } else {
        assert_eq!(
            driver["status"], SEMANTIC_STATUS,
            "actual combined driver completion"
        );
    }
    assert!(
        plan.get("random").is_none(),
        "no active predetermined random fixture"
    );
    assert_eq!(initial["account"]["coins"], plan["initialFixture"]["coins"]);
    assert_eq!(
        initial["account"]["house"],
        plan["repair"]["house"]["saved"]
    );
    server_receipts(&driver, &rows)?;
    let accepted_food = food_receipts(&driver, &rows)?;
    repair_receipts(&plan, &driver, &rows, &saved)?;
    for proof in array(&driver, "puzzles")? {
        let chosen = usize::try_from(integer(proof, "chosen")?)?;
        assert!(chosen < array(proof, "answers")?.len());
        assert!(!array(proof, "sequence")?.is_empty());
    }
    assert!(!array(&driver, "puzzles")?.is_empty());
    captured_inputs(&trace, &journal)?;
    let mut snapshots = BTreeMap::<i32, Vec<&Value>>::new();
    for row in &journal {
        if row["kind"] != "control"
            || row["request"]["command"] != "snapshot"
            || row["response"]["status"] != "observed"
            || row["response"]["data"]["ready"] != true
        {
            continue;
        }
        let cycle = i32::try_from(integer(&row["response"]["data"], "cycle")?)?;
        snapshots.entry(cycle).or_default().push(row);
    }
    let retaliation_off_cycles: BTreeSet<i32> = journal
        .iter()
        .filter(|row| {
            row["kind"] == "postcondition" && row["name"] == RETALIATION_OFF_POSTCONDITION
        })
        .map(|row| Ok(i32::try_from(integer(row, "cycle")?)?))
        .collect::<anyhow::Result<_>>()?;
    let mut replayed_retaliation_off = BTreeSet::new();
    let frames = arrivals(&trace)?;
    let (server_players, server_npcs) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let observed_lives = driver["six_full_health"]
        .as_object()
        .context("six observed normal lives")?;
    let definitions: BTreeSet<_> = observed_lives
        .values()
        .map(|life| Ok(i32::try_from(integer(life, "definition")?)?))
        .collect::<anyhow::Result<_>>()?;
    let mut death_sequences = BTreeMap::new();
    for row in rows.iter().filter(|row| row["kind"] == "death") {
        let Some(definition) = row["targetDefinition"]
            .as_i64()
            .and_then(|id| i32::try_from(id).ok())
            .filter(|id| definitions.contains(id))
        else {
            continue;
        };
        let modes = array(&row["animation"], "modes")?;
        let sequence = i32::try_from(
            modes
                .first()
                .and_then(Value::as_i64)
                .context("observed ordinary death cue")?,
        )?;
        assert!(
            sequence >= ZERO
                && modes
                    .iter()
                    .all(|mode| mode.as_i64() == Some(i64::from(sequence)))
        );
        anyhow::ensure!(
            death_sequences.insert(definition, sequence).is_none(),
            "one source death cue per normal brother"
        );
    }
    assert_eq!(death_sequences.len(), ORIGINAL_BROTHERS);
    let mut replay = Replay::start(&trace)?;
    if observed_backend {
        assert_eq!(
            replay.io.take_written(),
            trace.bytes(b"OUT ", INITIAL_RECORDING_OUTPUT_CYCLE),
            "exact actual startup generated output; IOUT remains the drain reply"
        );
    }
    let mut done = FIRST_INDEX;
    let mut player_updates = FIRST_INDEX;
    let mut npc_updates = FIRST_INDEX;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    let mut seen = BTreeSet::new();
    let mut died = BTreeSet::new();
    let mut hit = false;
    let mut empty_bar = false;
    let mut hud = false;
    let mut puzzle = false;
    let mut models = false;
    let mut rewards = false;
    let mut all_slain = false;
    let mut reset = false;
    let mut native_food = false;
    let mut previous_food = None;
    let mut native_snapshots = FIRST_INDEX;
    let mut copied_maps = BTreeSet::new();
    let mut house_entries = BTreeSet::new();
    let mut house_exits = BTreeSet::new();
    let mut normal_after_copied = false;
    let mut quotes = [false; REQUIRED_REPAIR_OWNERS];
    let mut repairs_published = [false; REQUIRED_REPAIR_OWNERS];
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            gameplay(&out.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle{cycle}: every actual native gameplay packet, including retained UIEV"
        );
        for (opcode, _) in client_frames(&out.written)? {
            assert_ne!(
                opcode,
                crate::proto::client::CLIENT_CHEAT,
                "no runtime developer command"
            );
        }
        if retaliation_off_cycles.contains(&cycle) {
            let vars = replay
                .game()
                .runtime
                .feed
                .state
                .varps
                .as_ref()
                .context("retaliation postcondition PLAYER variables")?;
            assert_eq!(
                vars.get(varp::AUTO_RETALIATE_DISABLED.id())
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?,
                RETALIATION_OFF,
                "actual native retaliation OFF postcondition is independently replayed"
            );
            replayed_retaliation_off.insert(cycle);
        }
        let next = replay.processed(&frames);
        let fresh = &frames[done..next];
        assert!(
            fresh
                .iter()
                .filter(|frame| frame.opcode == sp::PLAYER_INFO)
                .count()
                <= ONE as usize
                && fresh
                    .iter()
                    .filter(|frame| frame.opcode == sp::NPC_INFO)
                    .count()
                    <= ONE as usize,
            "one applied entity tick; do not hide intermediate rosters"
        );
        let observed = observe(replay.game());
        for frame in fresh {
            if frame.opcode == sp::PLAYER_INFO {
                let expected = server_players
                    .get(player_updates)
                    .context("independent player publication")?;
                assert_eq!(
                    observed.local.as_ref().context("native local player")?.tile,
                    expected[..TILE_FIELDS],
                    "cycle{cycle}: exact player position"
                );
                player_updates += ONE as usize;
            }
            if frame.opcode == sp::NPC_INFO {
                let expected: BTreeMap<_, _> = server_npcs
                    .get(npc_updates)
                    .context("independent NPC publication")?
                    .iter()
                    .map(|&[index, identity, x, z, level]| {
                        (index as usize, (identity, [x, z, level]))
                    })
                    .collect();
                let actual: BTreeMap<_, _> = observed
                    .npcs
                    .iter()
                    .map(|(&index, (identity, actor))| (index, (*identity, actor.tile)))
                    .collect();
                assert_eq!(actual, expected, "cycle{cycle}: exact native NPC roster");
                npc_updates += ONE as usize;
            }
        }
        done = next;
        for row in snapshots.get(&cycle).into_iter().flatten() {
            let snapshot = &row["response"]["data"];
            native_snapshot(&mut replay, snapshot)?;
            native_snapshots += ONE as usize;
            let generation = snapshot["map"]["terrain_generation"]
                .as_u64()
                .context("native terrain generation")?;
            if !snapshot["region"].is_null() {
                copied_maps.insert(generation);
                if row["stage"] == "ordinary_house_entry" {
                    house_entries.insert(generation);
                }
            } else {
                normal_after_copied |= !copied_maps.is_empty();
                if row["stage"] == "ordinary_house_exit" {
                    house_exits.insert(generation);
                }
            }
        }
        for enemy in replay.game().runtime.feed.state.npcs.entities.values() {
            if !definitions.contains(&enemy.type_id) {
                continue;
            }
            seen.insert(enemy.type_id);
            if enemy.path.animation.main.id() == death_sequences[&enemy.type_id] {
                died.insert(enemy.type_id);
            }
            if let Some(combat) = &enemy.path.combat {
                hit |= combat.hits.iter().any(|row| row[HIT_DAMAGE_INDEX] > ZERO);
                empty_bar |= combat.bars.iter().any(|bar| {
                    bar.updates
                        .iter()
                        .any(|update| update[HEALTH_BAR_END] == ZERO)
                });
            }
        }
        hud |= visible_group(&mut replay, interface::BARROWS_STATUS_OVERLAY.id());
        let puzzle_now = visible_group(&mut replay, interface::BARROWS_PATTERN_CHOICES.id());
        puzzle |= puzzle_now;
        if puzzle_now {
            models |= puzzle_models(&mut replay, &driver)?;
        }
        for proof in array(&driver, "puzzles")? {
            if proof["cycle"].as_i64() == Some(i64::from(cycle)) {
                assert!(
                    !puzzle_now,
                    "actual successful answer closes the native model choices"
                );
                assert_eq!(
                    bit(&replay, i32::try_from(integer(proof, "door_bit")?)?)?,
                    ONE
                );
            }
        }
        let pending = inventory(&mut replay, inv::BARROWS_PENDING_REWARDS.id());
        rewards |= pending
            .iter()
            .any(|(identity, count)| *identity >= ZERO && *count > ZERO);
        for take in array(&driver, "withdrawals")? {
            if take["cycle"].as_i64() == Some(i64::from(cycle)) {
                let item = i32::try_from(integer(take, "item")?)?;
                let remaining: i64 = pending
                    .iter()
                    .filter(|(held, _)| *held == item)
                    .map(|(_, count)| i64::from(*count))
                    .sum();
                assert_eq!(remaining, integer(take, "after")?);
                assert_eq!(remaining, integer(take, "server_pending")?);
            }
        }
        let slain = BROTHER_BITS
            .iter()
            .map(|(_, id)| bit(&replay, *id))
            .collect::<anyhow::Result<Vec<_>>>()?;
        all_slain |= slain.iter().all(|value| *value == ONE)
            && bit(&replay, varbit::BARROWS_TOTAL_KILLS.id())? >= ORIGINAL_BROTHERS as i32;
        reset |= all_slain
            && slain.iter().all(|value| *value == ZERO)
            && bit(&replay, varbit::BARROWS_REWARD_OPENED.id())? == ZERO;
        let food: i32 = inventory(&mut replay, inv::BACKPACK.id())
            .iter()
            .filter(|(identity, _)| *identity == obj::SHARK.id())
            .map(|(_, count)| *count)
            .sum();
        if let Some(previous_food) = previous_food {
            native_food |= food < previous_food;
        }
        previous_food = Some(food);
        let texts = visible_texts(&mut replay);
        for (index, proof) in array(&driver, "repairs")?.iter().enumerate() {
            let price = integer(proof, "price")?;
            quotes[index] |= texts
                .iter()
                .any(|text| text == &format!("Repair this equipment for {price} coins?"));
            if proof["client"]["cycle"].as_i64() != Some(i64::from(cycle)) {
                continue;
            }
            let after = &proof["commit"]["after"];
            native_inventory_slots(&mut replay, inv::BACKPACK.id(), &after["backpack"])?;
            native_inventory_slots(&mut replay, inv::WORN_EQUIPMENT.id(), &after["worn"])?;
            let coins: i64 = inventory(&mut replay, inv::COIN_WALLET.id())
                .iter()
                .filter(|(item, _)| *item == obj::COINS.id())
                .map(|(_, count)| i64::from(*count))
                .sum();
            assert_eq!(
                coins,
                integer(after, "coins")?,
                "quoted payment's actual native wallet publication"
            );
            repairs_published[index] = true;
        }
    }
    assert_eq!(
        replayed_retaliation_off, retaliation_off_cycles,
        "every actual native retaliation OFF milestone belongs to a recorded replay cycle"
    );
    assert_eq!(replay_pings, recorded_pings, "original ICMP bytes");
    assert_eq!(seen, definitions, "native actors for all six brothers");
    assert_eq!(
        died, definitions,
        "native death sequences for all six brothers"
    );
    assert!(
        hit && empty_bar && hud && puzzle && models && rewards && all_slain && reset && native_food,
        "actual combat/UI/inventory/life consumers"
    );
    assert!(
        player_updates > FIRST_INDEX && npc_updates > FIRST_INDEX && native_snapshots > FIRST_INDEX
    );
    assert!(
        normal_after_copied && copied_maps.len() >= REQUIRED_REPAIR_OWNERS,
        "copied maps, ordinary exit and re-entry"
    );
    assert!(
        house_entries.len() >= REQUIRED_REPAIR_OWNERS
            && house_exits.len() >= REQUIRED_REPAIR_OWNERS,
        "ordinary house entry, exit and persisted re-entry"
    );
    assert!(
        quotes.into_iter().all(|seen| seen) && repairs_published.into_iter().all(|seen| seen),
        "both ordinary Bob/POH quotes and native saved repair publications"
    );
    assert!(
        bit(&replay, varbit::CURRENT_LIFE_POINTS.id())? > ZERO,
        "native LP consumer survives"
    );
    let initial_food = stack_count(&initial["account"]["backpack"], i64::from(obj::SHARK.id()))?;
    let final_food = stack_count(&saved["backpack"], i64::from(obj::SHARK.id()))?;
    assert_eq!(
        initial_food - final_food,
        i64::try_from(accepted_food)?,
        "all food removals have ordinary commit receipts"
    );
    native_inventory_slots(&mut replay, inv::BACKPACK.id(), &saved["backpack"])?;
    native_inventory_slots(&mut replay, inv::WORN_EQUIPMENT.id(), &saved["worn"])?;
    let coins: i64 = inventory(&mut replay, inv::COIN_WALLET.id())
        .iter()
        .filter(|(item, _)| *item == obj::COINS.id())
        .map(|(_, count)| i64::from(*count))
        .sum();
    assert_eq!(
        coins,
        integer(&saved, "coins")?,
        "actual claimed loot and both repairs reach the native wallet"
    );
    Ok(())
}
