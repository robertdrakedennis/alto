//! Cached boss setup, a normal 45,000-life KBD fight and owned ground loot.
//! The recording fixes random draws, with all health, equipment, death and loot rules unchanged.
//! Scene selection commands enter ordinary interactions; socket E2E covers OPNPC/OPOBJ packets.
use super::scenario_tests::ui;
use super::scenario_woodcutting::{all_components, if_opensub};
use super::session_replay::{arrivals, client_frames, mask_wall_clock, Replay, Trace};
use rs910_symbols::{component, interface, inv, npc, obj, seq, varbit};

const FIXTURE: &str = "fixtures/session-replay/king-black-dragon";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const STATIC_COMPONENT_CHILD: i32 = -1;
const PURCHASE_COINS: i32 = 50_000;
const MAXIMUM_LIFE_POINTS: i32 = 9900;
const SELECTION_LABEL: &str = "1";

fn inventory(replay: &mut Replay, id: i32) -> Vec<(i32, i32)> {
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
                .collect()
        })
        .unwrap_or_default()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_king_black_dragon_setup_fight_kill_and_loot() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut seen = false;
    let mut dead = false;
    let mut gone = false;
    let mut projectile = false;
    let mut ground_loot = false;
    let mut cached_selection = false;
    let mut entered = false;
    let mut attack = false;
    let mut take = false;
    let mut minimum_life = MAXIMUM_LIFE_POINTS;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
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
            "cycle {cycle}: gameplay packets"
        );
        for (opcode, payload) in client_frames(&out.written)? {
            if opcode == crate::proto::client::CLIENT_CHEAT {
                let command = String::from_utf8_lossy(&payload);
                attack |= command.contains("opnpc ");
                take |= command.contains("opobj ");
                assert!(
                    !command.contains("hit ") && !command.contains("invset "),
                    "recorded combat/loot must use ordinary rules"
                );
            }
        }
        let game = replay.game();
        entered |= game.runtime.installed_region.is_some();
        let state = &game.runtime.feed.state;
        for enemy in state
            .npcs
            .entities
            .values()
            .filter(|enemy| enemy.type_id == npc::KING_BLACK_DRAGON.id())
        {
            seen = true;
            dead |= enemy.path.animation.main.id() == seq::KBD_COLLAPSE.id();
        }
        gone |= dead
            && !state
                .npcs
                .entities
                .values()
                .any(|enemy| enemy.type_id == npc::KING_BLACK_DRAGON.id());
        projectile |= !state.zones.transients.projectiles.is_empty();
        ground_loot |= state.zones.objects.stacks.values().any(|stack| {
            stack
                .iter()
                .any(|item| item.id == obj::KING_DRAGON_BONES.id())
        });
        if seen {
            minimum_life = minimum_life.min(
                game.varbit_value(varbit::CURRENT_LIFE_POINTS.id() as u16)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?,
            );
        }
        cached_selection |= all_components(ui(&mut replay)).iter().any(|component| {
            let component = component.borrow();
            component.f.parentlayer == component::encounter_setup::LIMIT_VALUE.packed()
                && component.f.id == STATIC_COMPONENT_CHILD
                && component
                    .f
                    .text
                    .as_deref()
                    .map(String::from_utf16_lossy)
                    .is_some_and(|text| text == SELECTION_LABEL)
        });
    }
    assert_eq!(replay_pings, recorded_pings, "ICMP bytes in stream order");
    assert!(
        entered && seen && dead && gone,
        "region={entered}, boss={seen}, death={dead}, removal={gone}"
    );
    assert!(
        projectile && ground_loot && attack && take,
        "projectile={projectile}, drop={ground_loot}, attack={attack}, take={take}"
    );
    assert!(
        cached_selection,
        "cached CS2 displays the authoritative solo selection"
    );
    assert!(
        minimum_life > ZERO,
        "recorded player survived without a fixture heal"
    );
    let packets = arrivals(&trace)?;
    assert!(packets
        .iter()
        .any(|packet| packet.opcode == crate::proto::server::IF_OPENSUB
            && if_opensub(&packet.payload)[1] == interface::ENCOUNTER_SETUP.id()));
    let bag = inventory(&mut replay, inv::BACKPACK.id());
    assert!(
        bag.iter()
            .any(|(id, count)| *id == obj::KING_DRAGON_BONES.id() && *count > ZERO),
        "normal ground loot reached the backpack: {bag:?}"
    );
    assert_eq!(
        bag.iter()
            .filter(|(id, _)| *id == obj::COINS.id())
            .map(|(_, count)| count)
            .sum::<i32>(),
        PURCHASE_COINS
    );
    assert!(
        inventory(&mut replay, inv::WORN_EQUIPMENT.id())
            .iter()
            .any(|(id, _)| *id == obj::DRAGONFIRE_GUARD_SHIELD.id()),
        "real shield equipment transaction"
    );
    Ok(())
}
