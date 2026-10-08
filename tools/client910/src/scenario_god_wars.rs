//! Four recorded ordinary GWD camps, normal arena lives, loot, altar and rejoin.
use super::scenario_tests::ui;
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Replay, Trace,
    AUTHENTICATED_HEADLESS_BACKEND, INITIAL_RECORDING_OUTPUT_CYCLE, MONOTONIC_CLOCK_FORMAT,
    MONOTONIC_CLOCK_HEAD, OBSERVED_BACKEND_HEAD,
};
use crate::client_core::input_event::{InputEvent, RECORD_TAG};
use anyhow::{ensure, Context};
use rs910_symbols::{inv, loc, npc, obj, varbit, varp};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const FIXTURE_ROOT: &str = "fixtures/session-replay/god-wars";
const HEADLESS_ROUTE_STATUS: &str = "ordinary_headless_four_role_route_pass_requires_native_proof";
const HEADLESS_EXECUTION_MODE: &str = "headless_gwd_faction_route";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const RETALIATION_OFF: i32 = 1;
const RETALIATION_RECEIPT: &str = "transport_retaliation_native_result";
const ONE: usize = 1;
const FIRST: usize = 0;
const STACK_QUANTITY: usize = 1;
const HIT_DAMAGE: usize = 1;
const HEALTH_BAR_END: usize = 2;
const TILE_FIELDS: usize = 3;
const ROLE_COUNT: usize = 4;
const ENTRY_DEBITS: usize = 2;
const DATED_ENTRY_COUNT: i64 = 40;
const DECLARED_INITIAL_COUNT: i64 = 39;
const PRAYER_FINE_PER_GOD_ITEM: i64 = 100; // not a content id
const POSITIVE_ROPE_COUNT: i64 = 1;
const SHA256_HEX_BYTES: usize = 64;
const LOCATION_ID_HIGH_HALF_BITS: u32 = 32;
const BANDOS_GENERAL_LIFE: i64 = 40_000;
const BANDOS_GUARD_LIFE: i64 = 5475;
const ARMADYL_GENERAL_LIFE: i64 = 75_000;
const ARMADYL_GUARD_LIFE: i64 = 10_000;
const SARADOMIN_GENERAL_LIFE: i64 = 60_000;
const ZAMORAK_GENERAL_LIFE: i64 = 55_000;
const SARADOMIN_ZAMORAK_GUARD_LIFE: i64 = 7500;
const BANDOS_RECORDED_INPUT_SHA256: &str =
    "b1f7c590422f916d1e073ee33ced35ad03df18d82f539a72b0d98d352111eb8c";
const GWD_NORMAL_INPUT_SHA256: &str =
    "98e8352850d8a31ca458abf458b8da66a90baeec34564469e81a9fafb666ca18";
const ROPE_RESET_KILL_NAME: &str = "exit-camp:1";
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

struct Role {
    definition: i32,
    name: &'static str,
    maximum_life: i64,
}

struct Faction {
    name: &'static str,
    recorded_input_sha256: &'static str,
    roles: [Role; ROLE_COUNT],
    count_bit: i32,
    altar: i32,
}

struct Life {
    definition: i32,
    index: usize,
    attack_cycle: i32,
    death_cycle: i32,
}

fn integer(value: &Value, field: &str) -> anyhow::Result<i64> {
    value[field]
        .as_i64()
        .with_context(|| format!("{field}: {value}"))
}

fn array<'a>(value: &'a Value, field: &str) -> anyhow::Result<&'a [Value]> {
    value[field]
        .as_array()
        .map(Vec::as_slice)
        .with_context(|| format!("{field}: {value}"))
}

fn json(root: &Path, name: &str) -> anyhow::Result<Value> {
    serde_json::from_slice(&std::fs::read(root.join(name))?).with_context(|| name.to_string())
}

fn rows(root: &Path, name: &str) -> anyhow::Result<Vec<Value>> {
    let text = std::fs::read_to_string(root.join(name))?;
    ensure!(text.ends_with('\n'), "{name}: partial final evidence row");
    text.lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .with_context(|| name.to_string())
}

fn sha256(value: &Value) -> anyhow::Result<&str> {
    let hash = value.as_str().context("source SHA256")?;
    ensure!(
        hash.len() == SHA256_HEX_BYTES && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "source SHA256 shape"
    );
    Ok(hash)
}

fn player(row: &Value) -> anyhow::Result<&Value> {
    let players = array(row, "players")?;
    ensure!(players.len() == ONE, "one declared live account");
    Ok(&players[FIRST])
}

fn count(value: &Value, faction: &str) -> anyhow::Result<i64> {
    let matches: Vec<_> = array(value, "counts")?
        .iter()
        .filter(|row| row["faction"] == faction)
        .collect();
    ensure!(matches.len() == ONE, "unique faction count");
    integer(matches[FIRST], "value")
}

fn same_life(left: &Value, right: &Value) -> bool {
    ["kind", "id", "definition", "generation", "instance"]
        .iter()
        .all(|key| left[*key] == right[*key])
}

fn item_count(slots: &Value, identity: i64) -> anyhow::Result<i64> {
    slots
        .as_array()
        .context("saved slots")?
        .iter()
        .filter(|slot| slot[FIRST].as_i64() == Some(identity))
        .map(|slot| slot[STACK_QUANTITY].as_i64().context("slot quantity"))
        .sum()
}

fn camp_current_life(proof: &Value, combat: &[Value], journal: &[Value]) -> anyhow::Result<()> {
    // Older recordings retain their actual full-life publication. New ordinary
    // camp kills record the current positive life rather than relabel damage.
    let current = proof
        .get("initial_current_life")
        .unwrap_or(&proof["full_health"]);
    let health = integer(current, "hitpoints")?;
    let maximum = integer(current, "maximumLife")?;
    ensure!(
        health > i64::from(ZERO) && health <= maximum,
        "positive bounded camp life"
    );
    if proof.get("initial_current_life").is_some() {
        ensure!(
            integer(current, "configuredMaximumLife")? == maximum,
            "configured camp maximum"
        );
    } else {
        ensure!(
            health == maximum,
            "historical full-life witness remains exact"
        );
    }
    ensure!(
        current["visible"] == true && current["instance"].is_null(),
        "visible normal-region camp life"
    );
    ensure!(
        same_life(current, &proof["death"]["target"]),
        "same original camp life"
    );
    ensure!(
        integer(&proof["death"]["target"], "currentLife")? == i64::from(ZERO),
        "actual camp death"
    );
    ensure!(
        combat.iter().any(|row| row["kind"] == "state"
            && row["tick"] == current["tick"]
            && row["enemies"]
                .as_array()
                .is_some_and(
                    |enemies| enemies.iter().any(|enemy| same_life(enemy, current)
                        && enemy["hitpoints"] == current["hitpoints"]
                        && enemy["maximumLife"] == current["maximumLife"]
                        && enemy["visible"] == true)
                )),
        "camp current life is an unchanged passive publication"
    );
    ensure!(
        current["configuredRole"] == false,
        "camp witness is not an arena role"
    );
    let attack_stage = format!(
        "fight:{}",
        proof["name"].as_str().context("camp kill name")?
    );
    ensure!(
        journal.iter().any(|row| row["kind"] == "control"
            && row["stage"] == attack_stage
            && row["request"]["command"] == "action"
            && row["response"]["status"] == "accepted"
            && row["request"]["action"]["kind"] == "npc"
            && row["request"]["action"]["operation"] == "Attack"
            && row["request"]["action"]["index"] == current["id"]
            && row["request"]["action"]["definition"] == current["definition"]),
        "actual accepted camp Attack"
    );
    ensure!(
        combat
            .iter()
            .filter(|row| row["kind"] == "landed"
                && row["killed"] == true
                && same_life(&row["target"], current)
                && row["tick"] == proof["death"]["tick"]
                && same_life(&row["source"], &proof["death"]["source"])
                && integer(row, "actualDamage").is_ok_and(|damage| damage > i64::from(ZERO)))
            .count()
            == ONE,
        "one actual killing impact from the credited player"
    );
    let hide = integer(&proof["death"], "hideTick")?;
    ensure!(
        hide > integer(&proof["death"], "tick")?,
        "ordinary camp death timer"
    );
    ensure!(
        combat.iter().any(|row| row["kind"] == "state"
            && row["phase"] == "npcs"
            && integer(row, "tick").is_ok_and(|tick| tick >= hide)
            && row["enemies"].as_array().is_some_and(|enemies| !enemies
                .iter()
                .any(|enemy| same_life(enemy, current) && enemy["visible"] == true))),
        "same camp life hides normally"
    );
    Ok(())
}

fn server_contract(
    faction: &Faction,
    plan: &Value,
    initial: &Value,
    journal: &[Value],
    combat: &[Value],
    saved: &Value,
    rope_before: &Value,
) -> anyhow::Result<Vec<Life>> {
    assert_eq!(plan["faction"], faction.name);
    assert_eq!(initial["plan"], *plan, "declared pre-login fixture plan");
    assert_eq!(integer(plan, "initialCount")?, DECLARED_INITIAL_COUNT);
    assert!(
        array(&initial["account"], "worn")?
            .iter()
            .all(|slot| slot[FIRST]
                .as_i64()
                .is_some_and(|item| item < i64::from(ZERO))),
        "gear must be equipped through ordinary input"
    );
    assert_eq!(
        sha256(&initial["source"]["inputs"]["instances/gwd.json"])?,
        faction.recorded_input_sha256
    );
    for hash in initial["source"]["inputs"]
        .as_object()
        .context("captured input hashes")?
        .values()
    {
        sha256(hash)?;
    }
    assert!(!journal.iter().any(|row| row["kind"] == "failure"));
    let completed: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == "complete")
        .collect();
    ensure!(completed.len() == ONE, "one actual semantic completion");
    let complete = completed[FIRST];
    assert_eq!(complete["faction"], faction.name);
    assert_eq!(complete["ordinaryRandom"], true);
    let final_states: Vec<_> = combat.iter().filter(|row| row["kind"] == "state").collect();
    let first_player = player(final_states.first().context("initial passive state")?)?;
    let final_player = player(final_states.last().context("closed passive state")?)?;
    let pid = integer(first_player, "pid")?;
    let generation = integer(first_player, "generation")?;
    assert_eq!(count(first_player, faction.name)?, DECLARED_INITIAL_COUNT);
    for state in &final_states {
        let actual = player(state)?;
        assert_eq!(integer(actual, "pid")?, pid);
        assert_eq!(
            integer(actual, "generation")?,
            generation,
            "same surviving player life throughout rejoin"
        );
        assert!(
            integer(actual, "life")? > i64::from(ZERO),
            "no death/rescue path"
        );
    }
    let fortieth: Vec<_> = journal
        .iter()
        .filter(|row| {
            row["kind"] == "credited_death"
                && row["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("camp:"))
        })
        .collect();
    ensure!(
        fortieth.len() == ONE,
        "the first camp death earns count forty"
    );
    let camp = fortieth[FIRST];
    assert!(combat.contains(&camp["death"]));
    camp_current_life(camp, combat, journal)?;
    assert_eq!(camp["death"]["source"]["id"], pid);
    assert_eq!(camp["death"]["source"]["generation"], generation);
    assert!(
        final_states.iter().any(|row| integer(row, "tick")
            .is_ok_and(|tick| tick >= integer(&camp["death"], "tick").unwrap_or(i64::MAX))
            && player(row)
                .is_ok_and(|actual| count(actual, faction.name)
                    .is_ok_and(|value| value == DATED_ENTRY_COUNT))),
        "fortieth count published by normal camp death"
    );
    let mut lives = Vec::new();
    for role in &faction.roles {
        let proofs: Vec<_> = journal
            .iter()
            .filter(|row| row["kind"] == "credited_death" && row["name"] == role.name)
            .collect();
        ensure!(
            proofs.len() == ONE,
            "one credited original normal role: {}",
            role.name
        );
        let proof = proofs[FIRST];
        let full = &proof["full_health"];
        let death = &proof["death"];
        assert_eq!(integer(full, "definition")?, i64::from(role.definition));
        assert_eq!(integer(full, "hitpoints")?, role.maximum_life);
        assert_eq!(integer(full, "maximumLife")?, role.maximum_life);
        assert_eq!(full["configuredRole"], true);
        assert!(
            final_states.iter().any(|row| integer(row, "tick")
                .is_ok_and(|tick| tick == integer(full, "tick").unwrap_or(i64::MIN))
                && row["enemies"]
                    .as_array()
                    .is_some_and(|enemies| enemies
                        .iter()
                        .any(|enemy| same_life(enemy, full)
                            && enemy["hitpoints"] == role.maximum_life))),
            "full HP came from actual configured current-life state"
        );
        assert!(
            combat.contains(death),
            "copied death is an original passive receipt"
        );
        assert_eq!(death["kind"], "death");
        assert!(same_life(&death["target"], full));
        assert_eq!(integer(&death["target"], "currentLife")?, i64::from(ZERO));
        assert_eq!(death["source"]["kind"], "player");
        assert_eq!(integer(&death["source"], "id")?, pid);
        assert_eq!(integer(&death["source"], "generation")?, generation);
        let killing: Vec<_> = combat
            .iter()
            .filter(|row| {
                row["kind"] == "landed"
                    && row["killed"] == true
                    && same_life(&row["target"], full)
                    && row["tick"] == death["tick"]
                    && row["source"]["id"] == pid
                    && row["source"]["generation"] == generation
            })
            .collect();
        ensure!(
            killing.len() == ONE,
            "one accepted killing impact per normal life"
        );
        assert!(integer(killing[FIRST], "actualDamage")? > i64::from(ZERO));
        assert!(integer(killing[FIRST], "actualDamage")? <= role.maximum_life);
        let hide_tick = integer(death, "hideTick")?;
        assert!(hide_tick > integer(death, "tick")?);
        assert!(
            final_states.iter().any(|row| row["phase"] == "npcs"
                && integer(row, "tick").is_ok_and(|tick| tick >= hide_tick)
                && row["enemies"].as_array().is_some_and(|enemies| !enemies
                    .iter()
                    .any(|enemy| same_life(enemy, full) && enemy["visible"] == true))),
            "ordinary body removal of this generation"
        );
        let attack = journal
            .iter()
            .find(|row| {
                row["kind"] == "control"
                    && row["request"]["command"] == "action"
                    && row["response"]["status"] == "accepted"
                    && row["request"]["action"]["kind"] == "npc"
                    && row["request"]["action"]["operation"] == "Attack"
                    && row["request"]["action"]["index"] == full["id"]
                    && row["request"]["action"]["definition"] == full["definition"]
            })
            .context("actual native Attack of original role slot")?;
        let attack_cycle = i32::try_from(integer(&attack["response"], "cycle")?)?;
        let death_cycle = i32::try_from(integer(proof, "cycle")?)?;
        assert!(attack_cycle < death_cycle);
        lives.push(Life {
            definition: role.definition,
            index: usize::try_from(integer(full, "id")?)?,
            attack_cycle,
            death_cycle,
        });
    }
    let rejected = journal
        .iter()
        .filter(|row| row["kind"] == "crossing" && row["admitted"] == false)
        .count();
    assert_eq!(rejected, ONE, "real low-count entry refusal");
    let mut previous = None;
    let mut debits = Vec::new();
    for state in &final_states {
        let actual = player(state)?;
        let next = count(actual, faction.name)?;
        if let Some((before, old_room)) = previous {
            if next < before
                && !actual["room"].is_null()
                && actual["membership"] == true
                && old_room != &actual["room"]
            {
                assert_eq!(
                    before - next,
                    DATED_ENTRY_COUNT,
                    "one entry debit, never a diagnostic refill"
                );
                debits.push((before, next));
            }
        }
        previous = Some((next, &actual["room"]));
    }
    assert_eq!(debits.len(), ENTRY_DEBITS);
    let rejoined: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == "arena_rejoined")
        .collect();
    ensure!(rejoined.len() == ONE, "one real second arena entry");
    let rejoin = rejoined[FIRST];
    assert_eq!(integer(rejoin, "debit")?, DATED_ENTRY_COUNT);
    assert_eq!(
        integer(rejoin, "before_debit")? - integer(rejoin, "after_debit")?,
        DATED_ENTRY_COUNT
    );
    assert_eq!(rejoin["passive"]["membership"], true);
    let earned: Vec<_> = journal
        .iter()
        .filter(|row| {
            row["kind"] == "rejoin_count_earned"
                && row["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("rejoin-camp:"))
        })
        .collect();
    assert!(!earned.is_empty());
    assert_eq!(
        integer(rejoin, "normal_credited_kills")?,
        i64::try_from(earned.len())?
    );
    for proof in earned {
        assert_eq!(
            integer(proof, "after")? - integer(proof, "before")?,
            i64::try_from(ONE)?
        );
        assert!(combat.contains(&proof["death"]));
        let life_proofs: Vec<_> = journal
            .iter()
            .filter(|row| {
                row["kind"] == "credited_death"
                    && row["name"] == proof["name"]
                    && row["death"] == proof["death"]
            })
            .collect();
        ensure!(
            life_proofs.len() == ONE,
            "one actual current-life replenishment witness"
        );
        camp_current_life(life_proofs[FIRST], combat, journal)?;
        assert_eq!(proof["death"]["source"]["id"], pid);
        assert_eq!(proof["death"]["source"]["generation"], generation);
    }
    let rope_earned: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == "rejoin_count_earned" && row["name"] == ROPE_RESET_KILL_NAME)
        .collect();
    ensure!(
        rope_earned.len() == ONE,
        "one additional ordinary camp kill after the second altar exit"
    );
    let rope_earned = rope_earned[FIRST];
    assert_eq!(integer(rope_earned, "before")?, i64::from(ZERO));
    assert_eq!(integer(rope_earned, "after")?, POSITIVE_ROPE_COUNT);
    assert!(combat.contains(&rope_earned["death"]));
    assert_eq!(rope_earned["death"]["source"]["id"], pid);
    assert_eq!(rope_earned["death"]["source"]["generation"], generation);
    let exit_life: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == "credited_death" && row["name"] == ROPE_RESET_KILL_NAME)
        .collect();
    ensure!(
        exit_life.len() == ONE,
        "original current camp life and actual death"
    );
    let exit_life = exit_life[FIRST];
    camp_current_life(exit_life, combat, journal)?;
    assert_eq!(exit_life["death"], rope_earned["death"]);
    let exit_tick = integer(&rope_earned["death"], "tick")?;
    assert!(
        final_states.iter().any(
            |row| integer(row, "tick").is_ok_and(|tick| tick >= exit_tick)
                && player(row).is_ok_and(|actual| actual["instance"].is_null()
                    && count(actual, faction.name).is_ok_and(|value| value == POSITIVE_ROPE_COUNT)
                    && actual["savedVarps"] == rope_before["savedVarps"])
        ),
        "independent positive count and real pre-rope saved backings agree"
    );
    let altar_exits: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == "altar_exit")
        .collect();
    ensure!(
        altar_exits.len() == ENTRY_DEBITS,
        "two actual normal altar exits"
    );
    assert!(
        integer(&altar_exits[ENTRY_DEBITS - ONE]["state"], "cycle")?
            < integer(&rope_earned["state"], "cycle")?,
        "the positive witness follows the second exit"
    );
    let rope_actions: Vec<_> = journal
        .iter()
        .filter(|row| {
            row["kind"] == "control"
                && row["request"]["command"] == "action"
                && row["response"]["status"] == "accepted"
                && row["request"]["action"]["kind"] == "loc"
                && row["request"]["action"]["definition"] == loc::GWD_SURFACE_ROPE.id()
        })
        .collect();
    ensure!(
        rope_actions.len() == ONE,
        "one real surface rope retained action"
    );
    assert!(
        integer(&rope_actions[FIRST]["response"], "cycle")?
            > integer(&rope_earned["state"], "cycle")?
    );
    assert!(
        integer(&complete["state"], "cycle")? > integer(&rope_actions[FIRST]["response"], "cycle")?
    );
    let taken: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == "taken")
        .collect();
    assert_eq!(taken.len(), ONE);
    let taken = taken[FIRST];
    let quantity = integer(&taken["observed"], "count")?;
    assert!(quantity > i64::from(ZERO));
    assert_eq!(
        taken["observed"]["definition"],
        taken["independent"]["item"]
    );
    assert_eq!(
        integer(taken, "after_count")? - integer(taken, "before_count")?,
        quantity
    );
    let token = &taken["independent"]["token"];
    let general = journal
        .iter()
        .find(|row| row["kind"] == "credited_death" && row["name"] == faction.roles[FIRST].name)
        .context("general's actual death")?;
    for coordinate in ["x", "z", "level"] {
        assert_eq!(
            taken["independent"][coordinate],
            general["death"]["target"][coordinate]
        );
    }
    assert!(
        !final_states.iter().any(|row| integer(row, "tick")
            .is_ok_and(|tick| tick < integer(&general["death"], "hideTick").unwrap_or(i64::MAX))
            && player(row).is_ok_and(|actual| actual["ground"]
                .as_array()
                .is_some_and(|ground| ground.iter().any(|drop| drop["token"] == *token)))),
        "this loot token was not an earlier stack"
    );
    assert!(final_states
        .iter()
        .any(|row| player(row).is_ok_and(|actual| actual["ground"]
            .as_array()
            .is_some_and(|ground| ground.contains(&taken["independent"])))));
    assert!(!array(final_player, "ground")?
        .iter()
        .any(|row| row["token"] == *token));
    assert!(
        item_count(
            &final_player["backpack"],
            integer(&taken["independent"], "item")?
        )? >= integer(taken, "after_count")?,
        "actual loot retained in ordinary saved backpack"
    );
    let altar: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == "altar_restored")
        .collect();
    ensure!(altar.len() == ONE, "real Pray restoration");
    let altar = altar[FIRST];
    let bonus = integer(altar, "matching_worn_count")? * PRAYER_FINE_PER_GOD_ITEM;
    assert_eq!(integer(altar, "allowance")?, bonus);
    assert_eq!(
        integer(altar, "expected")?,
        integer(&altar["passive"], "maximumPrayerFine")? + bonus
    );
    assert_eq!(altar["passive"]["prayerFine"], altar["expected"]);
    assert_eq!(altar["passive"]["prayerAllowanceFine"], altar["allowance"]);
    assert_eq!(
        journal
            .iter()
            .filter(|row| row["kind"] == "altar_exit")
            .count(),
        ENTRY_DEBITS
    );
    for row in journal.iter().filter(|row| row["kind"] == "altar_exit") {
        assert!(row["passive"]["instance"].is_null());
        assert_eq!(
            count(&row["passive"], faction.name)?,
            integer(row, "count")?
        );
    }
    let food: Vec<_> = journal.iter().filter(|row| row["kind"] == "food").collect();
    let commits: Vec<_> = combat
        .iter()
        .filter(|row| row["kind"] == "food_commit")
        .collect();
    assert_eq!(
        food.len(),
        commits.len(),
        "all actual food transactions have native driver evidence"
    );
    assert_eq!(array(complete, "food")?.len(), food.len());
    for proof in food {
        let commit = &proof["food_commit"];
        assert!(combat.contains(commit));
        assert_eq!(integer(commit, "pid")?, pid);
        assert_eq!(commit["item"], obj::SHARK.id());
        assert_eq!(commit["beforeItem"], obj::SHARK.id());
        assert_eq!(integer(commit, "beforeCount")?, i64::try_from(ONE)?);
        assert_eq!(integer(commit, "afterCount")?, i64::from(ZERO));
        assert!(integer(commit, "healed")? > i64::from(ZERO));
        assert_eq!(
            integer(commit, "lifeAfter")? - integer(commit, "lifeBefore")?,
            integer(commit, "healed")?
        );
        assert!(integer(commit, "lifeAfter")? <= integer(commit, "maximumLife")?);
        assert!(
            journal.iter().any(|row| row["kind"] == "control"
                && row["request"]["command"] == "action"
                && row["response"]["status"] == "accepted"
                && row["request"]["action"]["kind"] == "ui"
                && row["request"]["action"]["target"] == proof["target"]
                && row["request"]["action"]["expected_object"] == obj::SHARK.id()),
            "normal retained Eat admission for this actual commit"
        );
    }
    for row in array(&complete["passive"], "counts")? {
        assert_eq!(integer(row, "value")?, i64::from(ZERO));
    }
    assert!(complete["passive"]["instance"].is_null());
    assert_eq!(
        saved["savedVarps"], final_player["savedVarps"],
        "normally saved counters/preferences after closure"
    );
    assert_eq!(saved["backpack"], final_player["backpack"]);
    assert_eq!(saved["worn"], final_player["worn"]);
    assert_eq!(saved["resources"]["life"], final_player["life"]);
    assert_eq!(saved["resources"]["prayerFine"], final_player["prayerFine"]);
    assert_eq!(
        saved["resources"]["prayerOverboostFine"]
            .as_i64()
            .unwrap_or(i64::from(ZERO)),
        integer(final_player, "prayerAllowanceFine")?
    );
    Ok(lives)
}

pub(super) fn input_matches(
    action: &Value,
    map: &Value,
    placed_location: Option<i64>,
    event: &InputEvent,
) -> bool {
    let same_tile = |choice: &crate::client_core::input_event::MenuChoice| {
        action["x"]
            .as_i64()
            .zip(map["base_x"].as_i64())
            .is_some_and(|(x, base)| i64::from(choice.tile_x) == x - base)
            && action["z"]
                .as_i64()
                .zip(map["base_z"].as_i64())
                .is_some_and(|(z, base)| i64::from(choice.tile_z) == z - base)
    };
    let label = |choice: &crate::client_core::input_event::MenuChoice| {
        choice.enabled
            && action["operation"]
                .as_str()
                .is_some_and(|operation| choice.op.eq_ignore_ascii_case(operation))
    };
    match (action["kind"].as_str(), event) {
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
        (Some("walk"), InputEvent::Menu { choice, .. }) => {
            choice.enabled
                && choice.action == crate::ui_scene_options::WALK_ACTION
                && same_tile(choice)
        }
        (Some("loc"), InputEvent::Menu { choice, .. }) => {
            label(choice)
                && same_tile(choice)
                && placed_location
                    == Some((choice.entity_id >> LOCATION_ID_HIGH_HALF_BITS) & i64::from(i32::MAX))
        }
        (Some("npc"), InputEvent::Menu { choice, .. }) => {
            label(choice) && action["index"].as_i64() == Some(choice.entity_id)
        }
        (Some("object"), InputEvent::Menu { choice, .. }) => {
            label(choice)
                && same_tile(choice)
                && action["definition"].as_i64() == Some(choice.entity_id)
                && action["stack_index"].as_i64() == Some(choice.sub_id)
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

fn retained_inputs(faction: &Faction, trace: &Trace, journal: &[Value]) -> anyhow::Result<()> {
    assert!(!trace.head.iter().any(|(key, _)| key == "server_command"));
    for name in SCRIPTED_INPUT_VARIABLES {
        assert!(
            trace.env().get(*name).is_none_or(String::is_empty),
            "no timed injector {name}"
        );
    }
    let mut events = Vec::new();
    let mut prior = None;
    for record in trace
        .records
        .iter()
        .filter(|record| record.tag == *RECORD_TAG)
    {
        if let Some(previous) = prior {
            assert!(record.cycle >= previous, "actual UIEV file order");
        }
        prior = Some(record.cycle);
        let event = InputEvent::decode(&record.bytes)?;
        if matches!(
            event,
            InputEvent::Component { .. } | InputEvent::Menu { .. } | InputEvent::Key { .. }
        ) {
            events.push((record.cycle, event));
        }
    }
    let accepted: Vec<_> = journal
        .iter()
        .filter(|row| {
            row["kind"] == "control"
                && row["request"]["command"] == "action"
                && row["response"]["status"] == "accepted"
        })
        .collect();
    assert_eq!(
        events.len(),
        accepted.len(),
        "every semantic retained event has its real accepted action"
    );
    assert!(!events.is_empty());
    let mut kinds = BTreeSet::new();
    let mut attacks = BTreeSet::new();
    let mut altar_pray = false;
    let mut altar_exit = false;
    for ((cycle, event), receipt) in events.iter().zip(accepted) {
        assert_eq!(i64::from(*cycle), integer(&receipt["response"], "cycle")?);
        assert!(
            *cycle < trace.last_cycle(),
            "input needs a later consumed logic cycle"
        );
        let request = &receipt["request"];
        assert_eq!(receipt["response"]["data"]["map"], request["map"]);
        let action = &request["action"];
        let kind = action["kind"].as_str().context("native action kind")?;
        let placed_location = if kind == "loc" {
            let receipt_index = journal
                .iter()
                .position(|row| std::ptr::eq(row, receipt))
                .context("original journal action")?;
            let scanned = journal[..receipt_index]
                .iter()
                .rev()
                .filter(|row| {
                    row["kind"] == "control"
                        && row["request"]["command"] == "scan_locs"
                        && row["response"]["status"] == "observed"
                        && row["response"]["data"]["map"] == request["map"]
                })
                .find_map(|row| {
                    row["response"]["data"]["scan"]["entries"]
                        .as_array()
                        .and_then(|entries| {
                            entries.iter().find(|entry| {
                                ["definition", "x", "z", "level", "shape", "angle"]
                                    .iter()
                                    .all(|field| entry[*field] == action[*field])
                            })
                        })
                });
            Some(integer(
                scanned.context("native loc placement and resolved-definition scan")?,
                "base_definition",
            )?)
        } else {
            None
        };
        assert!(
            input_matches(action, &request["map"], placed_location, event),
            "actual admitted event matches immutable menu/UI identity"
        );
        kinds.insert(kind);
        if kind == "npc" && action["operation"] == "Attack" {
            attacks.insert(integer(action, "definition")?);
        }
        if kind == "loc" && action["definition"] == faction.altar {
            altar_pray |= action["operation"] == "Pray-at";
            altar_exit |= action["operation"] == "Teleport";
        }
    }
    assert!(["walk", "loc", "npc", "ui", "object"]
        .iter()
        .all(|kind| kinds.contains(kind)));
    for role in &faction.roles {
        assert!(attacks.contains(&i64::from(role.definition)));
    }
    assert!(altar_pray && altar_exit);
    Ok(())
}

fn inventory(replay: &mut Replay, identity: i32) -> Vec<(i32, i32)> {
    ui(replay)
        .engine
        .inv_cache
        .inventory(identity, false)
        .map(|state| {
            state
                .obj_ids
                .iter()
                .copied()
                .zip(state.counts.iter().copied())
                .collect()
        })
        .unwrap_or_default()
}

fn bit(replay: &Replay, identity: i32) -> anyhow::Result<i32> {
    replay
        .game()
        .varbit_value(u16::try_from(identity)?)
        .map_err(|error| anyhow::anyhow!("{error:?}"))
}

fn saved_bit(replay: &Replay, projection: &Value, identity: i32) -> anyhow::Result<i32> {
    let definition = replay
        .game()
        .inputs
        .bits
        .get(identity, false)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let binding = definition
        .binding
        .as_ref()
        .context("bound actual native count definition")?;
    let matching: Vec<_> = array(projection, "savedVarps")?
        .iter()
        .filter(|row| row[FIRST].as_i64() == Some(i64::from(binding.id)))
        .collect();
    ensure!(matching.len() == ONE, "exact saved native count backing");
    let value = i32::try_from(
        matching[FIRST][STACK_QUANTITY]
            .as_i64()
            .context("saved backing integer")?,
    )?;
    definition
        .get(value)
        .map_err(|error| anyhow::anyhow!("{error:?}"))
}

fn native_snapshot(replay: &mut Replay, state: &Value) -> anyhow::Result<()> {
    let game = replay.game();
    let map = &game.runtime.map;
    assert!(
        state["ready"] == true
            && game.runtime.map_request.is_none()
            && game.runtime.terrain.is_some()
    );
    assert_eq!(state["map"]["base_x"], map.base_x);
    assert_eq!(state["map"]["base_z"], map.base_z);
    assert_eq!(
        state["map"]["level"],
        game.runtime.feed.state.players.current_level
    );
    assert_eq!(state["map"]["width"], serde_json::json!(map.width));
    assert_eq!(state["map"]["height"], serde_json::json!(map.height));
    assert_eq!(
        state["map"]["terrain_generation"],
        serde_json::json!(game.runtime.terrain_generation)
    );
    assert!(
        state["region"].is_null() && game.runtime.installed_region.is_none(),
        "ordinary public GWD cache map"
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
    assert_eq!(state["player"]["fine_x"], local.fine_x);
    assert_eq!(state["player"]["fine_z"], local.fine_z);
    assert_eq!(
        state["player"]["route_length"],
        serde_json::json!(local.route_length)
    );
    for queried in array(state, "varps")? {
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
    for queried in array(state, "varbits")? {
        if let Some(value) = queried["value"].as_i64() {
            assert_eq!(
                i64::from(bit(replay, i32::try_from(integer(queried, "id")?)?)?),
                value
            );
        }
    }
    for queried in array(state, "inventories")? {
        if queried["value"].is_null() {
            continue;
        }
        assert_eq!(queried["value"]["truncated"], false);
        let ids = array(&queried["value"], "items")?;
        let counts = array(&queried["value"], "counts")?;
        assert_eq!(ids.len(), counts.len());
        let expected = ids
            .iter()
            .zip(counts)
            .map(|(item, count)| {
                Ok((
                    i32::try_from(item.as_i64().context("inventory identity")?)?,
                    i32::try_from(count.as_i64().context("inventory count")?)?,
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        assert_eq!(
            inventory(replay, i32::try_from(integer(queried, "id")?)?),
            expected
        );
    }
    Ok(())
}

fn replay_faction(faction: Faction) -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let root = rs910_core::test_support::client_dir()
        .join(FIXTURE_ROOT)
        .join(faction.name);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let observed_backend = trace.head.iter().any(|(key, value)| {
        key == OBSERVED_BACKEND_HEAD && value == AUTHENTICATED_HEADLESS_BACKEND
    });
    if observed_backend {
        let execution = json(&root, "execution.json")?;
        assert_eq!(execution["status"], HEADLESS_ROUTE_STATUS);
        assert_eq!(execution["executionMode"], HEADLESS_EXECUTION_MODE);
        assert_eq!(
            execution["recordingBackend"],
            AUTHENTICATED_HEADLESS_BACKEND
        );
        assert_eq!(execution["monotonicClock"], MONOTONIC_CLOCK_FORMAT);
        assert_eq!(trace.head(MONOTONIC_CLOCK_HEAD)?, MONOTONIC_CLOCK_FORMAT);
        assert_eq!(execution["faction"], faction.name);
        assert_eq!(
            execution["rendered"], false,
            "no rendered or foreground proof"
        );
    }
    let plan = json(&root, "plan.json")?;
    let initial = json(&root, "initial-fixture.json")?;
    let saved = json(&root, "final-saved-state.json")?;
    let rope_before = json(&root, "rope-before-saved-state.json")?;
    let combat = rows(&root, "combat-receipts.jsonl")?;
    let journal = rows(&root, "driver-journal.jsonl")?;
    let lives = server_contract(
        &faction,
        &plan,
        &initial,
        &journal,
        &combat,
        &saved,
        &rope_before,
    )?;
    retained_inputs(&faction, &trace, &journal)?;
    let frames = arrivals(&trace)?;
    let (players, npcs) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let mut snapshots = BTreeMap::<i32, Vec<&Value>>::new();
    for row in journal.iter().filter(|row| {
        row["kind"] == "control"
            && row["request"]["command"] == "snapshot"
            && row["response"]["status"] == "observed"
            && row["response"]["data"]["ready"] == true
    }) {
        let state = &row["response"]["data"];
        snapshots
            .entry(i32::try_from(integer(state, "cycle")?)?)
            .or_default()
            .push(state);
    }
    ensure!(!snapshots.is_empty(), "real native observations");
    let required_snapshots: usize = snapshots.values().map(Vec::len).sum();
    for row in journal.iter().filter(|row| {
        matches!(
            row["kind"].as_str(),
            Some("altar_restored" | "arena_rejoined" | "altar_exit" | "complete")
        )
    }) {
        let cycle = i32::try_from(integer(&row["state"], "cycle")?)?;
        ensure!(
            (FIRST_CYCLE..=trace.last_cycle()).contains(&cycle),
            "semantic checkpoint must lie in actual recorded logic prefix"
        );
    }
    for row in journal
        .iter()
        .filter(|row| row["kind"] == "rejoin_count_earned" && row["name"] == ROPE_RESET_KILL_NAME)
    {
        let cycle = i32::try_from(integer(&row["state"], "cycle")?)?;
        ensure!(
            (FIRST_CYCLE..=trace.last_cycle()).contains(&cycle),
            "positive saved/native witness lies inside the actual recorded prefix"
        );
    }
    let retaliation_receipts: Vec<_> = journal
        .iter()
        .filter(|row| row["kind"] == RETALIATION_RECEIPT)
        .collect();
    ensure!(
        !retaliation_receipts.is_empty(),
        "actual native and passive transport retaliation OFF"
    );
    assert_eq!(
        integer(&plan["toolbar"], "retaliationVarp")?,
        i64::from(varp::AUTO_RETALIATE_DISABLED.id())
    );
    for row in &retaliation_receipts {
        let cycle = i32::try_from(integer(row, "cycle")?)?;
        ensure!(
            (FIRST_CYCLE..=trace.last_cycle()).contains(&cycle),
            "retaliation receipt must lie in the actual recorded prefix"
        );
        assert_eq!(integer(row, "after")?, i64::from(RETALIATION_OFF));
        assert_eq!(
            integer(&row["passive"], "autoRetaliateDisabled")?,
            i64::from(RETALIATION_OFF)
        );
    }
    let mut checked_retaliation_receipts = FIRST;
    let mut replay = Replay::start(&trace)?;
    if observed_backend {
        assert_eq!(
            replay.io.take_written(),
            trace.bytes(b"OUT ", INITIAL_RECORDING_OUTPUT_CYCLE),
            "exact actual startup generated output; IOUT remains the drain reply"
        );
    }
    assert_eq!(
        saved_bit(&replay, &rope_before, faction.count_bit)?,
        i32::try_from(ONE)?,
        "real durable positive count before rope"
    );
    for identity in [
        varbit::GWD_BANDOS_KILLS.id(),
        varbit::GWD_ARMADYL_KILLS.id(),
        varbit::GWD_SARADOMIN_KILLS.id(),
        varbit::GWD_ZAMORAK_KILLS.id(),
    ] {
        assert_eq!(
            saved_bit(&replay, &saved, identity)?,
            ZERO,
            "closed account durable rope reset"
        );
    }
    let mut done = FIRST;
    let mut player_updates = FIRST;
    let mut npc_updates = FIRST;
    let mut checked_snapshots = FIRST;
    let mut seen = BTreeSet::new();
    let mut hit = BTreeSet::new();
    let mut zero_bar = BTreeSet::new();
    let mut removed = BTreeSet::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        let expected = trace.bytes(b"OUT ", cycle);
        assert_eq!(
            mask_wall_clock(&output.written)?,
            mask_wall_clock(&expected)?,
            "cycle{cycle}: all normal client packets in original order"
        );
        for bytes in [&output.written, &expected] {
            for (opcode, _) in client_frames(bytes)? {
                assert_ne!(opcode, cp::CLIENT_CHEAT, "no developer commands");
            }
        }
        for row in retaliation_receipts
            .iter()
            .filter(|row| row["cycle"].as_i64() == Some(i64::from(cycle)))
        {
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
                "actual native retaliation OFF is independently replayed"
            );
            assert_eq!(row["after"], serde_json::json!(RETALIATION_OFF));
            checked_retaliation_receipts += ONE;
        }
        let next = replay.processed(&frames);
        let fresh = &frames[done..next];
        assert!(
            fresh
                .iter()
                .filter(|frame| frame.opcode == sp::PLAYER_INFO)
                .count()
                <= ONE
                && fresh
                    .iter()
                    .filter(|frame| frame.opcode == sp::NPC_INFO)
                    .count()
                    <= ONE,
            "one applied entity tick: never hide an intermediate roster"
        );
        let observed = observe(replay.game());
        for frame in fresh {
            if frame.opcode == sp::PLAYER_INFO {
                let expected = players
                    .get(player_updates)
                    .context("independent player packet row")?;
                assert_eq!(
                    observed.local.as_ref().context("native local player")?.tile,
                    expected[..TILE_FIELDS]
                );
                player_updates += ONE;
            }
            if frame.opcode == sp::NPC_INFO {
                let expected: BTreeMap<_, _> = npcs
                    .get(npc_updates)
                    .context("independent NPC packet row")?
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
                assert_eq!(
                    actual, expected,
                    "cycle{cycle}: native identity and location for every applied NPC packet"
                );
                npc_updates += ONE;
            }
        }
        done = next;
        for state in snapshots.get(&cycle).into_iter().flatten() {
            native_snapshot(&mut replay, state)?;
            checked_snapshots += ONE;
        }
        for row in journal.iter().filter(|row| {
            row["kind"] == "rejoin_count_earned"
                && row["name"] == ROPE_RESET_KILL_NAME
                && row["state"]["cycle"].as_i64() == Some(i64::from(cycle))
        }) {
            native_snapshot(&mut replay, &row["state"])?;
            assert_eq!(
                bit(&replay, faction.count_bit)?,
                i32::try_from(ONE)?,
                "actual native positive count before the rope action"
            );
        }
        for life in &lives {
            if cycle < life.attack_cycle || removed.contains(&life.definition) {
                continue;
            }
            if let Some(enemy) = replay
                .game()
                .runtime
                .feed
                .state
                .npcs
                .entities
                .get(&life.index)
                .filter(|enemy| enemy.type_id == life.definition)
            {
                seen.insert(life.definition);
                if let Some(combat) = &enemy.path.combat {
                    if combat.hits.iter().any(|row| row[HIT_DAMAGE] > ZERO) {
                        hit.insert(life.definition);
                    }
                    if combat.bars.iter().any(|bar| {
                        bar.updates
                            .iter()
                            .any(|update| update[HEALTH_BAR_END] == ZERO)
                    }) {
                        zero_bar.insert(life.definition);
                    }
                }
            } else if cycle >= life.death_cycle && seen.contains(&life.definition) {
                removed.insert(life.definition);
            }
        }
        for row in journal.iter().filter(|row| {
            matches!(
                row["kind"].as_str(),
                Some("altar_restored" | "arena_rejoined" | "altar_exit" | "complete")
            ) && row["state"]["cycle"].as_i64() == Some(i64::from(cycle))
        }) {
            let passive = &row["passive"];
            assert_eq!(
                i64::from(bit(&replay, faction.count_bit)?),
                count(passive, faction.name)?
            );
            assert_eq!(
                i64::from(bit(&replay, varbit::CURRENT_PRAYER_POINTS.id())?),
                integer(passive, "prayerFine")?
            );
            if row["kind"] == "complete" {
                for identity in [
                    varbit::GWD_BANDOS_KILLS.id(),
                    varbit::GWD_ARMADYL_KILLS.id(),
                    varbit::GWD_SARADOMIN_KILLS.id(),
                    varbit::GWD_ZAMORAK_KILLS.id(),
                ] {
                    assert_eq!(bit(&replay, identity)?, ZERO);
                }
                assert_eq!(
                    bit(&replay, varbit::LEGACY_COMBAT_ACTIVE.id())?,
                    i32::try_from(ONE)?
                );
            }
        }
    }
    assert_eq!(
        checked_retaliation_receipts,
        retaliation_receipts.len(),
        "every recorded native retaliation OFF receipt is consumed"
    );
    assert_eq!(done, frames.len(), "all recorded server frames applied");
    assert_eq!(player_updates, players.len());
    assert_eq!(npc_updates, npcs.len());
    assert!(player_updates > FIRST && npc_updates > FIRST && checked_snapshots > FIRST);
    assert_eq!(
        checked_snapshots, required_snapshots,
        "no skipped native snapshot outside recorded prefix"
    );
    let expected: BTreeSet<_> = faction.roles.iter().map(|role| role.definition).collect();
    assert_eq!(seen, expected);
    assert_eq!(hit, expected);
    assert_eq!(zero_bar, expected);
    assert_eq!(
        removed, expected,
        "native roster removes every recorded normal life"
    );
    let final_complete = journal
        .iter()
        .find(|row| row["kind"] == "complete")
        .context("complete")?;
    let saved_items = |field: &str| -> anyhow::Result<Vec<(i32, i32)>> {
        array(&saved, field)?
            .iter()
            .map(|slot| {
                Ok((
                    i32::try_from(slot[FIRST].as_i64().context("saved item")?)?,
                    i32::try_from(slot[STACK_QUANTITY].as_i64().context("saved quantity")?)?,
                ))
            })
            .collect()
    };
    assert_eq!(
        inventory(&mut replay, inv::BACKPACK.id()),
        saved_items("backpack")?
    );
    assert_eq!(
        inventory(&mut replay, inv::WORN_EQUIPMENT.id()),
        saved_items("worn")?
    );
    if faction.name == "armadyl" {
        let initial_arrows = item_count(
            &initial["account"]["backpack"],
            i64::from(obj::RUNE_ARROW_AMMUNITION.id()),
        )?;
        let observed_arrows = combat
            .iter()
            .filter(|row| row["kind"] == "state")
            .map(|row| {
                let actual = player(row)?;
                Ok(item_count(
                    &actual["backpack"],
                    i64::from(obj::RUNE_ARROW_AMMUNITION.id()),
                )? + item_count(&actual["worn"], i64::from(obj::RUNE_ARROW_AMMUNITION.id()))?)
            })
            .collect::<anyhow::Result<Vec<i64>>>()?;
        assert!(
            observed_arrows
                .iter()
                .any(|quantity| *quantity >= i64::from(ZERO) && *quantity < initial_arrows),
            "ordinary real bow/ammunition consumer, independent of later dropped-arrow loot"
        );
    }
    assert_eq!(
        final_complete["passive"]["generation"],
        combat
            .iter()
            .find(|row| row["kind"] == "state")
            .context("initial")?["players"][FIRST]["generation"]
    );
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_bandos_normal_roles_loot_altar_and_rejoin() -> anyhow::Result<()> {
    replay_faction(Faction {
        name: "bandos",
        recorded_input_sha256: BANDOS_RECORDED_INPUT_SHA256,
        count_bit: varbit::GWD_BANDOS_KILLS.id(),
        altar: loc::GWD_BANDOS_ALTAR.id(),
        roles: [
            Role {
                definition: npc::GWD_GENERAL_GRAARDOR.id(),
                name: "General Graardor",
                maximum_life: BANDOS_GENERAL_LIFE,
            },
            Role {
                definition: npc::GWD_SERGEANT_STRONGSTACK.id(),
                name: "Sergeant Strongstack",
                maximum_life: BANDOS_GUARD_LIFE,
            },
            Role {
                definition: npc::GWD_SERGEANT_STEELWILL.id(),
                name: "Sergeant Steelwill",
                maximum_life: BANDOS_GUARD_LIFE,
            },
            Role {
                definition: npc::GWD_SERGEANT_GRIMSPIKE.id(),
                name: "Sergeant Grimspike",
                maximum_life: BANDOS_GUARD_LIFE,
            },
        ],
    })
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_armadyl_normal_roles_loot_altar_and_rejoin() -> anyhow::Result<()> {
    replay_faction(Faction {
        name: "armadyl",
        recorded_input_sha256: GWD_NORMAL_INPUT_SHA256,
        count_bit: varbit::GWD_ARMADYL_KILLS.id(),
        altar: loc::GWD_ARMADYL_ALTAR.id(),
        roles: [
            Role {
                definition: npc::GWD_KREE_ARRA.id(),
                name: "Kree'arra",
                maximum_life: ARMADYL_GENERAL_LIFE,
            },
            Role {
                definition: npc::GWD_WINGMAN_SKREE.id(),
                name: "Wingman Skree",
                maximum_life: ARMADYL_GUARD_LIFE,
            },
            Role {
                definition: npc::GWD_FLOCKLEADER_GEERIN.id(),
                name: "Flockleader Geerin",
                maximum_life: ARMADYL_GUARD_LIFE,
            },
            Role {
                definition: npc::GWD_FLIGHT_KILISA.id(),
                name: "Flight Kilisa",
                maximum_life: ARMADYL_GUARD_LIFE,
            },
        ],
    })
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_saradomin_normal_roles_loot_altar_and_rejoin() -> anyhow::Result<()> {
    replay_faction(Faction {
        name: "saradomin",
        recorded_input_sha256: GWD_NORMAL_INPUT_SHA256,
        count_bit: varbit::GWD_SARADOMIN_KILLS.id(),
        altar: loc::GWD_SARADOMIN_ALTAR.id(),
        roles: [
            Role {
                definition: npc::GWD_COMMANDER_ZILYANA.id(),
                name: "Commander Zilyana",
                maximum_life: SARADOMIN_GENERAL_LIFE,
            },
            Role {
                definition: npc::GWD_STARLIGHT.id(),
                name: "Starlight",
                maximum_life: SARADOMIN_ZAMORAK_GUARD_LIFE,
            },
            Role {
                definition: npc::GWD_GROWLER.id(),
                name: "Growler",
                maximum_life: SARADOMIN_ZAMORAK_GUARD_LIFE,
            },
            Role {
                definition: npc::GWD_BREE.id(),
                name: "Bree",
                maximum_life: SARADOMIN_ZAMORAK_GUARD_LIFE,
            },
        ],
    })
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_zamorak_normal_roles_loot_altar_and_rejoin() -> anyhow::Result<()> {
    replay_faction(Faction {
        name: "zamorak",
        recorded_input_sha256: GWD_NORMAL_INPUT_SHA256,
        count_bit: varbit::GWD_ZAMORAK_KILLS.id(),
        altar: loc::GWD_ZAMORAK_ALTAR.id(),
        roles: [
            Role {
                definition: npc::GWD_K_RIL_TSUTSAROTH.id(),
                name: "K'ril Tsutsaroth",
                maximum_life: ZAMORAK_GENERAL_LIFE,
            },
            Role {
                definition: npc::GWD_BALFRUG_KREEYATH.id(),
                name: "Balfrug Kreeyath",
                maximum_life: SARADOMIN_ZAMORAK_GUARD_LIFE,
            },
            Role {
                definition: npc::GWD_TSTANON_KARLAK.id(),
                name: "Tstanon Karlak",
                maximum_life: SARADOMIN_ZAMORAK_GUARD_LIFE,
            },
            Role {
                definition: npc::GWD_ZAKL_N_GRITCH.id(),
                name: "Zakl'n Gritch",
                maximum_life: SARADOMIN_ZAMORAK_GUARD_LIFE,
            },
        ],
    })
}
