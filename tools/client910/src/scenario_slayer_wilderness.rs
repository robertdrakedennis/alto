//! Native reward input, equipped salt finishing and Wilderness obelisk travel.
//! Skills/points and the initial pad relocation are declared fixture setup;
//! attacks, reward spending, tool consumption and queued travel use live owners.
use super::scenario_tests::ui;
use super::session_replay::{client_frames, mask_wall_clock, Replay, Trace};
use anyhow::Context;
use rs910_symbols::{component, interface, inv, location, npc, obj, varbit};
use serde_json::Value;

const FIXTURE: &str = "fixtures/session-replay/slayer-wilderness";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const FIRST_SLOT: usize = 0;
const MAIN_HAND_SLOT: usize = 3;
const STACK_COUNT_INDEX: usize = 1;
const COMPONENT_OFFSET: usize = 4;
const COMPONENT_END: usize = 8;
const HIT_DAMAGE_INDEX: usize = 1;
const HEALTH_BAR_END: usize = 2;
const SINGLE_FINISHING_KILL: usize = 1;
const NPC_HIGH_BYTE: usize = 1;
const NPC_LOW_BYTE: usize = 2;
const BYTE_BITS: u32 = 8;
const ALTERNATE_BYTE_BIAS: u8 = 128;
const UI_READY: i32 = 1300;

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

fn verify_receipts(root: &std::path::Path) -> anyhow::Result<usize> {
    let plan: Value = serde_json::from_str(&std::fs::read_to_string(root.join("plan.json"))?)?;
    assert_eq!(plan["slug"], npc::ROCK_SLUG.id());
    assert_eq!(plan["salt"], obj::BAG_OF_SALT.id());
    assert_eq!(plan["weapon"], obj::ABYSSAL_WHIP.id());
    let rows: Vec<Value> = std::fs::read_to_string(root.join("combat-receipts.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let deaths: Vec<_> = rows.iter().filter(|row| row["kind"] == "death").collect();
    assert_eq!(deaths.len(), SINGLE_FINISHING_KILL);
    assert_eq!(deaths[FIRST_SLOT]["targetDefinition"], npc::ROCK_SLUG.id());
    assert!(rows.iter().any(|row| row["kind"] == "launch"
        && row["source"] == "player"
        && row["targetId"] == deaths[FIRST_SLOT]["targetId"]
        && row["damage"]
            .as_i64()
            .is_some_and(|damage| damage > i64::from(ZERO))));
    let players: Vec<_> = rows
        .iter()
        .filter(|row| row["kind"] == "state")
        .filter_map(|row| {
            row["players"]
                .as_array()
                .and_then(|players| players.first())
        })
        .collect();
    assert!(players
        .iter()
        .any(|player| player["quick"] == true && player["slayer"]["points"] == ZERO));
    let has_salt = |player: &&Value| {
        player["backpack"].as_array().is_some_and(|slots| {
            slots.iter().any(|slot| {
                slot[FIRST_SLOT] == obj::BAG_OF_SALT.id()
                    && slot[STACK_COUNT_INDEX]
                        .as_i64()
                        .is_some_and(|count| count > i64::from(ZERO))
            })
        })
    };
    assert!(
        players.iter().any(has_salt),
        "salt was not in the initial inventory"
    );
    assert!(
        players
            .iter()
            .any(|player| player["quick"] == true && !has_salt(player)),
        "salt consumption absent"
    );
    for field in ["pad", "expectedPad"] {
        assert!(
            players
                .iter()
                .any(|player| player["x"] == plan[field]["centre"]["x"]
                    && player["z"] == plan[field]["centre"]["z"]
                    && player["level"] == plan[field]["centre"]["level"]
                    && player["wilderness"] == plan[field]["level"]),
            "pad {field} was not observed"
        );
    }
    usize::try_from(
        deaths[FIRST_SLOT]["targetId"]
            .as_u64()
            .context("finishing target id")?,
    )
    .context("finishing target index")
}

fn native_level_text(replay: &mut Replay) -> anyhow::Result<Option<String>> {
    const NO_CHILD: i32 = -1;
    const PACKED_GROUP_BITS: u32 = 16;
    const MAX_VISIBLE_ANCESTORS: usize = 64;
    let state = ui(replay);
    let Some(first) = state
        .store
        .get(component::wilderness_hud::LEVEL_TEXT.packed(), NO_CHILD)?
    else {
        return Ok(None);
    };
    let Some(text) = first
        .borrow()
        .f
        .text
        .as_ref()
        .map(|text| String::from_utf16_lossy(text))
    else {
        return Ok(None);
    };
    let mut current = first;
    let mut visited = std::collections::BTreeSet::new();
    for _ in ZERO as usize..MAX_VISIBLE_ANCESTORS {
        anyhow::ensure!(
            visited.insert(std::rc::Rc::as_ptr(&current) as usize),
            "Wilderness HUD ancestry cycle"
        );
        if !rs910_ui::ui_hooks::attached(&mut state.store, &current)? {
            return Ok(None);
        }
        let component = current.borrow();
        if component.runtime_entry_hidden().unwrap_or(component.f.hide)
            || component.f.width <= ZERO
            || component.f.height <= ZERO
        {
            return Ok(None);
        }
        let rect = [
            component.f.x,
            component.f.y,
            component.f.width,
            component.f.height,
        ];
        let runtime_parent = component.runtime_parent();
        let layer = component.f.layer;
        let group = (component.f.parentlayer as u32 >> PACKED_GROUP_BITS) as i32;
        drop(component);
        let parent = if let Some(parent) = runtime_parent {
            Some(parent)
        } else if layer != NO_CHILD {
            state.store.get(layer, NO_CHILD)?
        } else if group == state.state.life.top {
            return Ok(Some(text));
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
        let Some(parent) = parent else {
            return Ok(None);
        };
        let width = parent.borrow().f.width;
        let height = parent.borrow().f.height;
        const X: usize = 0;
        const Y: usize = 1;
        const WIDTH: usize = 2;
        const HEIGHT: usize = 3;
        if rect[X] < ZERO
            || rect[Y] < ZERO
            || rect[X] + rect[WIDTH] > width
            || rect[Y] + rect[HEIGHT] > height
        {
            return Ok(None);
        }
        current = parent;
    }
    anyhow::bail!("Wilderness HUD ancestry exceeds its recorded bound")
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_native_slayer_reward_finisher_and_wilderness_obelisk() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let finishing_target = verify_receipts(&root)?;
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    let mut native_attack = false;
    let mut bought = false;
    let mut walked = false;
    let mut fought = false;
    let mut hit = false;
    let mut corpse = false;
    let mut initial_salt = false;
    let mut final_salt = false;
    let mut south = false;
    let mut destination = false;
    let mut wilderness_hud_mounted = false;
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        let recorded = trace.bytes(b"OUT ", cycle);
        let recorded_frames = client_frames(&recorded)?;
        let picked = recorded_frames
            .iter()
            .any(|(opcode, _)| *opcode == crate::proto::client::OPNPC2);
        for (opcode, payload) in &recorded_frames {
            if *opcode == crate::proto::client::OPNPC2 {
                let target = usize::from(payload[NPC_HIGH_BYTE]) << BYTE_BITS
                    | usize::from(payload[NPC_LOW_BYTE].wrapping_sub(ALTERNATE_BYTE_BIAS));
                assert_eq!(
                    target, finishing_target,
                    "native Attack must identify the observed finisher target"
                );
                native_attack = true;
            }
        }
        let actual = gameplay(&output.written, &mut replay_pings)?;
        let expected = gameplay(&recorded, &mut recorded_pings)?;
        if picked {
            // The recording owns the renderer's body pick. Headless replay
            // omits that pick; compare all other packets in this exact cycle.
            let beside_pick = |frames: Vec<(u8, Vec<u8>)>| {
                frames
                    .into_iter()
                    .filter(|(opcode, _)| {
                        ![
                            crate::proto::client::OPNPC2,
                            crate::proto::client::MOVE_GAMECLICK,
                        ]
                        .contains(opcode)
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                beside_pick(actual),
                beside_pick(expected),
                "cycle {cycle}: packets beside recorded body pick"
            );
        } else {
            assert_eq!(actual, expected, "cycle {cycle}: gameplay bytes");
        }
        for (opcode, payload) in client_frames(&output.written)? {
            if opcode == crate::proto::client::IF_BUTTON1 {
                let packed =
                    i32::from_be_bytes(payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?);
                bought |= packed == component::slayer_rewards::LEARN_QUICK_KILLS_BUY.packed();
            }
            walked |= opcode == crate::proto::client::MOVE_MINIMAPCLICK;
            if opcode == crate::proto::client::CLIENT_CHEAT {
                let command = String::from_utf8_lossy(&payload);
                assert!(!["hit ", "setxp ", "invset "]
                    .iter()
                    .any(|bypass| command.contains(bypass)));
                fought |= command.contains("opnpc ");
            }
        }
        let game = replay.game();
        for enemy in game.runtime.feed.state.npcs.entities.values() {
            if enemy.type_id == npc::ROCK_SLUG.id() {
                if let Some(combat) = &enemy.path.combat {
                    hit |= combat.hits.iter().any(|hit| hit[HIT_DAMAGE_INDEX] > ZERO);
                    corpse |= combat.bars.iter().any(|bar| {
                        bar.updates
                            .iter()
                            .any(|update| update[HEALTH_BAR_END] == ZERO)
                    });
                }
            }
        }
        if let Some(me) = game
            .runtime
            .feed
            .state
            .players
            .players
            .get(game.runtime.map.local)
            .and_then(Option::as_ref)
        {
            let at = |tile: rs910_symbols::Location| {
                game.runtime.map.base_x + me.x[FIRST_SLOT] == tile.x()
                    && game.runtime.map.base_z + me.z[FIRST_SLOT] == tile.z()
                    && me.level == tile.level()
            };
            south |= at(location::WILDERNESS_OBELISK_SOUTH) && me.appearance.wilderness > ZERO;
            destination |=
                at(location::WILDERNESS_OBELISK_LAVA_MAZE) && me.appearance.wilderness > ZERO;
        }
        if cycle == UI_READY {
            assert_eq!(
                game.varbit_value(varbit::SLAYER_QUICK_KILLS_UNLOCKED.id() as u16)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?,
                FIRST_CYCLE
            );
            assert_eq!(
                game.varbit_value(varbit::SLAYER_REWARD_POINTS.id() as u16)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?,
                ZERO
            );
        }
        wilderness_hud_mounted |= replay
            .ui()
            .state
            .layout
            .subs
            .iter()
            .any(|&(parent, group)| {
                parent == component::game_window::WILDERNESS_HUD_SLOT.packed()
                    && group == interface::WILDERNESS_HUD.id()
            });
        if cycle == UI_READY || cycle == trace.last_cycle() {
            let inventory = ui(&mut replay)
                .engine
                .inv_cache
                .inventory(inv::BACKPACK.id(), false)
                .context("backpack")?;
            let salt = inventory.obj_ids.contains(&obj::BAG_OF_SALT.id());
            if cycle == UI_READY {
                initial_salt = salt;
            } else {
                final_salt = salt;
            }
        }
    }
    assert_eq!(recorded_pings, replay_pings, "ICMP bytes in stream order");
    assert!(native_attack && bought && walked && fought && hit && corpse);
    assert!(initial_salt && !final_salt && south && destination && wilderness_hud_mounted);
    let plan: Value = serde_json::from_str(&std::fs::read_to_string(root.join("plan.json"))?)?;
    let level = plan["expectedPad"]["level"]
        .as_i64()
        .context("destination Wilderness level")?;
    assert_eq!(
        native_level_text(&mut replay)?,
        Some(format!("Level: {level}")),
        "native timer text must be visible through every ancestor"
    );
    let worn = ui(&mut replay)
        .engine
        .inv_cache
        .inventory(inv::WORN_EQUIPMENT.id(), false)
        .context("worn inventory")?;
    assert_eq!(worn.obj_ids[MAIN_HAND_SLOT], obj::ABYSSAL_WHIP.id());
    Ok(())
}
