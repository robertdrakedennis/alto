//! A dated Mazchna assignment completed through ordinary client operations.
//! The recorder fixes rolls and lowers Bat life points to bound capture time;
//! shared combat deaths, Slayer XP, saved progress and cached UI remain live.
use super::scenario_woodcutting::{message_game, update_stat};
use super::session_replay::{arrivals, client_frames, mask_wall_clock, Replay, Trace};
use super::*;
use rs910_symbols::{component, interface, varbit, varp, ComponentId};
use std::collections::BTreeSet;

const FIXTURE: &str = "fixtures/session-replay/slayer";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const INITIAL_TASK_COUNT: i32 = 40;
const PREVIOUS_STREAK: i32 = 4;
const COMPLETED_STREAK: i32 = 5;
const AWARDED_POINTS: i32 = 1;
const SLAYER_SKILL: i32 = 18;
const COMPONENT_OFFSET: usize = 4;
const COMPONENT_END: usize = 8;

fn counter_text(replay: &Replay, component: ComponentId) -> anyhow::Result<String> {
    let counter = replay
        .ui()
        .store
        .interfaces
        .get(&interface::SLAYER_COUNTER.id())
        .context("Slayer counter")?
        .borrow();
    let widget = counter
        .get(component.packed())?
        .context("counter component")?;
    let widget = widget.borrow();
    Ok(String::from_utf16_lossy(
        widget.f.text.as_deref().unwrap_or(&[]),
    ))
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_slayer_session_assigns_counts_completes_and_opens_rewards() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut counts = BTreeSet::new();
    let mut replay_pings = Vec::new();
    let mut recorded_pings = Vec::new();
    let mut assignment_operation = false;
    let mut gem_operation = false;
    let mut cached_target = false;
    let mut cached_count = false;
    let mut cached_streak = false;
    let mut rewards = false;
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        // The ICMP worker's completion cycle varies; compare report bytes in
        // stream order and retain exact per-cycle gameplay packet equality.
        let gameplay =
            |bytes: &[u8], pings: &mut Vec<Vec<u8>>| -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
                Ok(mask_wall_clock(bytes)?
                    .into_iter()
                    .filter(|(opcode, payload)| {
                        if *opcode == crate::proto::client::PING_STATISTICS {
                            pings.push(payload.clone());
                            false
                        } else {
                            *opcode != crate::proto::client::NO_TIMEOUT
                        }
                    })
                    .collect())
            };
        assert_eq!(
            gameplay(&out.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: gameplay packets differ"
        );
        for (opcode, payload) in client_frames(&out.written)? {
            assignment_operation |= opcode == crate::proto::client::RESUME_PAUSEBUTTON;
            if opcode == crate::proto::client::IF_BUTTON1 {
                let bytes: [u8; COMPONENT_END - COMPONENT_OFFSET] =
                    payload[COMPONENT_OFFSET..COMPONENT_END].try_into()?;
                gem_operation |= i32::from_be_bytes(bytes) == component::backpack::SLOTS.packed();
            }
        }
        let count = replay
            .game()
            .runtime
            .feed
            .state
            .varps
            .as_ref()
            .context("player variables")?
            .get(varp::SLAYER_TASK_COUNT.id())
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        let counter_mounted = replay
            .ui()
            .state
            .layout
            .subs
            .iter()
            .any(|(_, mounted)| *mounted == interface::SLAYER_COUNTER.id());
        if count > ZERO {
            counts.insert(count);
        }
        if count > ZERO && counter_mounted {
            cached_target |= counter_text(&replay, component::slayer_counter::TARGET)?
                .to_ascii_lowercase()
                .contains("bats");
            cached_count |= counter_text(&replay, component::slayer_counter::COUNT)?
                .contains(&count.to_string());
        }
        if counter_mounted {
            cached_streak |= counter_text(&replay, component::slayer_counter::STREAK)?
                .contains(&PREVIOUS_STREAK.to_string());
        }
        rewards |= replay
            .ui()
            .state
            .layout
            .subs
            .iter()
            .any(|(_, mounted)| *mounted == interface::SLAYER_REWARDS.id());
    }
    assert_eq!(replay_pings, recorded_pings, "ping reports in stream order");
    assert!(
        assignment_operation && gem_operation,
        "ordinary assignment and gem operations"
    );
    assert_eq!(
        counts,
        (FIRST_CYCLE..=INITIAL_TASK_COUNT).collect(),
        "every remaining-kill count reached the client"
    );
    assert!(
        cached_target && cached_count && cached_streak,
        "cached counter target={cached_target}, count={cached_count}, streak={cached_streak}"
    );
    assert!(rewards, "Slayer rewards window mounted");
    for (bit, expected) in [
        (varbit::SLAYER_REWARD_POINTS, AWARDED_POINTS),
        (varbit::SLAYER_TASK_STREAK, COMPLETED_STREAK),
    ] {
        assert_eq!(
            replay
                .game()
                .varbit_value(bit.id() as u16)
                .map_err(|error| anyhow::anyhow!("{error:?}"))?,
            expected,
            "{bit:?}"
        );
    }
    assert_eq!(
        replay
            .game()
            .runtime
            .feed
            .state
            .varps
            .as_ref()
            .context("player variables")?
            .get(varp::SLAYER_TASK_COUNT.id())
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        ZERO
    );
    let frames = arrivals(&trace)?;
    assert!(
        frames
            .iter()
            .filter(|frame| frame.opcode == crate::proto::server::UPDATE_STAT)
            .map(|frame| update_stat(&frame.payload))
            .any(|stat| stat[ZERO as usize] == SLAYER_SKILL && stat[FIRST_CYCLE as usize] > ZERO),
        "Slayer XP reached the client"
    );
    assert!(
        frames
            .iter()
            .filter(|frame| frame.opcode == crate::proto::server::MESSAGE_GAME)
            .map(|frame| message_game(&frame.payload).1)
            .any(|text| text.contains("completed your Slayer assignment")
                && text.contains("1 Slayer points")),
        "completion and awarded points reached the client"
    );
    Ok(())
}
