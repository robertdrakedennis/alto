//! Ordinary NPC ranged and magic attacks with real generated life points.
//! The client equips and protects through UI input; developer `opnpc` commands enter the
//! shared interaction path, and the live socket loop observes both ordinary deaths.
//! Socket E2E separately covers the player OPNPC2 network input.
use super::session_replay::{client_frames, mask_wall_clock, Replay, Trace};
use rs910_symbols::{npc, varbit};
use std::collections::BTreeSet;

const FIXTURE: &str = "fixtures/session-replay/npc-styles";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const WIZARD_PHASE: i32 = 1100;
const REQUIRED_ATTACKS: usize = 2;
const HEALTH_BAR_END: usize = 2;
const PROTECTED_GUARD_DAMAGE: i32 = 13;
const PROTECTED_WIZARD_DAMAGE: i32 = 24;

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_npc_ranged_and_magic_projectiles_protection_and_deaths() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut seen = BTreeSet::new();
    let mut dead = BTreeSet::new();
    let mut gone = BTreeSet::new();
    let mut damage = BTreeSet::new();
    let mut previous_life = None;
    let mut ranged_projectile = false;
    let mut magic_projectile = false;
    let mut protected_ranged = false;
    let mut protected_magic = false;
    let mut attacks = ZERO as usize;
    let mut replay_pings = Vec::new();
    let mut recorded_pings = Vec::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        // Camera telemetry belongs to omitted redraw/collision work; combat/gameplay
        // packets retain exact per-cycle equality. ICMP completions retain stream order.
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
                                && *opcode != crate::proto::client::EVENT_CAMERA_POSITION
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
            if opcode == crate::proto::client::CLIENT_CHEAT {
                attacks += usize::from(String::from_utf8_lossy(&payload).contains("opnpc "));
            }
        }
        let game = replay.game();
        let state = &game.runtime.feed.state;
        for enemy in state.npcs.entities.values() {
            seen.insert(enemy.type_id);
            if enemy.path.combat.as_ref().is_some_and(|combat| {
                combat.bars.iter().any(|bar| {
                    bar.updates
                        .iter()
                        .any(|update| update[HEALTH_BAR_END] == ZERO)
                })
            }) {
                dead.insert(enemy.type_id);
            }
        }
        for id in &dead {
            if !state
                .npcs
                .entities
                .values()
                .any(|enemy| enemy.type_id == *id)
            {
                gone.insert(*id);
            }
        }
        let projectiles = !state.zones.transients.projectiles.is_empty();
        if cycle < WIZARD_PHASE {
            ranged_projectile |= projectiles;
        } else {
            magic_projectile |= projectiles;
        }
        let bit = |id| {
            game.varbit_value(id)
                .map_err(|error| anyhow::anyhow!("{error:?}"))
        };
        protected_ranged |= bit(varbit::ACTIVE_PRAYER_RANGED_GUARD.id() as u16)? != ZERO;
        protected_magic |= bit(varbit::ACTIVE_PRAYER_MAGIC_GUARD.id() as u16)? != ZERO;
        let life = bit(varbit::CURRENT_LIFE_POINTS.id() as u16)?;
        match previous_life {
            Some(previous) if previous > life => {
                damage.insert(previous - life);
            }
            _ => {}
        }
        previous_life = Some(life);
    }
    assert_eq!(
        replay_pings, recorded_pings,
        "ICMP report bytes in stream order"
    );
    assert!(
        attacks >= REQUIRED_ATTACKS,
        "shared interaction attack commands: {attacks}"
    );
    assert!(
        ranged_projectile && magic_projectile,
        "ranged projectile={ranged_projectile}, magic projectile={magic_projectile}"
    );
    assert!(
        protected_ranged && protected_magic,
        "ranged protection={protected_ranged}, magic protection={protected_magic}"
    );
    for expected in [PROTECTED_GUARD_DAMAGE, PROTECTED_WIZARD_DAMAGE] {
        assert!(
            damage.contains(&expected),
            "protected damage {expected} absent: {damage:?}"
        );
    }
    for required in [npc::FALADOR_GUARD_ARCHER, npc::DARK_WIZARD_VARROCK] {
        assert!(
            seen.contains(&required.id()),
            "NPC {required:?} never reached the client"
        );
        // Ordinary population members may respawn; a zero headbar followed by removal proves death.
        assert!(
            dead.contains(&required.id()) && gone.contains(&required.id()),
            "NPC {required:?} did not die and leave: dead={dead:?}, gone={gone:?}"
        );
    }
    Ok(())
}
