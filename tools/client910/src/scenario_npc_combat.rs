//! NPC population and melee, client side: a recorded dev-server session in
//! which the real client stands among the Lumbridge farm's wandering chickens,
//! attacks the one beside it with a left click, kills it, sees its death
//! sequence and the drop appear on the ground, and clicks the drop to take it,
//! replayed through the production owners (`session_replay`).
//!
//! Fixture: `fixtures/session-replay/npc-combat/` (`record.sh`; the release
//! client with `CLIENT910_RECORD` against the dev lobby/world and its generated
//! NPC population, one developer command per 100 cycles, left clicks at the
//! cycles below).
use super::scenario_woodcutting::{update_inv_partial, update_stat};
use super::session_replay::{arrivals, client_frames, mask_wall_clock, Replay, Trace};
use super::*;
use rs910_symbols::{inv, obj, seq};

const FIXTURE: &str = "fixtures/session-replay/npc-combat";
/// The NPC `npcadd 41 1 0` put beside the player: the slot after the 11,134
/// generated spawns and the fishing spots.
const CHICKEN_SLOT: usize = 11138;
/// Where it stands: east of the player's tile (3231, 3293), on the crate loc that
/// fills the tile.
const CHICKEN_TILE: [i32; 3] = [3232, 3293, 0];
/// Chicken (NPC type 41) death sequence the generated profile names, and the
/// ticks (30 cycles each) the server lets it play before the body goes.
const DEATH_SEQ: i32 = seq::CHICKEN_DEATH.id();
const DEATH_TICKS: i32 = 6;
const TICK_CYCLES: i32 = 30;
/// The chicken's drop table (wiki): bones and a raw chicken always, then one of the
/// main drops (feathers most often).
const BONES: i32 = obj::BONES.id();
const RAW_CHICKEN: i32 = obj::RAW_CHICKEN.id();
/// `record.sh`: the cycles of the left click on the chicken (Attack) and of the click on its drop (Take).
const ATTACK_CYCLE: i32 = 701;
const TAKE_CYCLE: i32 = 1701;
/// `record.sh`: `setxp 0 40000` and `setxp 2 40000`; Attack, Strength and Constitution (stat 3, level 10: 1154 XP).
const SET_XP: i32 = 40000;
/// The player's backpack inventory.
const BACKPACK_INV: i32 = inv::BACKPACK.id();

const CONSTITUTION_XP: i32 = 1154;

/// `OPNPC2` (Attack): `p1_alt3(ctrl)`, `p2_alt2(index)`.
fn opnpc2_index(payload: &[u8]) -> (i32, usize) {
    let ctrl = (128 - i32::from(payload[0])) & 0xFF;
    let index = usize::from(payload[1]) << 8 | usize::from(payload[2].wrapping_sub(128));
    (ctrl, index)
}

/// `OPOBJ3` (Take): obj and x little-endian, z big-endian, then `128 - flags` (1 ctrl, 2 from a submenu):
/// `(obj, x, z, flags)`.
fn opobj3(payload: &[u8]) -> (i32, i32, i32, i32) {
    let little = |at: usize| i32::from(payload[at]) | i32::from(payload[at + 1]) << 8;
    let z = i32::from(payload[4]) << 8 | i32::from(payload[5]);

    (little(0), little(2), z, 128 - i32::from(payload[6]))
}

/// The ground stack the client holds on a tile: `(obj, count)` by rank.
fn stack_at(game: &crate::client_game::ClientGame, tile: [i32; 3]) -> Vec<(i32, i32)> {
    let key = i64::from(tile[2] & 3) << 28
        | i64::from(tile[1] & 0x3fff) << 14
        | i64::from(tile[0] & 0x3fff);
    game.runtime
        .feed
        .state
        .zones
        .objects
        .stacks
        .get(&key)
        .map(|stack| stack.iter().map(|o| (o.id, o.count)).collect())
        .unwrap_or_default()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_npc_session_populates_wanders_and_fights() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut most_npcs = 0;
    let mut most_chickens = 0;
    let mut tiles =
        std::collections::BTreeMap::<usize, std::collections::BTreeSet<(i32, i32)>>::new();
    let mut hitmarks = Vec::new();
    let mut bars = 0;
    let mut sequences = std::collections::BTreeSet::new();
    let mut attacks = Vec::new();
    let mut takes = Vec::new();
    // The cycle the chicken was first in view, its death sequence began, its body left, and its drop appeared.
    let (mut seen, mut dying, mut gone, mut dropped) = (0, 0, 0, 0);
    let mut drop = Vec::new();
    let (mut taken_at, mut last_stack) = (0, Vec::new());
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        let recorded = trace.bytes(b"OUT ", cycle);
        if cycle == ATTACK_CYCLE || cycle == TAKE_CYCLE {
            // The click picks the NPC and the ground item from the renderer's drawn bodies and models, which the
            // headless replay does not draw: the replayed client walks to the tile instead. The recorded frame is
            // the Attack or the Take.
            for (opcode, payload) in client_frames(&recorded)? {
                if cycle == ATTACK_CYCLE {
                    assert_eq!(
                        opcode,
                        crate::proto::client::OPNPC2,
                        "the first click's frame"
                    );
                    attacks.push(opnpc2_index(&payload));
                } else {
                    assert_eq!(
                        opcode,
                        crate::proto::client::OPOBJ3,
                        "the second click's frame"
                    );
                    takes.push(opobj3(&payload));
                }
            }
        } else {
            assert_eq!(
                mask_wall_clock(&out.written)?,
                mask_wall_clock(&recorded)?,
                "cycle {cycle}: client packets differ from the recording"
            );
        }
        let game = replay.game();
        let npcs = &game.runtime.feed.state.npcs;
        most_npcs = most_npcs.max(npcs.entities.len());
        most_chickens = most_chickens.max(
            npcs.entities
                .values()
                .filter(|n| n.name == "Chicken")
                .count(),
        );
        for (&index, npc) in &npcs.entities {
            tiles
                .entry(index)
                .or_default()
                .insert((npc.path.x[0], npc.path.z[0]));
            if index != CHICKEN_SLOT {
                continue;
            }
            if seen == 0 {
                seen = cycle;
            }
            if let Some(combat) = &npc.path.combat {
                for hit in combat.hits.iter().filter(|h| h[4] != 0) {
                    hitmarks.push((hit[0], hit[1]));
                }
                bars = bars.max(
                    combat
                        .bars
                        .iter()
                        .map(|b| b.updates.len())
                        .max()
                        .unwrap_or(0),
                );
            }
            let sequence = npc.path.animation.main.id();
            if sequence >= 0 {
                sequences.insert(sequence);
            }
            if sequence == DEATH_SEQ && dying == 0 {
                dying = cycle;
            }
        }
        if seen != 0 && gone == 0 && !npcs.entities.contains_key(&CHICKEN_SLOT) {
            gone = cycle;
        }
        let stack = stack_at(game, CHICKEN_TILE);
        if dropped == 0 && !stack.is_empty() {
            dropped = cycle;
            drop = stack.clone();
        }
        if dropped != 0 && taken_at == 0 && stack.len() < drop.len() {
            taken_at = cycle;
        }
        last_stack = stack;
    }
    // A populated area: the farm's chickens and the people around the spawn are in view.
    assert!(most_npcs >= 15, "{most_npcs} NPCs in view at most");
    assert!(
        most_chickens >= 5,
        "{most_chickens} chickens in view at most"
    );
    // Wanderers move between tiles (the dev chicken, which stands still, does not).
    let wanderers = tiles
        .iter()
        .filter(|(&index, t)| index != CHICKEN_SLOT && t.len() >= 3)
        .count();
    assert!(wanderers >= 3, "{wanderers} NPCs walked about");
    assert_eq!(
        tiles[&CHICKEN_SLOT].len(),
        1,
        "the dev chicken stands still"
    );
    // The first click was an Attack on the dev chicken's slot, without ctrl.
    assert_eq!(attacks, [(0, CHICKEN_SLOT)]);
    // The fight: a hitmark and a headbar on the chicken.
    assert!(!hitmarks.is_empty(), "no hitmark on the chicken");
    assert!(bars >= 1, "no headbar on the chicken");
    // The kill: it was in view before the click, plays its death sequence from the click on, and its body
    // goes once the sequence has run its ticks.
    assert!(seen > 0 && seen < ATTACK_CYCLE, "in view at {seen}");
    assert!(sequences.contains(&DEATH_SEQ), "{sequences:?}");
    assert!(
        (ATTACK_CYCLE..ATTACK_CYCLE + 2 * TICK_CYCLES).contains(&dying),
        "death sequence from cycle {dying}"
    );
    assert!(
        gone - dying >= (DEATH_TICKS - 1) * TICK_CYCLES
            && gone - dying <= (DEATH_TICKS + 1) * TICK_CYCLES,
        "died at {dying}, body gone at {gone}"
    );
    // The drop: the table's two sure items, and perhaps a third, on the tile it died on as the body goes.
    assert!(
        (dropped - gone).abs() <= TICK_CYCLES && dropped >= dying,
        "body gone at {gone}, drop at {dropped}"
    );
    let ids: Vec<i32> = drop.iter().map(|&(id, _)| id).collect();
    assert!(
        ids.contains(&BONES) && ids.contains(&RAW_CHICKEN) && (2..=3).contains(&ids.len()),
        "dropped {drop:?}"
    );
    // The experience: Attack takes the kill's share (one hit, one third as much again to Constitution).
    let stats: Vec<[i32; 3]> = arrivals(&trace)?
        .iter()
        .filter(|a| a.opcode == crate::proto::server::UPDATE_STAT)
        .map(|a| update_stat(&a.payload))
        .collect();
    let attack = stats.iter().rev().find(|s| s[0] == 0).context("Attack")?;
    let constitution = stats
        .iter()
        .rev()
        .find(|s| s[0] == 3)
        .context("Constitution")?;
    assert_eq!(
        stats.iter().filter(|s| s[0] == 0 && s[1] > 0).count(),
        2,
        "Attack: set, then the kill"
    );
    assert!(attack[1] > SET_XP, "Attack XP {attack:?}");
    assert!(
        constitution[1] > CONSTITUTION_XP,
        "Constitution XP {constitution:?}"
    );
    let (attack_gain, constitution_gain) = (attack[1] - SET_XP, constitution[1] - CONSTITUTION_XP);
    assert!(
        (3 * constitution_gain - attack_gain).abs() <= 3,
        "Attack +{attack_gain}, Constitution +{constitution_gain}"
    );
    // The click on the drop (the middle of the drawn model, on the crate the chicken stood on) asks for one of its
    // items, on its tile, without ctrl.
    assert_eq!(takes.len(), 1);
    let (obj, x, z, flags) = takes[0];
    assert!(ids.contains(&obj), "take of {obj}, drop {drop:?}");
    assert_eq!([x, z, flags], [CHICKEN_TILE[0], CHICKEN_TILE[1], 0]);
    // Nobody can stand on a crate's tile: the player took it from the side, where they stood all along.
    let positions: std::collections::BTreeSet<(i64, i64)> =
        std::fs::read_to_string(root.join("server-trace.jsonl"))?
            .lines()
            .filter(|line| line.contains("\"player_info\""))
            .map(|line| {
                let at: serde_json::Value = serde_json::from_str(line)?;
                anyhow::Ok((
                    at["x"].as_i64().context("x")?,
                    at["z"].as_i64().context("z")?,
                ))
            })
            .collect::<anyhow::Result<_>>()?;
    assert_eq!(positions, [(3231, 3293)].into(), "the player's tiles");
    // The take: the server removes that item from the stack (OBJ_DEL: obj, then the packed tile), and the client's
    // stack loses it and keeps the rest, within a few ticks of the click.
    let removed: Vec<(i32, i32)> = arrivals(&trace)?
        .iter()
        .filter(|a| a.opcode == crate::proto::server::OBJ_DEL)
        .map(|a| {
            let packed = (128 - i32::from(a.payload[2])) & 0xFF;
            (
                i32::from(a.payload[0]) << 8 | i32::from(a.payload[1]),
                packed,
            )
        })
        .collect();
    let local_tile = (CHICKEN_TILE[0] & 7) << 4 | (CHICKEN_TILE[1] & 7);
    assert_eq!(removed, [(obj, local_tile)], "OBJ_DEL frames");
    assert!(
        taken_at > TAKE_CYCLE && taken_at < TAKE_CYCLE + 10 * TICK_CYCLES,
        "the stack lost the item at {taken_at}"
    );
    let rest: Vec<(i32, i32)> = drop.iter().copied().filter(|&(id, _)| id != obj).collect();
    assert_eq!(last_stack, rest, "the stack after the take");
    // ... and the item lands in the first free backpack slot, with the stack's count.
    let count = drop
        .iter()
        .find(|&&(id, _)| id == obj)
        .map(|&(_, count)| count)
        .context("count")?;
    let backpack: Vec<_> = arrivals(&trace)?
        .iter()
        .filter(|a| a.opcode == crate::proto::server::UPDATE_INV_PARTIAL)
        .map(|a| update_inv_partial(&a.payload))
        .filter(|(inv, _)| *inv == BACKPACK_INV)
        .collect();
    assert_eq!(
        backpack,
        [(BACKPACK_INV, vec![[0, obj, count]])],
        "backpack updates"
    );
    Ok(())
}
