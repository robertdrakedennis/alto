//! Native Legacy special pane pointer input, ordinary combat and periodic energy.
//! The recorded console target command uses the same normal interaction owner;
//! actual OPNPC and weapon-family mechanics are independently socket-tested.
use super::scenario_tests::ui;
use super::session_replay::{client_frames, mask_wall_clock, Replay, Trace};
use anyhow::Context;
use rs910_symbols::{component, interface, inv, npc, obj, varp};
use serde_json::Value;

const FIXTURE: &str = "fixtures/session-replay/legacy-specials";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const NO_CHILD: i32 = -1;
const FIRST_SLOT: usize = 0;
const SINGLE_LAUNCH: usize = 1;
const NATIVE_POINTER_OPERATIONS: usize = 3;
const COMPONENT_OFFSET: usize = 4;
const COMPONENT_END: usize = 8;
const HIT_DAMAGE_INDEX: usize = 1;
const HEALTH_BAR_END: usize = 2;
const MAIN_HAND_SLOT: usize = 3;
const UI_READY: i32 = 1000;
const PACKED_GROUP_BITS: u32 = 16;

fn gameplay(bytes: &[u8], pings: &mut Vec<Vec<u8>>) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
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
}

fn native_energy(replay: &mut Replay, variable: i32) -> anyhow::Result<i32> {
    let game = replay
        .core
        .session
        .as_mut()
        .context("session")?
        .game
        .as_mut()
        .context("game")?;
    match crate::client_game::with_game(game, |vars| {
        vars.get(native910::vars::VarScope::Player, variable as u16, false)
    })? {
        native910::vm::Value::Int(value) => Ok(value),
        value => anyhow::bail!("energy variable has wrong native type: {value:?}"),
    }
}

fn verify_receipts(root: &std::path::Path) -> anyhow::Result<(i32, i32, i32)> {
    let plan: Value = serde_json::from_str(&std::fs::read_to_string(root.join("plan.json"))?)?;
    let rows: Vec<Value> = std::fs::read_to_string(root.join("special-receipts.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let launches: Vec<_> = rows
        .iter()
        .filter(|r| r["kind"] == "launch" && r["source"] == "player" && !r["special"].is_null())
        .collect();
    assert_eq!(launches.len(), SINGLE_LAUNCH);
    let launch = launches[FIRST_SLOT];
    let integer = |v: &Value| v.as_i64().context("recorded integer");
    assert!(integer(&launch["damage"])? > i64::from(ZERO));
    let maximum = integer(&plan["energy"]["maximumEnergyFine"])?;
    let spent = maximum - integer(&plan["special"]["costFine"])?;
    let regeneration = integer(&plan["energy"]["regenerationFine"])?;
    let interval = integer(&plan["energy"]["regenerationTicks"])?;
    assert_eq!(
        integer(&plan["special"]["weapon"])?,
        i64::from(obj::GRANITE_MAUL.id())
    );
    assert_eq!(integer(&launch["sourceEnergy"])?, spent);
    assert!(rows.iter().any(|r| r["kind"] == "death"
        && r["targetId"] == launch["targetId"]
        && r["sourceId"] == launch["sourceId"]
        && r["tick"] == launch["tick"]));
    let states: Vec<_> = rows
        .iter()
        .filter(|r| r["kind"] == "state" && r["phase"] == "players")
        .filter_map(|r| {
            r["players"]
                .as_array()
                .and_then(|p| p.first())
                .map(|p| (r, p))
        })
        .collect();
    let first = states.first().context("first connected player state")?;
    let spent_state = states
        .iter()
        .find(|(r, p)| r["tick"] == launch["tick"] && p["energy"].as_i64() == Some(spent))
        .context("spent state")?;
    let spent_tick = integer(&spent_state.0["tick"])?;
    let refill = states
        .iter()
        .find(|(r, p)| {
            r["tick"].as_i64().is_some_and(|tick| tick > spent_tick)
                && p["energy"].as_i64().is_some_and(|energy| energy > spent)
        })
        .context("first refill")?;
    assert_eq!(integer(&refill.1["energy"])? - spent, regeneration);
    assert_eq!(
        (integer(&refill.0["tick"])? - integer(&first.0["tick"])?) % interval,
        i64::from(ZERO)
    );
    for (_, p) in &states {
        assert_eq!(p["energyFeed"], p["energy"]);
        assert_eq!(
            integer(&p["armedFeed"])? != i64::from(ZERO),
            p["armed"].as_bool().context("armed")?
        );
    }
    Ok((maximum as i32, spent as i32, (spent + regeneration) as i32))
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_native_legacy_special_bar_toggles_spends_hits_and_refills() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let (maximum, spent, refill) = verify_receipts(&root)?;
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    let mut energy_values = Vec::new();
    let mut armed_values = Vec::new();
    let mut special_operations = ZERO as usize;
    let mut equipped = false;
    let mut target_interacted = false;
    let mut real_hit = false;
    let mut visible_corpse = false;
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        assert_eq!(
            gameplay(&output.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: ordinary gameplay bytes"
        );
        for (opcode, payload) in client_frames(&output.written)? {
            if opcode == crate::proto::client::IF_BUTTON1 {
                let packed =
                    i32::from_be_bytes(payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?);
                special_operations +=
                    usize::from(packed == component::legacy_combat::SPECIAL_ATTACK.packed());
            }
            if opcode == crate::proto::client::IF_BUTTON2 {
                let packed =
                    i32::from_be_bytes(payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?);
                equipped |= packed == component::backpack::SLOTS.packed();
            }
            if opcode == crate::proto::client::CLIENT_CHEAT {
                let command = String::from_utf8_lossy(&payload);
                assert!(
                    !command.contains("hit ")
                        && !command.contains("setxp ")
                        && !command.contains("invset ")
                        && !command.contains("energy "),
                    "combat/energy bypass in recorded input"
                );
                target_interacted |= command.contains("opnpc ");
            }
        }
        if cycle >= UI_READY {
            let energy = native_energy(&mut replay, varp::ADRENALINE_FINE.id())?;
            let armed = native_energy(&mut replay, varp::SPECIAL_ATTACK_ARMED.id())? != ZERO;
            if energy_values.last().copied() != Some(energy) {
                energy_values.push(energy);
            }
            if armed_values.last().copied() != Some(armed) {
                armed_values.push(armed);
            }
        }
        if cycle == UI_READY {
            assert!(replay
                .ui()
                .state
                .layout
                .subs
                .iter()
                .any(
                    |&(parent, group)| parent == component::game_window::MELEE_SLOT.packed()
                        && group == interface::LEGACY_COMBAT.id()
                ));
            let bar = ui(&mut replay)
                .store
                .get(component::legacy_combat::SPECIAL_ATTACK.packed(), NO_CHILD)?
                .context("native special bar")?;
            let bar = bar.borrow();
            assert!(!bar.runtime_entry_hidden().unwrap_or(bar.f.hide));
            assert!(bar.f.width > ZERO && bar.f.height > ZERO);
        }
        for enemy in replay.game().runtime.feed.state.npcs.entities.values() {
            if enemy.type_id != npc::CHICKEN.id() {
                continue;
            }
            if let Some(combat) = &enemy.path.combat {
                real_hit |= combat.hits.iter().any(|hit| hit[HIT_DAMAGE_INDEX] > ZERO);
                visible_corpse |= combat.bars.iter().any(|bar| {
                    bar.updates
                        .iter()
                        .any(|update| update[HEALTH_BAR_END] == ZERO)
                });
            }
        }
    }
    assert_eq!(recorded_pings, replay_pings, "ICMP bytes in stream order");
    assert_eq!(special_operations, NATIVE_POINTER_OPERATIONS);
    assert_eq!(energy_values, [maximum, spent, refill]);
    assert_eq!(armed_values, [false, true, false, true, false]);
    assert!(equipped && target_interacted && real_hit && visible_corpse);
    let inventory = ui(&mut replay)
        .engine
        .inv_cache
        .inventory(inv::WORN_EQUIPMENT.id(), false)
        .context("worn inventory")?;
    assert_eq!(inventory.obj_ids[MAIN_HAND_SLOT], obj::GRANITE_MAUL.id());
    assert_eq!(
        component::legacy_combat::SPECIAL_ATTACK.packed() as u32 >> PACKED_GROUP_BITS,
        interface::LEGACY_COMBAT.id() as u32
    );
    Ok(())
}

const LOCATE_FIXTURE: &str = "fixtures/session-replay/legacy-locate";
const LOCATE_RECIPIENTS: usize = 2;
const LOCATE_POINTER_OPERATIONS: usize = 1;
const LOCATE_EQUIPMENT_OPERATIONS: usize = 2;
const AMMUNITION_SLOT: usize = 13;
const RECT_X: usize = 0;
const RECT_Y: usize = 1;
const RECT_WIDTH: usize = 2;
const RECT_HEIGHT: usize = 3;
const DEBIT_PAIR_LENGTH: usize = 2;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocatePlan {
    energy: LocateEnergy,
    special: LocateSpecial,
    physical: LocatePhysical,
    ammunition: LocateAmmunition,
    recipients: Vec<LocateRecipient>,
    cycle: LocateCycles,
    last_cycle: i32,
    pointer: LocatePointer,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocateEnergy {
    maximum_energy_fine: i32,
    regeneration_fine: i32,
    regeneration_ticks: i32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocateSpecial {
    weapon: i32,
    cost_fine: i32,
    effects: Vec<LocateEffect>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocateEffect {
    kind: String,
    duration_ticks: i32,
    radius_tiles: i32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocatePhysical {
    fresh_charges: i32,
    wear_per_tick: i32,
    used_item: i32,
}

#[derive(serde::Deserialize)]
struct LocateAmmunition {
    item: i32,
    count: i32,
    consumed: bool,
}

#[derive(serde::Deserialize)]
struct LocateRecipient {
    role: String,
    definition: i32,
    x: i32,
    z: i32,
}

#[derive(serde::Deserialize)]
struct LocateCycles {
    #[serde(rename = "READY")]
    ready: i32,
}

#[derive(serde::Deserialize)]
struct LocatePointer {
    x: i32,
    y: i32,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum LocateReceipt {
    Launch(LocateLaunch),
    Landed(LocateLanded),
    Death {
        tick: i32,
        #[serde(rename = "targetId")]
        target_id: i32,
        #[serde(rename = "sourceId")]
        source_id: i32,
    },
    State(LocateState),
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocateLaunch {
    hit_id: i32,
    tick: i32,
    source: String,
    source_id: i32,
    target_id: i32,
    target_life: i32,
    target_x: i32,
    target_z: i32,
    style: String,
    delay: i32,
    damage: i32,
    accurate: bool,
    special: Option<Value>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocateLanded {
    hit_id: i32,
    target_id: i32,
    target_life: i32,
    source_id: i32,
    actual_damage: i32,
    killed: bool,
    tick: i32,
}

#[derive(serde::Deserialize)]
struct LocateState {
    phase: String,
    tick: i32,
    players: Vec<LocatePlayer>,
    enemies: Vec<LocateEnemy>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocatePlayer {
    energy: i32,
    energy_feed: i32,
    armed: bool,
    armed_feed: i32,
    worn: Vec<LocateItem>,
    conditions: Vec<LocateCondition>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum LocateItem {
    Plain(i32, i32),
    Physical(i32, i32, LocateInstance),
}

impl LocateItem {
    fn item(&self) -> i32 {
        match self {
            Self::Plain(item, _) | Self::Physical(item, _, _) => *item,
        }
    }

    fn count(&self) -> i32 {
        match self {
            Self::Plain(_, count) | Self::Physical(_, count, _) => *count,
        }
    }

    fn instance(&self) -> Option<&LocateInstance> {
        match self {
            Self::Physical(_, _, instance) => Some(instance),
            Self::Plain(_, _) => None,
        }
    }
}

#[derive(serde::Deserialize)]
struct LocateInstance {
    key: String,
    charges: i32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocateCondition {
    applied_tick: i32,
    until_tick: i32,
    area: Option<LocateArea>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocateArea {
    radius_tiles: i32,
}

#[derive(serde::Deserialize)]
struct LocateEnemy {
    id: i32,
    x: i32,
    z: i32,
    hitpoints: i32,
    alive: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocateEnergyInputs {
    variable: i32,
    expected_native_values_from_ready: Vec<i32>,
}

struct LocateObserved {
    maximum: i32,
    spent: i32,
    refill: i32,
    targets: Vec<usize>,
}

fn verify_locate_receipts(
    root: &std::path::Path,
    plan: &LocatePlan,
) -> anyhow::Result<LocateObserved> {
    let rows: Vec<LocateReceipt> = std::fs::read_to_string(root.join("locate-receipts.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let launches: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            LocateReceipt::Launch(hit) if hit.source == "player" && hit.special.is_none() => {
                Some(hit)
            }
            _ => None,
        })
        .collect();
    assert_eq!(launches.len(), LOCATE_RECIPIENTS);
    let first_launch = launches[FIRST_SLOT];
    let states: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            LocateReceipt::State(state) if state.phase == "players" => {
                state.players.first().map(|player| (state, player))
            }
            _ => None,
        })
        .collect();
    let first_state = states.first().context("first connected state")?;
    let spent = plan.energy.maximum_energy_fine - plan.special.cost_fine;
    let spent_state = states
        .iter()
        .find(|(_, player)| player.energy == spent)
        .context("self activation spent state")?;
    let refill_state = states
        .iter()
        .find(|(state, player)| state.tick > spent_state.0.tick && player.energy > spent)
        .context("first generated refill")?;
    assert_eq!(refill_state.1.energy - spent, plan.energy.regeneration_fine);
    assert_eq!(
        (refill_state.0.tick - first_state.0.tick) % plan.energy.regeneration_ticks,
        ZERO
    );
    assert!(refill_state.0.tick - spent_state.0.tick <= plan.energy.regeneration_ticks);
    let mut energies = Vec::new();
    for (_, player) in &states {
        assert_eq!(player.energy_feed, player.energy);
        assert_eq!(player.armed_feed != ZERO, player.armed);
        if energies.last().copied() != Some(player.energy) {
            energies.push(player.energy);
        }
    }
    let debits: Vec<_> = energies
        .windows(DEBIT_PAIR_LENGTH)
        .filter(|pair| pair[FIRST_SLOT] > pair[SINGLE_LAUNCH])
        .collect();
    assert_eq!(debits.len(), SINGLE_LAUNCH);
    assert_eq!(
        debits[FIRST_SLOT][FIRST_SLOT] - debits[FIRST_SLOT][SINGLE_LAUNCH],
        plan.special.cost_fine
    );
    let expected: std::collections::BTreeSet<_> = plan
        .recipients
        .iter()
        .filter(|recipient| recipient.role != "outside")
        .map(|recipient| (recipient.x, recipient.z))
        .collect();
    assert_eq!(
        launches
            .iter()
            .map(|hit| (hit.target_x, hit.target_z))
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    for hit in &launches {
        assert_eq!(hit.tick, first_launch.tick);
        assert_eq!(hit.style, "ranged");
        assert!(hit.delay > ZERO && hit.damage > ZERO && hit.accurate);
        let landed: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                LocateReceipt::Landed(arrival) if arrival.hit_id == hit.hit_id => Some(arrival),
                _ => None,
            })
            .collect();
        assert_eq!(landed.len(), SINGLE_LAUNCH);
        let arrival = landed[FIRST_SLOT];
        assert_eq!(
            (arrival.target_id, arrival.target_life, arrival.source_id),
            (hit.target_id, hit.target_life, hit.source_id)
        );
        let original_life = rows
            .iter()
            .filter_map(|row| match row {
                LocateReceipt::State(state) if state.tick < hit.tick => {
                    state.enemies.iter().find(|enemy| enemy.id == hit.target_id)
                }
                _ => None,
            })
            .next_back()
            .context("pre-launch target life")?
            .hitpoints;
        assert_eq!(arrival.actual_damage, hit.damage.min(original_life));
        assert!(arrival.actual_damage > ZERO && arrival.killed);
        assert_eq!(arrival.tick, hit.tick + hit.delay);
        assert!(rows.iter().any(|row| matches!(row, LocateReceipt::Death { tick, target_id, source_id } if *tick == arrival.tick && *target_id == hit.target_id && *source_id == hit.source_id)));
    }
    let outside = plan
        .recipients
        .iter()
        .find(|recipient| recipient.role == "outside")
        .context("outside recipient")?;
    let outside_states: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            LocateReceipt::State(state) => state
                .enemies
                .iter()
                .find(|enemy| (enemy.x, enemy.z) == (outside.x, outside.z)),
            _ => None,
        })
        .collect();
    let initial_outside = outside_states.first().context("outside spawned")?;
    assert!(outside_states
        .iter()
        .all(|enemy| enemy.alive && enemy.hitpoints == initial_outside.hitpoints));
    assert_eq!(plan.ammunition.item, obj::BRONZE_ARROW.id());
    assert!(!plan.ammunition.consumed);
    let ammunition: Vec<_> = states
        .iter()
        .filter_map(|(_, player)| player.worn.get(AMMUNITION_SLOT))
        .filter(|item| item.item() == plan.ammunition.item)
        .collect();
    assert!(!ammunition.is_empty());
    assert!(ammunition
        .iter()
        .all(|item| item.count() == plan.ammunition.count));
    let balances: Vec<_> = states
        .iter()
        .filter_map(|(state, player)| {
            player
                .worn
                .get(MAIN_HAND_SLOT)?
                .instance()
                .map(|instance| (state.tick, instance))
        })
        .collect();
    let before = balances
        .iter()
        .find(|(tick, _)| *tick == spent_state.0.tick)
        .context("activation wear state")?
        .1;
    let after = balances
        .iter()
        .find(|(tick, _)| *tick == first_launch.tick)
        .context("area swing wear state")?
        .1;
    assert_eq!(
        plan.physical.fresh_charges - before.charges,
        plan.physical.wear_per_tick
    );
    assert_eq!(before.charges - after.charges, plan.physical.wear_per_tick);
    assert!(balances
        .iter()
        .all(|(_, instance)| instance.key == before.key));
    let effect = plan
        .special
        .effects
        .iter()
        .find(|effect| effect.kind == "ordinary-target-area")
        .context("generated Locate effect")?;
    let conditions: Vec<_> = states
        .iter()
        .flat_map(|(state, player)| {
            player
                .conditions
                .iter()
                .filter(|condition| condition.area.is_some())
                .map(move |condition| (state.tick, condition))
        })
        .collect();
    assert!(!conditions.is_empty());
    assert!(conditions.iter().all(
        |(_, condition)| condition.until_tick - condition.applied_tick == effect.duration_ticks
            && condition
                .area
                .as_ref()
                .is_some_and(|area| area.radius_tiles == effect.radius_tiles)
    ));
    assert!(conditions
        .iter()
        .any(|(tick, condition)| *tick == first_launch.tick
            && condition.applied_tick <= *tick
            && *tick < condition.until_tick));
    assert!(states.iter().any(|(state, player)| state.tick
        >= spent_state.0.tick + effect.duration_ticks
        && player
            .conditions
            .iter()
            .all(|condition| condition.area.is_none())));
    Ok(LocateObserved {
        maximum: plan.energy.maximum_energy_fine,
        spent,
        refill: spent + plan.energy.regeneration_fine,
        targets: launches
            .iter()
            .map(|hit| {
                usize::try_from(hit.target_id).context("nonnegative recorded native NPC slot")
            })
            .collect::<Result<_, _>>()?,
    })
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_legacy_locate_pointer_charge_bow_hits_and_refill_reach_native_consumers(
) -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(LOCATE_FIXTURE);
    let plan: LocatePlan = serde_json::from_slice(&std::fs::read(root.join("plan.json"))?)?;
    assert_eq!(plan.special.weapon, obj::DECIMATION.id());
    assert!(plan
        .recipients
        .iter()
        .all(|recipient| recipient.definition == npc::CHICKEN.id()));
    let observed = verify_locate_receipts(&root, &plan)?;
    let trace = Trace::load(&root.join("session.rtr"))?;
    // The actual recording lost focus. This is a native gameplay replay, not a
    // certificate of continuous visible rendering or of the absent terminal images.
    assert!(trace
        .records
        .iter()
        .any(|record| record.cycle >= plan.cycle.ready
            && &record.tag == b"FOCS"
            && record.bytes == [ZERO as u8]));
    assert_eq!(trace.last_cycle(), plan.last_cycle);
    let mut replay = Replay::start(&trace)?;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    let mut energy_values = Vec::new();
    let mut special_operations = ZERO as usize;
    let mut equipment_operations = ZERO as usize;
    let mut ordinary_interaction = false;
    let mut hit_slots = std::collections::BTreeSet::new();
    let mut corpse_slots = std::collections::BTreeSet::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        assert_eq!(
            gameplay(&output.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: ordinary Locate gameplay bytes"
        );
        for (opcode, payload) in client_frames(&output.written)? {
            if opcode == crate::proto::client::IF_BUTTON1 {
                let packed =
                    i32::from_be_bytes(payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?);
                special_operations +=
                    usize::from(packed == component::legacy_combat::SPECIAL_ATTACK.packed());
            } else if opcode == crate::proto::client::IF_BUTTON2 {
                let packed =
                    i32::from_be_bytes(payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?);
                equipment_operations += usize::from(packed == component::backpack::SLOTS.packed());
            } else if opcode == crate::proto::client::CLIENT_CHEAT {
                let command = String::from_utf8_lossy(&payload);
                assert!(
                    !command.contains("hit ")
                        && !command.contains("setxp ")
                        && !command.contains("invset ")
                        && !command.contains("energy ")
                );
                ordinary_interaction |= command.contains("opnpc ");
            }
        }
        if cycle >= plan.cycle.ready {
            let energy = native_energy(&mut replay, varp::ADRENALINE_FINE.id())?;
            if energy_values.last().copied() != Some(energy) {
                energy_values.push(energy);
            }
            assert_eq!(
                native_energy(&mut replay, varp::SPECIAL_ATTACK_ARMED.id())?,
                ZERO
            );
        }
        if cycle == plan.cycle.ready {
            let rect = super::scenario_tests::component_rect(ui(&mut replay), &|control| {
                control.f.parentlayer == component::legacy_combat::SPECIAL_ATTACK.packed()
            })
            .context("actual current bow native bar")?;
            assert!(rect[RECT_WIDTH] > ZERO && rect[RECT_HEIGHT] > ZERO);
            assert!(
                plan.pointer.x >= rect[RECT_X]
                    && plan.pointer.x < rect[RECT_X] + rect[RECT_WIDTH]
                    && plan.pointer.y >= rect[RECT_Y]
                    && plan.pointer.y < rect[RECT_Y] + rect[RECT_HEIGHT]
            );
            let inventory = ui(&mut replay)
                .engine
                .inv_cache
                .inventory(inv::WORN_EQUIPMENT.id(), false)
                .context("actual native ready inventory")?;
            assert_eq!(inventory.obj_ids[MAIN_HAND_SLOT], obj::DECIMATION.id());
            assert_eq!(inventory.obj_ids[AMMUNITION_SLOT], obj::BRONZE_ARROW.id());
        }
        for (slot, enemy) in &replay.game().runtime.feed.state.npcs.entities {
            if enemy.type_id != npc::CHICKEN.id() {
                continue;
            }
            if let Some(combat) = &enemy.path.combat {
                if combat.hits.iter().any(|hit| hit[HIT_DAMAGE_INDEX] > ZERO) {
                    hit_slots.insert(*slot);
                }
                if combat.bars.iter().any(|bar| {
                    bar.updates
                        .iter()
                        .any(|update| update[HEALTH_BAR_END] == ZERO)
                }) {
                    corpse_slots.insert(*slot);
                }
            }
        }
    }
    assert_eq!(recorded_pings, replay_pings, "ICMP bytes in stream order");
    assert_eq!(special_operations, LOCATE_POINTER_OPERATIONS);
    assert_eq!(equipment_operations, LOCATE_EQUIPMENT_OPERATIONS);
    assert!(ordinary_interaction);
    let energy_inputs: LocateEnergyInputs =
        serde_json::from_slice(&std::fs::read(root.join("native-energy-inputs.json"))?)?;
    assert_eq!(energy_inputs.variable, varp::ADRENALINE_FINE.id());
    assert!(energy_inputs
        .expected_native_values_from_ready
        .starts_with(&[observed.maximum, observed.spent, observed.refill]));
    assert_eq!(
        energy_values,
        energy_inputs.expected_native_values_from_ready
    );
    assert!(observed
        .targets
        .iter()
        .all(|slot| hit_slots.contains(slot) && corpse_slots.contains(slot)));
    let inventory = ui(&mut replay)
        .engine
        .inv_cache
        .inventory(inv::WORN_EQUIPMENT.id(), false)
        .context("native worn inventory")?;
    assert_eq!(inventory.obj_ids[MAIN_HAND_SLOT], plan.physical.used_item);
    assert_eq!(inventory.obj_ids[AMMUNITION_SLOT], plan.ammunition.item);
    assert_eq!(inventory.counts[AMMUNITION_SLOT], plan.ammunition.count);
    Ok(())
}
