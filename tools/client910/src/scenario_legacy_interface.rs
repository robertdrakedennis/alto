//! Ordinary recorded Legacy HUD: allowed Settings, native pointer input,
//! fixed resources, corpse-time XP and normal item-window selection.
use super::scenario_legacy_combat::gameplay;
use super::scenario_tests::{component_rect, ui, walk_components};
use super::scenario_woodcutting::{cs2_ints, update_stat};
use super::session_replay::{arrivals, client_frames, Replay, Trace};
use anyhow::Context;
use native910::{vars::VarScope, vm::Value};
use rs910_symbols::{component, interface, npc, param, varbit, varc, varp};
use std::collections::BTreeSet;

const FIXTURE: &str = "fixtures/session-replay/legacy-interface";
const FIRST_CYCLE: i32 = 1;
const LAST_CYCLE: i32 = 3000;
const ZERO: i32 = 0;
const ENABLED: i32 = 1;
const COMBAT_SKILLS: usize = 4;
const COMPONENT_OFFSET: usize = 4;
const COMPONENT_END: usize = 8;
const GROUP_BITS: i32 = 16;
const READY: i32 = 1350;
const REWARDED: i32 = 2000;
const NORMAL: i32 = 2300;
const RESTORED: i32 = 2750;
const RETALIATION_OFF: i32 = 1;
const AGGRESSIVE_STANCE: i32 = 3;
const ATTACK_SKILL: i32 = 0;
const DEFENCE_SKILL: i32 = 1;
const STRENGTH_SKILL: i32 = 2;
const TRAINING_MASK: i32 = 7;
const INITIAL_XP: [i32; COMBAT_SKILLS] = [1154; COMBAT_SKILLS];
const EARNED_XP: [i32; COMBAT_SKILLS] = [1162; COMBAT_SKILLS];
const MELEE_DAMAGE: i32 = 111;
const CAPTURE_WIDTH: i32 = 1024;
const CAPTURE_HEIGHT: i32 = 768;
const XP_DROP: &str = "+32 xp";
const INITIAL_STANCE: i32 = 1;
const BACKPACK_IMAGE: i32 = 2400;
const WORN_IMAGE: i32 = 2500;
const BACKPACK_WINDOW: i32 = 2;
const WORN_WINDOW: i32 = 3;
const CENTRE_DIVISOR: i32 = 2;
const RECT_X: usize = 0;
const RECT_Y: usize = 1;
const RECT_WIDTH: usize = 2;
const RECT_HEIGHT: usize = 3;
const REQUIRED_TABS: usize = 2;
const LP_DISPLAY: &str = "100";
const INITIAL_LP_DISPLAY: &str = "100/100";

fn integer(replay: &mut Replay, scope: VarScope, id: i32) -> anyhow::Result<i32> {
    let game = replay
        .core
        .session
        .as_mut()
        .context("session")?
        .game
        .as_mut()
        .context("game")?;
    match crate::client_game::with_game(game, |vars| vars.get(scope, id as u16, false))? {
        Value::Int(value) => Ok(value),
        value => anyhow::bail!("native variable type {value:?}"),
    }
}

fn player_bit(replay: &Replay, id: i32) -> anyhow::Result<i32> {
    replay
        .game()
        .varbit_value(id as u16)
        .map_err(|error| anyhow::anyhow!("{error:?}"))
}

fn visible_xp_drop(replay: &mut Replay) -> Option<[i32; 4]> {
    let mut found = None;
    walk_components(ui(replay), &mut |rect, c| {
        let group = c.f.parentlayer >> GROUP_BITS;
        let [x, y, width, height] = rect;
        if (group == interface::GAME_WINDOW.id() || group == interface::EXPERIENCE_DROPS.id())
            && width > ZERO
            && height > ZERO
            && x < CAPTURE_WIDTH
            && y < CAPTURE_HEIGHT
            && x + width > ZERO
            && y + height > ZERO
            && String::from_utf16_lossy(c.f.text.as_deref().unwrap_or_default()).contains(XP_DROP)
        {
            found = Some(rect);
        }
    });
    found
}

fn verify_rewards(root: &std::path::Path) -> anyhow::Result<()> {
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(root.join("hud-receipts.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let launches: Vec<_> = rows
        .iter()
        .filter(|row| row["kind"] == "launch" && row["source"] == "player")
        .collect();
    assert_eq!(launches.len(), ENABLED as usize);
    let launch = launches.first().context("ordinary launch")?;
    assert_eq!(launch["damage"], MELEE_DAMAGE);
    assert_eq!(
        launch["training"]["stats"],
        serde_json::json!([ATTACK_SKILL, STRENGTH_SKILL, DEFENCE_SKILL])
    );
    assert_eq!(launch["training"]["timing"], "death-animation");
    let death = rows
        .iter()
        .find(|row| row["kind"] == "death")
        .context("death")?;
    assert_eq!(death["targetDefinition"], npc::CHICKEN.id());
    let tick = death["tick"].as_i64().context("death tick")?;
    let hide = death["hideTick"].as_i64().context("hide tick")?;
    assert!(hide > tick);
    let npc_states = rows
        .iter()
        .filter(|row| row["kind"] == "state" && row["phase"] == "npcs");
    let before = npc_states
        .clone()
        .rev()
        .find(|row| {
            row["tick"]
                .as_i64()
                .is_some_and(|at| at >= tick && at < hide)
        })
        .context("visible corpse state")?;
    let after = npc_states
        .clone()
        .find(|row| row["tick"] == hide)
        .context("corpse disappearance state")?;
    assert_eq!(
        before["rows"][ZERO as usize]["xp"],
        serde_json::json!(INITIAL_XP)
    );
    assert_eq!(
        after["rows"][ZERO as usize]["xp"],
        serde_json::json!(EARNED_XP)
    );
    fn enemy<'a>(
        state: &'a serde_json::Value,
        target: &serde_json::Value,
    ) -> anyhow::Result<&'a serde_json::Value> {
        state["enemies"]
            .as_array()
            .context("enemies")?
            .iter()
            .find(|enemy| enemy["id"] == *target)
            .context("dead target")
    }
    assert_eq!(enemy(before, &death["targetId"])?["visible"], true);
    assert_eq!(enemy(after, &death["targetId"])?["visible"], false);
    assert_eq!(
        enemy(before, &death["targetId"])?["generation"],
        enemy(after, &death["targetId"])?["generation"]
    );
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_legacy_hud_native_training_retaliation_layout_resources_and_xp() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    verify_rewards(&root)?;
    let trace = Trace::load(&root.join("session.rtr"))?;
    assert_eq!(trace.last_cycle(), LAST_CYCLE);
    let packets = arrivals(&trace)?;
    let mut replay = Replay::start(&trace)?;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    let mut controls = BTreeSet::new();
    let mut skill_updates = BTreeSet::new();
    let mut processed = ZERO as usize;
    let mut drop_receipt = None;
    let mut drop_texts = BTreeSet::new();
    for cycle in FIRST_CYCLE..=LAST_CYCLE {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            gameplay(&out.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: gameplay bytes"
        );
        for (opcode, payload) in client_frames(&out.written)? {
            if opcode == crate::proto::client::IF_BUTTON1 {
                controls.insert(i32::from_be_bytes(
                    payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?,
                ));
            }
        }
        let done = replay.processed(&packets);
        for packet in packets[processed..done]
            .iter()
            .filter(|packet| packet.opcode == crate::proto::server::UPDATE_STAT)
        {
            let [skill, xp, _] = update_stat(&packet.payload);
            if (ZERO..COMBAT_SKILLS as i32).contains(&skill) && xp == EARNED_XP[skill as usize] {
                skill_updates.insert(skill);
            }
        }
        processed = done;
        walk_components(ui(&mut replay), &mut |_, c| {
            let group = c.f.parentlayer >> GROUP_BITS;
            if group == interface::EXPERIENCE_DROPS.id() {
                drop_texts.insert(String::from_utf16_lossy(
                    c.f.text.as_deref().unwrap_or_default(),
                ));
            }
        });
        if let Some(rect) = visible_xp_drop(&mut replay) {
            drop_receipt.get_or_insert((cycle, rect));
        }
        if [
            READY,
            REWARDED,
            NORMAL,
            BACKPACK_IMAGE,
            WORN_IMAGE,
            RESTORED,
            LAST_CYCLE,
        ]
        .contains(&cycle)
        {
            assert_eq!(
                player_bit(&replay, varbit::LEGACY_COMBAT_ACTIVE.id())?,
                ENABLED
            );
            assert_eq!(
                player_bit(&replay, varbit::SLIM_WINDOW_HEADERS.id())?,
                ENABLED
            );
            assert_eq!(
                player_bit(&replay, varbit::LEGACY_COMBAT_STANCE.id())?,
                if cycle == READY {
                    INITIAL_STANCE
                } else {
                    AGGRESSIVE_STANCE
                }
            );
            assert_eq!(
                player_bit(&replay, varbit::MELEE_TRAINING_MASK.id())?,
                TRAINING_MASK
            );
            let fixed = ![NORMAL, BACKPACK_IMAGE, WORN_IMAGE].contains(&cycle);
            assert_eq!(
                player_bit(&replay, varbit::LEGACY_INTERFACE_MODE.id())?,
                i32::from(fixed)
            );
            assert_eq!(
                integer(
                    &mut replay,
                    VarScope::Client,
                    varc::LEGACY_TRAINING_PANE.id()
                )?,
                i32::from(fixed)
            );
            assert_eq!(
                integer(
                    &mut replay,
                    VarScope::Player,
                    varp::AUTO_RETALIATE_DISABLED.id()
                )?,
                if cycle > READY && cycle < LAST_CYCLE {
                    RETALIATION_OFF
                } else {
                    ZERO
                }
            );
            let xp: Vec<_> = (ZERO..COMBAT_SKILLS as i32)
                .flat_map(|stat| [("push_constant_int", stat), ("stat_visible_xp", ZERO)])
                .collect();
            assert_eq!(
                cs2_ints(&mut replay, &xp)?,
                if cycle == READY {
                    INITIAL_XP
                } else {
                    EARNED_XP
                }
            );
            let rt = ui(&mut replay);
            if fixed {
                for parent in [
                    component::legacy_combat::AGGRESSIVE_STANCE,
                    component::legacy_combat::BALANCED_STANCE,
                    component::legacy_combat::DEFENSIVE_STANCE,
                    component::legacy_combat::SPECIAL_ATTACK,
                    component::legacy_combat::RETALIATION_TOGGLE,
                ] {
                    let rect = component_rect(rt, &|c| c.f.parentlayer == parent.packed())
                        .context("native fixed control")?;
                    assert!(rect[RECT_X] >= ZERO && rect[RECT_Y] >= ZERO);
                    assert!(rect[RECT_X] + rect[RECT_WIDTH] <= CAPTURE_WIDTH);
                    assert!(rect[RECT_Y] + rect[RECT_HEIGHT] <= CAPTURE_HEIGHT);
                }
            } else {
                let mut tabs = Vec::new();
                walk_components(rt, &mut |rect, c| {
                    if let Some((_, crate::ui_components::Arg::Int(destination))) =
                        c.params.as_ref().and_then(|values| {
                            values.iter().find(|(id, _)| {
                                *id == i64::from(param::WINDOW_TAB_DESTINATION.id())
                            })
                        })
                    {
                        if [BACKPACK_WINDOW, WORN_WINDOW].contains(destination) {
                            tabs.push(serde_json::json!({"destination":destination,"parent":c.f.parentlayer,"child":c.f.id,"rect":rect,"centre":[rect[RECT_X]+rect[RECT_WIDTH]/CENTRE_DIVISOR,rect[RECT_Y]+rect[RECT_HEIGHT]/CENTRE_DIVISOR]}));
                        }
                    }
                });
                assert_eq!(tabs.len(), REQUIRED_TABS);
                let selected = if cycle == WORN_IMAGE {
                    component::worn_equipment::SLOTS
                } else {
                    component::backpack::SLOTS
                };
                if [BACKPACK_IMAGE, WORN_IMAGE].contains(&cycle) {
                    assert!(
                        component_rect(rt, &|c| c.f.parentlayer == selected.packed()).is_some(),
                        "actual selected grid {cycle}"
                    );
                }
                println!(
                    "HUD_RECORDED_TABS {}",
                    serde_json::json!({"cycle":cycle,"tabs":tabs})
                );
            }
            if fixed {
                let expected_life = if cycle == READY {
                    INITIAL_LP_DISPLAY
                } else {
                    LP_DISPLAY
                };
                let rect = component_rect(rt, &|c| {
                    c.f.parentlayer == component::legacy_life::VALUE_ROOT.packed()
                        && c.f.layer == component::legacy_life::VALUE_ROOT.packed()
                        && String::from_utf16_lossy(c.f.text.as_deref().unwrap_or_default()).trim()
                            == expected_life
                })
                .context("exact native LP text beneath its cache-owned value layer")?;
                assert!(rect[RECT_WIDTH] > ZERO && rect[RECT_HEIGHT] > ZERO);
            }
        }
    }
    assert!(
        replay.ui().diagnostics.errors.is_empty(),
        "native hook errors: {:?}",
        replay.ui().diagnostics.errors
    );
    assert_eq!(skill_updates.len(), COMBAT_SKILLS);
    assert!(
        drop_receipt.is_some(),
        "exact +32 xp in actual visible native layer walk: {drop_texts:?}"
    );
    println!("HUD_RECORDED_XP_DROP {drop_receipt:?}");
    for parent in [
        component::legacy_combat::AGGRESSIVE_STANCE,
        component::legacy_combat::RETALIATION_TOGGLE,
        component::gameplay_settings::OPTIONS,
    ] {
        assert!(controls.contains(&parent.packed()));
    }
    Ok(())
}
