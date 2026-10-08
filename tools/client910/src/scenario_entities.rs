//! Entity presentation, client side: a recorded dev-server session in which
//! the real client meets each entity event once (worn item, spot animations,
//! overhead icons, tint, hits, a hint arrow, a new NPC), replayed through the
//! production owners (`session_replay`).
//!
//! Fixture: `fixtures/session-replay/entities/` (`record.sh`; the release
//! client with `CLIENT910_RECORD` against the dev lobby/world, one developer
//! command per 100 cycles).
use super::session_replay::{mask_wall_clock, Replay, Trace};
use rs910_symbols::{obj, spot};

const FIXTURE: &str = "fixtures/session-replay/entities";
/// The NPC the commands address: the first development spawn.
const HANS: usize = 1;
const PRAYER_ICONS: i32 = 440;
/// Before the last command (`npcadd`, sent at about cycle 1000).
const NEW_NPC_AFTER: i32 = 950;

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_entity_session_reaches_every_presentation_state() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut seen = std::collections::BTreeMap::<&str, i32>::new();
    let mut npcs_before = 0;
    let mut note = |what: &'static str, cycle: i32, holds: bool| {
        if holds {
            seen.entry(what).or_insert(cycle);
        }
    };
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        assert_eq!(
            mask_wall_clock(&out.written)?,
            mask_wall_clock(&trace.bytes(b"OUT ", cycle))?,
            "cycle {cycle}: client packets differ from the recording"
        );
        let game = replay.game();
        let players = &game.runtime.feed.state.players;
        let Some(me) = players
            .players
            .get(game.runtime.map.local)
            .and_then(Option::as_ref)
        else {
            continue;
        };
        let npcs = &game.runtime.feed.state.npcs;
        note(
            "worn staff",
            cycle,
            me.appearance
                .model
                .as_ref()
                .is_some_and(|m| m.kits.contains(&(0x4000_0000 | obj::STAFF_OF_AIR.id()))),
        );
        note(
            "player spot",
            cycle,
            me.animation.spots[0].id == spot::UNDERGROWTH_BANNER.id(),
        );
        note(
            "player head icon",
            cycle,
            (me.appearance.head_ids[0], me.appearance.head_groups[0]) == (0, PRAYER_ICONS),
        );
        note(
            "player tint",
            cycle,
            me.tint[3] != 0 && me.tint[4] > cycle - 100,
        );
        note(
            "player hit",
            cycle,
            me.combat
                .as_ref()
                .is_some_and(|c| c.hits.iter().any(|hit| hit[1] == 7)),
        );
        if let Some(hans) = npcs.entities.get(&HANS) {
            note(
                "npc spot",
                cycle,
                hans.path.animation.spots[0].id == spot::UNDERGROWTH_BANNER.id(),
            );
            note(
                "npc hit",
                cycle,
                hans.path
                    .combat
                    .as_ref()
                    .is_some_and(|c| c.hits.iter().any(|hit| hit[1] == 12)),
            );
            note(
                "npc head icon",
                cycle,
                hans.head_icons
                    .is_some_and(|(groups, icons)| (groups[0], icons[0]) == (PRAYER_ICONS, 1)),
            );
        }
        note(
            "hint arrow",
            cycle,
            replay
                .minimap
                .hint_arrow_state
                .iter()
                .flatten()
                .any(|arrow| arrow.hint_type == 1 && arrow.npc_index == Some(HANS)),
        );
        // The last command adds a fresh NPC, which starts its fade-in.
        if cycle == NEW_NPC_AFTER {
            npcs_before = npcs.entities.len();
        }
        note(
            "new npc",
            cycle,
            cycle > NEW_NPC_AFTER
                && npcs.entities.len() == npcs_before + 1
                && npcs
                    .entities
                    .values()
                    .any(|npc| npc.fade_alpha == 255 && npc.fade_start > NEW_NPC_AFTER),
        );
    }
    for what in [
        "worn staff",
        "player spot",
        "player head icon",
        "player tint",
        "player hit",
        "npc spot",
        "npc hit",
        "npc head icon",
        "hint arrow",
        "new npc",
    ] {
        assert!(
            seen.contains_key(what),
            "{what} never reached the client state: {seen:?}"
        );
    }
    Ok(())
}
