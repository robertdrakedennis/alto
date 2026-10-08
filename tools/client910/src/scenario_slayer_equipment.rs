//! Native equipment input and durable Slayer upgrades, rewards and ring travel.
use super::scenario_tests::ui;
use super::session_replay::{client_frames, mask_wall_clock, Replay, Trace};
use anyhow::Context;
use rs910_symbols::{component, inv, varbit};
use serde_json::Value;
use std::collections::BTreeSet;

const FIXTURE: &str = "fixtures/session-replay/slayer-equipment";
const FIRST_CYCLE: i32 = 1;
const FIRST_ANCESTOR: usize = 0;
const ZERO: i32 = 0;
const FIRST_FIELD: usize = 0;
const X: usize = 0;
const Y: usize = 1;
const INSTANCE_FIELD: usize = 2;
const HEAD_SLOT: usize = 0;
const COMPONENT_OFFSET: usize = 4;
const COMPONENT_END: usize = 8;
const ITEM_LOW: usize = 0;
const ITEM_HIGH: usize = 1;
const SLOT_HIGH: usize = 2;
const SLOT_LOW: usize = 3;
const SHORT_HIGH_SHIFT: u32 = 8;
const ALTERNATE_LOW_BIAS: u8 = 128;
const PAUSE_COMPONENT_BYTES: [usize; 4] = [1, 0, 3, 2];
const NATIVE_WEAR_OPERATIONS: usize = 3;
const ONE_NATIVE_WEAR: usize = 1;
const LAST_ITEM_OFFSET: usize = 1;
const COMPLETE_TIER_COUNT: usize = 4;
const SLAYER_SKILL: i32 = 18;

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

fn integer(value: &Value) -> anyhow::Result<i32> {
    i32::try_from(value.as_i64().context("recorded integer")?).context("recorded integer width")
}

fn native_component_reachable(
    replay: &mut Replay,
    parent: i32,
    child: i32,
) -> anyhow::Result<bool> {
    const NO_CHILD: i32 = -1;
    const PACKED_GROUP_BITS: u32 = 16;
    const MAX_VISIBLE_ANCESTORS: usize = 64;
    const CENTRE_DIVISOR: i32 = 2;
    let state = ui(replay);
    let Some(mut current) = state.store.get(parent, child)? else {
        return Ok(false);
    };
    let mut point = {
        let component = current.borrow();
        [
            component.f.width / CENTRE_DIVISOR,
            component.f.height / CENTRE_DIVISOR,
        ]
    };
    let mut visited = BTreeSet::new();
    for _ in FIRST_ANCESTOR..MAX_VISIBLE_ANCESTORS {
        anyhow::ensure!(
            visited.insert(std::rc::Rc::as_ptr(&current) as usize),
            "native control ancestry cycle"
        );
        if !rs910_ui::ui_hooks::attached(&mut state.store, &current)? {
            return Ok(false);
        }
        let component = current.borrow();
        if component.runtime_entry_hidden().unwrap_or(component.f.hide)
            || component.f.width <= ZERO
            || component.f.height <= ZERO
            || point[X] < ZERO
            || point[Y] < ZERO
            || point[X] >= component.f.width
            || point[Y] >= component.f.height
        {
            return Ok(false);
        }
        let position = [component.f.x, component.f.y];
        let runtime_parent = component.runtime_parent();
        let layer = component.f.layer;
        let group = (component.f.parentlayer as u32 >> PACKED_GROUP_BITS) as i32;
        drop(component);
        let next = if let Some(parent) = runtime_parent {
            Some(parent)
        } else if layer != NO_CHILD {
            state.store.get(layer, NO_CHILD)?
        } else if group == state.state.life.top {
            return Ok(true);
        } else if let Some(&(parent, _)) = state
            .state
            .layout
            .subs
            .iter()
            .find(|&&(_, child)| child == group)
        {
            state.store.get(parent, NO_CHILD)?
        } else {
            None
        };
        let Some(next) = next else { return Ok(false) };
        {
            let parent = next.borrow();
            point = [
                position[X] + point[X] - parent.f.scrollx,
                position[Y] + point[Y] - parent.f.scrolly,
            ];
        }
        current = next;
    }
    anyhow::bail!("native control ancestry exceeds its recorded bound")
}

fn verify_receipts(root: &std::path::Path, plan: &Value) -> anyhow::Result<()> {
    let rows: Vec<Value> = std::fs::read_to_string(root.join("equipment-receipts.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let players: Vec<_> = rows
        .iter()
        .filter_map(|row| row["players"].as_array()?.first())
        .collect();
    let original_key = plan["initialInstance"]["key"]
        .as_str()
        .context("initial physical key")?;
    let physical = |player: &&Value, item: &Value, slaying: i32| {
        let slot = &player["worn"][HEAD_SLOT];
        slot[FIRST_FIELD] == *item
            && slot[INSTANCE_FIELD]["key"] == original_key
            && slot[INSTANCE_FIELD]["resources"]["slayingTeleports"] == slaying
            && slot[INSTANCE_FIELD]["resources"]["ferociousTeleports"] == FIRST_CYCLE
    };
    assert!(players
        .iter()
        .any(|player| physical(player, &plan["initialHelmet"], FIRST_CYCLE)));
    let upgrades = plan["upgrades"].as_array().context("upgrades")?;
    assert_eq!(upgrades.len(), COMPLETE_TIER_COUNT);
    for upgrade in upgrades {
        assert!(players.iter().any(|player| physical(
            player,
            &upgrade["fusedOutput"],
            FIRST_CYCLE
        )));
    }
    assert!(
        players.iter().any(|player| {
            let head = &player["worn"][HEAD_SLOT];
            head[FIRST_FIELD] == plan["initialHelmet"]
                && head[INSTANCE_FIELD]["key"]
                    .as_str()
                    .is_some_and(|key| key != original_key)
                && head[INSTANCE_FIELD]["resources"] == serde_json::json!({})
                && player["slayer"]["points"] == plan["expectedPoints"]
        }),
        "fusion must create a different identity with empty balances"
    );
    let final_item = &upgrades.last().context("last upgrade")?["fusedOutput"];
    let final_state = players
        .iter()
        .find(|player| {
            physical(player, final_item, ZERO)
                && ["x", "z", "level"]
                    .iter()
                    .all(|field| player[field] == plan["destination"]["to"][field])
        })
        .context("ordinary ring arrival with independent remaining balance")?;
    assert_eq!(final_state["saved"]["worn"], final_state["worn"]);
    assert_eq!(final_state["saved"]["slayer"], final_state["slayer"]);
    assert_eq!(final_state["slayerXp"], plan["expectedXp"]);
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_native_slayer_equipment_upgrades_rewards_fusion_and_travel() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let plan: Value = serde_json::from_str(&std::fs::read_to_string(root.join("plan.json"))?)?;
    verify_receipts(&root, &plan)?;
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    let expected_upgrades: BTreeSet<i32> = plan["upgrades"]
        .as_array()
        .context("upgrades")?
        .iter()
        .map(|row| integer(&row["button"]))
        .collect::<Result<_, _>>()?;
    let expected_items: BTreeSet<i32> = plan["upgrades"]
        .as_array()
        .context("upgrades")?
        .iter()
        .map(|row| integer(&row["fusedOutput"]))
        .collect::<Result<_, _>>()?;
    let mut observed_upgrades = BTreeSet::new();
    let mut observed_items = BTreeSet::new();
    let mut native_wears = ZERO as usize;
    let mut native_teleport = false;
    let mut native_destination_choice = false;
    let mut native_rewards = BTreeSet::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        if cycle >= integer(&plan["controlOperations"][FIRST_FIELD]["cycle"])? {
            assert_eq!(
                replay
                    .game()
                    .varbit_value(u16::try_from(varbit::LEGACY_COMBAT_ACTIVE.id())?)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?,
                FIRST_CYCLE,
                "content stays in Legacy combat"
            );
            assert_eq!(
                replay
                    .game()
                    .varbit_value(u16::try_from(varbit::LEGACY_INTERFACE_MODE.id())?)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?,
                ZERO,
                "qualified normal interface preference"
            );
        }
        for control in plan["controlOperations"]
            .as_array()
            .context("native control timeline")?
        {
            if integer(&control["cycle"])? == cycle + FIRST_CYCLE {
                assert!(native_component_reachable(&mut replay, integer(&control["parent"])?, integer(&control["child"])?)? , "native operation must have attached visible ancestors and an unclipped centre before input at cycle {cycle}");
            }
        }
        assert_eq!(
            gameplay(&output.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: ordinary gameplay bytes"
        );
        for (opcode, payload) in client_frames(&output.written)? {
            if opcode == crate::proto::client::IF_BUTTON1 {
                let parent =
                    i32::from_be_bytes(payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?);
                if parent == component::backpack::SLOTS.packed() {
                    native_wears += ONE_NATIVE_WEAR;
                }
                if expected_upgrades.contains(&parent) {
                    observed_upgrades.insert(parent);
                }
                if [
                    component::slayer_rewards::MAGIC_DART_RUNES_BUY.packed(),
                    component::slayer_rewards::BROAD_ARROWS_BUY.packed(),
                    component::slayer_rewards::SLAYER_EXPERIENCE_BUY.packed(),
                    component::slayer_rewards::LEARN_FUSED_RINGS_BUY.packed(),
                ]
                .contains(&parent)
                {
                    native_rewards.insert(parent);
                }
            }
            if opcode == crate::proto::client::IF_BUTTON3 {
                let parent =
                    i32::from_be_bytes(payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?);
                let slot = (i32::from(payload[SLOT_HIGH]) << SHORT_HIGH_SHIFT)
                    | i32::from(payload[SLOT_LOW].wrapping_sub(ALTERNATE_LOW_BIAS));
                let item = i32::from(payload[ITEM_LOW].wrapping_sub(ALTERNATE_LOW_BIAS))
                    | (i32::from(payload[ITEM_HIGH]) << SHORT_HIGH_SHIFT);
                native_teleport |= parent == component::worn_equipment::SLOTS.packed()
                    && slot == i32::try_from(HEAD_SLOT)?
                    && item
                        == integer(
                            &plan["upgrades"][COMPLETE_TIER_COUNT - LAST_ITEM_OFFSET]
                                ["fusedOutput"],
                        )?;
            }
            if opcode == crate::proto::client::RESUME_PAUSEBUTTON {
                let parent = i32::from_be_bytes(PAUSE_COMPONENT_BYTES.map(|index| payload[index]));
                native_destination_choice |=
                    parent == component::dialogue_options::FIRST_OPTION.packed();
            }
            if opcode == crate::proto::client::CLIENT_CHEAT {
                let command = String::from_utf8_lossy(&payload);
                assert!(![
                    "hit ",
                    "setxp ",
                    "invset ",
                    "setvar ",
                    "ifopensub ",
                    "tele "
                ]
                .iter()
                .any(|bypass| command.contains(bypass)));
            }
        }
        if let Some(inventory) = ui(&mut replay)
            .engine
            .inv_cache
            .inventory(inv::WORN_EQUIPMENT.id(), false)
        {
            if let Some(item) = inventory.obj_ids.get(HEAD_SLOT) {
                if expected_items.contains(item) {
                    observed_items.insert(*item);
                }
            }
        }
    }
    anyhow::ensure!(
        ui(&mut replay).diagnostics.errors.is_empty(),
        "native hook errors: {:?}",
        ui(&mut replay).diagnostics.errors
    );
    assert_eq!(recorded_pings, replay_pings, "ICMP bytes in stream order");
    assert_eq!(native_wears, NATIVE_WEAR_OPERATIONS);
    assert_eq!(observed_upgrades, expected_upgrades);
    assert_eq!(observed_items, expected_items);
    assert_eq!(native_rewards.len(), COMPLETE_TIER_COUNT);
    assert!(native_teleport && native_destination_choice);
    assert_eq!(
        replay
            .game()
            .varbit_value(u16::try_from(varbit::SLAYER_REWARD_POINTS.id())?)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        integer(&plan["expectedPoints"])?
    );
    let stats = replay
        .game()
        .ui_variables
        .stats
        .as_ref()
        .context("native stats")?;
    assert_eq!(
        stats.stat_xp_actual(SLAYER_SKILL)?,
        integer(&plan["expectedXp"])?
    );
    let inventory = ui(&mut replay)
        .engine
        .inv_cache
        .inventory(inv::BACKPACK.id(), false)
        .context("native backpack")?;
    for stack in plan["expectedItems"]
        .as_array()
        .context("expected reward items")?
    {
        let item = integer(&stack["item"])?;
        let count = integer(&stack["count"])?;
        assert!(inventory
            .obj_ids
            .iter()
            .zip(&inventory.counts)
            .any(|(id, quantity)| *id == item && *quantity >= count));
    }
    Ok(())
}
