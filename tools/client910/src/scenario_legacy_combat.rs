//! Legacy ordinary attacks through native equipment, autocast and Settings input.
//! Target-selection console commands use the ordinary interaction owner; actual
//! OPNPC input and automatic retaliation acquisition are separately socket-tested.
use super::scenario_tests::ui;
use super::scenario_woodcutting::{cs2_ints, update_stat};
use super::session_replay::{arrivals, client_frames, mask_wall_clock, Replay, Trace};
use anyhow::Context;
use rs910_symbols::{component, inv, npc, obj, varbit, varp};
use std::collections::{BTreeMap, BTreeSet};

const FIXTURE: &str = "fixtures/session-replay/legacy-combat";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const ATTACK: i32 = 0;
const DEFENCE: i32 = 1;
const STRENGTH: i32 = 2;
const RANGED: i32 = 4;
const MAGIC: i32 = 6;
const FIRST_SLOT: usize = 0;
const COMBAT_SKILL_COUNT: usize = 7;
const HIT_DAMAGE_INDEX: usize = 1;
const HEALTH_BAR_END: usize = 2;
const COMPONENT_OFFSET: usize = 4;
const COMPONENT_END: usize = 8;
const REQUIRED_FIGHTS: usize = 3;
const REQUIRED_TRAINING_OPTIONS: usize = 4;
const MELEE_TRAINING: i32 = 7;
const SHARED_TRAINING: i32 = 3;
const INITIAL_XP: [i32; COMBAT_SKILL_COUNT] = [1154, 1154, 1154, 1154, 0, 0, 0];
const AFTER_MELEE_XP: [i32; COMBAT_SKILL_COUNT] = [1162, 1162, 1162, 1162, 0, 0, 0];
const AFTER_RANGED_XP: [i32; COMBAT_SKILL_COUNT] = [1162, 1174, 1162, 1170, 12, 0, 0];
const FINAL_XP: [i32; COMBAT_SKILL_COUNT] = [1162, 1186, 1162, 1178, 12, 0, 12];
const MELEE_DAMAGE: i32 = 111;
const RANGED_DAMAGE: i32 = 144;
const MAGIC_DAMAGE: i32 = 146;
const POLICY_READY: i32 = 1000;
const RANGED_START: i32 = 2000;
const MAGIC_START: i32 = 3000;
const RESOURCE_REMAINING: i32 = 3;
const NO_DAMAGE_COMMAND: &str = "hit ";
const KEYBOARD_EVENT_BYTES: usize = 4;
const KEYBOARD_TIME_OFFSET: usize = 1;

// Injector event time is sampled after the recorded frame clock. Compare the
// key bytes exactly; its three-byte elapsed-time telemetry is machine time.
pub(super) fn gameplay(
    bytes: &[u8],
    pings: &mut Vec<Vec<u8>>,
) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    Ok(mask_wall_clock(bytes)?
        .into_iter()
        .filter_map(|(opcode, mut payload)| {
            if opcode == crate::proto::client::PING_STATISTICS {
                pings.push(payload);
                return None;
            }
            if opcode == crate::proto::client::NO_TIMEOUT
                || opcode == crate::proto::client::EVENT_CAMERA_POSITION
            {
                return None;
            }
            if opcode == crate::proto::client::EVENT_KEYBOARD {
                assert_eq!(payload.len() % KEYBOARD_EVENT_BYTES, ZERO as usize);
                for event in payload.chunks_exact_mut(KEYBOARD_EVENT_BYTES) {
                    event[KEYBOARD_TIME_OFFSET..].fill(ZERO as u8);
                }
            }
            Some((opcode, payload))
        })
        .collect())
}

fn retaliation(replay: &mut Replay) -> anyhow::Result<i32> {
    let game = replay
        .core
        .session
        .as_mut()
        .context("session")?
        .game
        .as_mut()
        .context("game")?;
    match crate::client_game::with_game(game, |vars| {
        vars.get(
            native910::vars::VarScope::Player,
            varp::AUTO_RETALIATE_DISABLED.id() as u16,
            false,
        )
    })? {
        native910::vm::Value::Int(value) => Ok(value),
        value => anyhow::bail!("retaliation variable type {value:?}"),
    }
}

/// Passive server observations establish capped overkill and corpse-time rewards
/// independently of the client's packet parser and its skill/animation consumers.
fn verify_server_receipts(root: &std::path::Path) -> anyhow::Result<()> {
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(root.join("combat-receipts.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let launches: Vec<_> = rows
        .iter()
        .filter(|row| row["kind"] == "launch" && row["source"] == "player")
        .collect();
    assert_eq!(launches.len(), REQUIRED_FIGHTS);
    let damage = [MELEE_DAMAGE, RANGED_DAMAGE, MAGIC_DAMAGE];
    let training = [
        vec![ATTACK, STRENGTH, DEFENCE],
        vec![RANGED, DEFENCE],
        vec![MAGIC, DEFENCE],
    ];
    for (index, launch) in launches.iter().enumerate() {
        assert_eq!(launch["damage"], damage[index]);
        assert_eq!(
            launch["training"]["stats"],
            serde_json::json!(training[index])
        );
        assert_eq!(launch["training"]["timing"], "death-animation");
        assert_eq!(
            launch["delay"].as_i64().context("delay")? > i64::from(ZERO),
            index > FIRST_SLOT
        );
    }
    let deaths: Vec<_> = rows.iter().filter(|row| row["kind"] == "death").collect();
    assert_eq!(deaths.len(), REQUIRED_FIGHTS);
    let before = [INITIAL_XP, AFTER_MELEE_XP, AFTER_RANGED_XP];
    let after = [AFTER_MELEE_XP, AFTER_RANGED_XP, FINAL_XP];
    for (index, death) in deaths.iter().enumerate() {
        assert_eq!(death["targetDefinition"], npc::CHICKEN.id());
        let tick = death["tick"].as_i64().context("death tick")?;
        let hide = death["hideTick"].as_i64().context("hide tick")?;
        assert!(hide > tick);
        let states = rows
            .iter()
            .filter(|row| row["kind"] == "state" && row["phase"] == "npcs");
        let dying = states
            .clone()
            .rev()
            .find(|row| {
                row["tick"]
                    .as_i64()
                    .is_some_and(|at| at < hide && at >= tick)
            })
            .context("visible corpse observation")?;
        let vanished = states
            .clone()
            .find(|row| row["tick"] == hide)
            .context("body disappearance observation")?;
        assert_eq!(
            dying["players"][FIRST_SLOT]["xp"],
            serde_json::json!(before[index])
        );
        assert_eq!(
            vanished["players"][FIRST_SLOT]["xp"],
            serde_json::json!(after[index])
        );
        let target = |row: &serde_json::Value| -> anyhow::Result<bool> {
            row["enemies"]
                .as_array()
                .context("enemies")?
                .iter()
                .find(|enemy| enemy["id"] == death["targetId"])
                .context("dead target")?["visible"]
                .as_bool()
                .context("visible")
        };
        assert!(target(dying)?);
        assert!(!target(vanished)?);
    }
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_legacy_melee_ranged_magic_training_resources_and_corpse_rewards() -> anyhow::Result<()>
{
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    verify_server_receipts(&root)?;
    let trace = Trace::load(&root.join("session.rtr"))?;
    let packets = arrivals(&trace)?;
    let mut replay = Replay::start(&trace)?;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    let mut hits = BTreeSet::new();
    let mut dying = BTreeMap::new();
    let mut vanished = Vec::new();
    let mut death_starts = Vec::new();
    let mut training_operations = ZERO as usize;
    let mut equipment_operations = ZERO as usize;
    let mut attacks = ZERO as usize;
    let mut ranged_projectile = false;
    let mut magic_projectile = false;
    let mut disabled_retaliation = false;
    let mut enabled_retaliation = false;
    let mut previous_xp = INITIAL_XP;
    let mut rewards = Vec::new();
    let mut processed = FIRST_SLOT;
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            gameplay(&out.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: gameplay output"
        );
        for (opcode, payload) in client_frames(&out.written)? {
            if opcode == crate::proto::client::CLIENT_CHEAT {
                let command = String::from_utf8_lossy(&payload);
                assert!(
                    !command.contains(NO_DAMAGE_COMMAND)
                        && !command.contains("setxp ")
                        && !command.contains("invset "),
                    "recorded fight bypassed ordinary owners"
                );
                attacks += usize::from(command.contains("opnpc "));
            }
            if [
                crate::proto::client::IF_BUTTON1,
                crate::proto::client::IF_BUTTON2,
            ]
            .contains(&opcode)
            {
                let parent =
                    i32::from_be_bytes(payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?);
                training_operations +=
                    usize::from(parent == component::gameplay_settings::OPTIONS.packed());
                equipment_operations += usize::from(parent == component::backpack::SLOTS.packed());
            }
        }
        let game = replay.game();
        if cycle >= POLICY_READY {
            assert_eq!(
                game.varbit_value(varbit::LEGACY_COMBAT_ACTIVE.id() as u16)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?,
                FIRST_CYCLE
            );
        }
        let state = &game.runtime.feed.state;
        for (&id, enemy) in &state.npcs.entities {
            if enemy.type_id != npc::CHICKEN.id() {
                continue;
            }
            if let Some(combat) = &enemy.path.combat {
                for hit in &combat.hits {
                    hits.insert(hit[HIT_DAMAGE_INDEX]);
                }
            }
            if enemy.path.combat.as_ref().is_some_and(|combat| {
                combat.bars.iter().any(|bar| {
                    bar.updates
                        .iter()
                        .any(|update| update[HEALTH_BAR_END] == ZERO)
                })
            }) && !dying.contains_key(&id)
            {
                dying.insert(id, cycle);
                death_starts.push(cycle);
            }
        }
        let gone: Vec<_> = dying
            .keys()
            .copied()
            .filter(|id| !state.npcs.entities.contains_key(id))
            .collect();
        for id in gone {
            vanished.push(cycle);
            dying.remove(&id);
        }
        let projectiles = !state.zones.transients.projectiles.is_empty();
        ranged_projectile |= (RANGED_START..MAGIC_START).contains(&cycle) && projectiles;
        magic_projectile |= cycle >= MAGIC_START && projectiles;
        let done = replay.processed(&packets);
        let fresh = &packets[processed..done];
        processed = done;
        if fresh
            .iter()
            .any(|packet| packet.opcode == crate::proto::server::UPDATE_STAT)
        {
            let mut expected = previous_xp;
            for packet in fresh
                .iter()
                .filter(|packet| packet.opcode == crate::proto::server::UPDATE_STAT)
            {
                let [stat, xp, _] = update_stat(&packet.payload);
                if let Some(value) = expected.get_mut(stat as usize) {
                    *value = xp;
                }
            }
            if expected != previous_xp {
                assert!(
                    vanished.contains(&cycle),
                    "cycle {cycle}: XP arrived before NPC body removal"
                );
                assert_eq!(
                    expected,
                    [AFTER_MELEE_XP, AFTER_RANGED_XP, FINAL_XP][rewards.len()]
                );
                rewards.push(cycle);
            }
            let commands: Vec<_> = (ZERO..INITIAL_XP.len() as i32)
                .flat_map(|stat| [("push_constant_int", stat), ("stat_visible_xp", ZERO)])
                .collect();
            assert_eq!(
                cs2_ints(&mut replay, &commands)?,
                expected,
                "cycle {cycle}: native skill XP consumer"
            );
            previous_xp = expected;
        }
        let preference = retaliation(&mut replay)?;
        disabled_retaliation |= preference == FIRST_CYCLE;
        enabled_retaliation |= disabled_retaliation && preference == ZERO;
    }
    assert_eq!(replay_pings, recorded_pings, "ICMP bytes in stream order");
    assert_eq!(attacks, REQUIRED_FIGHTS);
    assert_eq!(death_starts.len(), REQUIRED_FIGHTS);
    assert_eq!(vanished.len(), REQUIRED_FIGHTS);
    assert_eq!(rewards.len(), REQUIRED_FIGHTS);
    assert!(
        training_operations >= REQUIRED_TRAINING_OPTIONS
            && equipment_operations >= REQUIRED_TRAINING_OPTIONS
    );
    assert!(ranged_projectile && magic_projectile && disabled_retaliation && enabled_retaliation);
    for damage in [MELEE_DAMAGE, RANGED_DAMAGE, MAGIC_DAMAGE] {
        assert!(hits.contains(&damage), "hit {damage} absent: {hits:?}");
    }
    assert_eq!(previous_xp, FINAL_XP);
    for (bit, expected) in [
        (varbit::MELEE_TRAINING_MASK, MELEE_TRAINING),
        (varbit::RANGED_TRAINING_MASK, SHARED_TRAINING),
        (varbit::MAGIC_TRAINING_MASK, SHARED_TRAINING),
    ] {
        assert_eq!(
            replay
                .game()
                .varbit_value(bit.id() as u16)
                .map_err(|error| anyhow::anyhow!("{error:?}"))?,
            expected
        );
    }
    let inventory = |replay: &mut Replay, id: i32| {
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
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    assert!(inventory(&mut replay, inv::BACKPACK.id())
        .contains(&(obj::AIR_RUNE.id(), RESOURCE_REMAINING)));
    assert!(inventory(&mut replay, inv::WORN_EQUIPMENT.id())
        .contains(&(obj::BRONZE_ARROW.id(), RESOURCE_REMAINING)));
    Ok(())
}
