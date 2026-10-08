//! Recorded native XP drops, pouch rendering, and cache-qualified NPC action playback.
use super::scenario_tests::ui;
use super::scenario_woodcutting::all_components;
use super::session_replay::{mask_wall_clock, Replay, Trace};
use super::*;
use rs910_symbols::{interface, inv, seq};
use std::collections::{BTreeMap, BTreeSet};

const FIRST_CYCLE: i32 = 1;
const GROUP_BITS: i32 = 16;
const INITIAL_COINS: i32 = 1000;
const FINAL_COINS: i32 = 3500;
const INITIAL_DISPLAY: &str = "1000";
const FINAL_DISPLAY: &str = "3500";
const WOOD_DROP: &str = "+25 xp";
const FARM_DROP: &str = "+4 xp";
const CLIENT_CYCLES_PER_TICK: i32 = 30;
const DELIVERY_TOLERANCE: i32 = CLIENT_CYCLES_PER_TICK * 2;
const FIGHTS: [(&str, i32, i32, i32, i32); 3] = [
    (
        "Cow",
        seq::COW_ATTACK.id(),
        seq::COW_DEFEND.id(),
        seq::COW_DEATH.id(),
        4,
    ),
    (
        "Goblin",
        seq::GOBLIN_ATTACK.id(),
        seq::GOBLIN_BLOCK.id(),
        seq::GOBLIN_DEATH.id(),
        3,
    ),
    (
        "Chicken",
        seq::CHICKEN_ATTACK.id(),
        seq::CHICKEN_DEFEND.id(),
        seq::CHICKEN_GROUND_DEATH.id(),
        3,
    ),
];

fn trace_of(name: &str) -> anyhow::Result<Trace> {
    Trace::load(
        &rs910_core::test_support::client_dir()
            .join("fixtures/session-replay")
            .join(name)
            .join("session.rtr"),
    )
}

fn visible_texts(replay: &mut Replay) -> Vec<(i32, String)> {
    all_components(ui(replay))
        .into_iter()
        .filter_map(|component| {
            let component = component.borrow();
            let fields = &component.f;
            (!fields.hide && fields.width > 0 && fields.height > 0).then(|| {
                (
                    fields.parentlayer >> GROUP_BITS,
                    String::from_utf16_lossy(fields.text.as_deref().unwrap_or_default()),
                )
            })
        })
        .collect()
}

fn check_packets(trace: &Trace, cycle: i32, written: &[u8]) -> anyhow::Result<()> {
    let gameplay = |bytes: &[u8]| -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
        Ok(mask_wall_clock(bytes)?
            .into_iter()
            .filter(|(opcode, _)| {
                ![
                    crate::proto::client::PING_STATISTICS,
                    crate::proto::client::NO_TIMEOUT,
                ]
                .contains(opcode)
            })
            .collect())
    };
    assert_eq!(
        gameplay(written)?,
        gameplay(&trace.bytes(b"OUT ", cycle))?,
        "cycle {cycle}: gameplay packets"
    );
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_correctness_wood_drop_and_pouch_change() -> anyhow::Result<()> {
    let trace = trace_of("correctness-wood")?;
    let mut replay = Replay::start(&trace)?;
    let mut drop_seen = false;
    let mut balances = BTreeSet::new();
    let mut displays = BTreeSet::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        check_packets(&trace, cycle, &output.written)?;
        if let Some(wallet) = replay
            .ui()
            .engine
            .inv_cache
            .inventory(inv::COIN_WALLET.id(), false)
        {
            balances.extend(wallet.counts.iter().copied());
        }
        for (group, text) in visible_texts(&mut replay) {
            if group == interface::GAME_WINDOW.id() || group == interface::EXPERIENCE_DROPS.id() {
                drop_seen |= text.contains(WOOD_DROP);
            }
            if group == interface::BACKPACK.id() {
                displays.insert(text.replace([' ', ','], ""));
            }
        }
    }
    assert!(drop_seen, "native visible +25 xp component after chopping");
    assert!(balances.contains(&INITIAL_COINS) && balances.contains(&FINAL_COINS));
    assert!(
        displays.iter().any(|text| text.contains(INITIAL_DISPLAY))
            && displays.iter().any(|text| text.contains(FINAL_DISPLAY)),
        "native pouch display texts: {displays:?}"
    );
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_correctness_farm_drop() -> anyhow::Result<()> {
    let trace = trace_of("correctness-farm")?;
    let mut replay = Replay::start(&trace)?;
    let mut drop_seen = false;
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        check_packets(&trace, cycle, &output.written)?;
        drop_seen |= visible_texts(&mut replay).iter().any(|(group, text)| {
            (*group == interface::GAME_WINDOW.id() || *group == interface::EXPERIENCE_DROPS.id())
                && text.contains(FARM_DROP)
        });
    }
    assert!(
        drop_seen,
        "native visible +4 xp component while clearing weeds"
    );
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_correctness_npc_actions_and_death_timing() -> anyhow::Result<()> {
    let trace = trace_of("correctness-npcs")?;
    let mut replay = Replay::start(&trace)?;
    let mut sequences = BTreeMap::<String, BTreeSet<i32>>::new();
    let mut dying = BTreeMap::<usize, (String, i32, i32)>::new();
    let mut removed = BTreeSet::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        check_packets(&trace, cycle, &output.written)?;
        let entities = &replay.game().runtime.feed.state.npcs.entities;
        for (&index, npc) in entities {
            if let Some((name, _, _, death, ticks)) =
                FIGHTS.iter().find(|(name, ..)| *name == npc.name)
            {
                let animation = npc.path.animation.main.id();
                sequences
                    .entry((*name).into())
                    .or_default()
                    .insert(animation);
                if animation == *death {
                    dying
                        .entry(index)
                        .or_insert(((*name).into(), cycle, *ticks));
                }
            }
        }
        for (&index, (name, started, ticks)) in &dying {
            if !entities.contains_key(&index) && !removed.contains(name) {
                let elapsed = cycle - started;
                let duration = ticks * CLIENT_CYCLES_PER_TICK;
                assert!(
                    (duration - DELIVERY_TOLERANCE..=duration + DELIVERY_TOLERANCE)
                        .contains(&elapsed),
                    "{name}: despawn after {elapsed} cycles, cache duration {duration}"
                );
                removed.insert(name.clone());
            }
        }
    }
    for (name, attack, _defend, death, _) in FIGHTS {
        let seen = sequences
            .get(name)
            .with_context(|| format!("{name} was not in view"))?;
        assert!(seen.contains(&attack), "{name} attack: {seen:?}");
        assert!(seen.contains(&death), "{name} death: {seen:?}");
        assert!(
            removed.contains(name),
            "{name} body removed after its death sequence"
        );
    }
    Ok(())
}
