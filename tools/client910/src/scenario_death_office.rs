//! One ordinary client recording: fatal hit, private Office rebuild, cached
//! reclaim UI operations, sacrifice/claim and the real bridged hourglass exit.
//! Developer commands only set up damage and approach; socket E2E covers loc/NPC input.
use super::scenario_tests::ui;
use super::scenario_woodcutting::{if_opensub, message_game};
use super::session_replay::{arrivals, mask_wall_clock, observe, Replay, Trace};
use rs910_symbols::{interface, inv, location, npc, obj, varbit};

const FIXTURE: &str = "fixtures/session-replay/death-office";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const CHUNK_TILES: i32 = 8;
const GROUND_LEVEL: usize = 0;
const BRIDGE_LEVEL: usize = 1;
const TEMPLATE_SOURCE_X_SHIFT: i32 = 14;
const TEMPLATE_SOURCE_Z_SHIFT: i32 = 3;
const TEMPLATE_SOURCE_X_MASK: i32 = (1 << 10) - 1;
const TEMPLATE_SOURCE_Z_MASK: i32 = (1 << 11) - 1;
const KEPT_SWORDS: i32 = 3;
const RECLAIMED_SWORDS: i32 = 4;
const INITIAL_COINS: i32 = 10000;

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
fn recorded_death_office_rebuild_reclaim_and_bridged_hourglass_exit() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut died = false;
    let mut office = false;
    let mut bridge_copied = false;
    let mut reaper = false;
    let mut selected = false;
    let mut item_widgets = false;
    let mut minimum_swords = i32::MAX;
    let mut recorded_pings = Vec::new();
    let mut replay_pings = Vec::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
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
            gameplay(&output.written, &mut replay_pings)?,
            gameplay(&trace.bytes(b"OUT ", cycle), &mut recorded_pings)?,
            "cycle {cycle}: gameplay packets"
        );
        let game = replay.game();
        died |= game
            .varbit_value(varbit::CURRENT_LIFE_POINTS.id() as u16)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            == ZERO;
        reaper |= game
            .runtime
            .feed
            .state
            .npcs
            .entities
            .values()
            .any(|entity| entity.type_id == npc::DEATH_OFFICE_REAPER.id());
        selected |= game
            .varbit_value(varbit::DEATH_RECLAIM_SAVED_COUNT.id() as u16)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            == RECLAIMED_SWORDS;
        if let Some(layout) = game.runtime.installed_region.as_ref() {
            office = true;
            let local = observe(game)
                .local
                .ok_or_else(|| anyhow::anyhow!("Office local player missing"))?;
            let cx = usize::try_from((local.tile[0] - game.runtime.map.base_x) / CHUNK_TILES);
            let cz = usize::try_from((local.tile[1] - game.runtime.map.base_z) / CHUNK_TILES);
            let (Ok(cx), Ok(cz)) = (cx, cz) else {
                continue;
            };
            if cx >= layout.chunks_x || cz >= layout.chunks_z {
                continue;
            }
            let at =
                |level| layout.templates[(level * layout.chunks_x + cx) * layout.chunks_z + cz];
            let ground = at(GROUND_LEVEL);
            let bridge = at(BRIDGE_LEVEL);
            if ground >= ZERO && bridge >= ZERO {
                let source = location::DEATH_OFFICE_ARRIVAL;
                bridge_copied |= (ground >> TEMPLATE_SOURCE_X_SHIFT) & TEMPLATE_SOURCE_X_MASK
                    == source.x() / CHUNK_TILES
                    && (ground >> TEMPLATE_SOURCE_Z_SHIFT) & TEMPLATE_SOURCE_Z_MASK
                        == source.z() / CHUNK_TILES
                    && (bridge >> TEMPLATE_SOURCE_X_SHIFT) & TEMPLATE_SOURCE_X_MASK
                        == source.x() / CHUNK_TILES;
            }
        }
        item_widgets |= super::scenario_woodcutting::all_components(ui(&mut replay))
            .iter()
            .any(|component| {
                component.borrow().f.invobject == obj::SMITHING_RUNE_SWORD.id()
                    && (component.borrow().f.parentlayer as u32 >> 16) as i32
                        == interface::DEATH_RECLAIM.id()
            });
        let swords: i32 = inventory(&mut replay, inv::BACKPACK.id())
            .iter()
            .filter(|(id, _)| *id == obj::SMITHING_RUNE_SWORD.id())
            .map(|(_, count)| count)
            .sum();
        if swords > ZERO {
            minimum_swords = minimum_swords.min(swords);
        }
    }
    assert_eq!(replay_pings, recorded_pings, "ICMP bytes in stream order");
    assert!(died && office && bridge_copied && reaper && selected, "death={died}, Office={office}, bridge={bridge_copied}, Reaper={reaper}, selected={selected}");
    assert!(item_widgets, "cached scripts created reclaim item widgets");
    assert_eq!(minimum_swords, KEPT_SWORDS, "three swords survived death");
    let packets = arrivals(&trace)?;
    assert!(
        packets
            .iter()
            .any(|packet| packet.opcode == crate::proto::server::IF_OPENSUB
                && if_opensub(&packet.payload)[1] == interface::DEATH_RECLAIM.id()),
        "cached reclaim interface opened"
    );
    assert!(packets
        .iter()
        .any(|packet| packet.opcode == crate::proto::server::MESSAGE_GAME
            && message_game(&packet.payload).1 == "Oh dear, you are dead!"));
    let bag = inventory(&mut replay, inv::BACKPACK.id());
    assert_eq!(
        bag.iter()
            .filter(|(id, _)| *id == obj::SMITHING_RUNE_SWORD.id())
            .map(|(_, count)| count)
            .sum::<i32>(),
        RECLAIMED_SWORDS,
        "one reclaimed and one sacrificed"
    );
    assert_eq!(
        bag.iter()
            .filter(|(id, _)| *id == obj::COINS.id())
            .map(|(_, count)| count)
            .sum::<i32>(),
        INITIAL_COINS,
        "sacrifice credit covered the fee"
    );
    assert!(
        replay.game().runtime.installed_region.is_none(),
        "hourglass returned to the ordinary map"
    );
    let local = observe(replay.game())
        .local
        .ok_or_else(|| anyhow::anyhow!("respawn local player missing"))?;
    let respawn = location::STANDARD_LUMBRIDGE_TELEPORT;
    assert_eq!(
        local.tile,
        [respawn.x(), respawn.z(), respawn.level()],
        "hourglass exit at respawn"
    );
    Ok(())
}
