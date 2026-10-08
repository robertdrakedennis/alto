//! A real Legacy session: native Chicken Attack and ground Take, a second
//! ordinary Goblin interaction, corpse-time rewards, and client actor/loot state.
//! The recorded renderer supplies the two body/model picks; headless replay
//! omits those picks and compares every other gameplay packet in their cycles.
use super::scenario_tests::ui;
use super::scenario_woodcutting::{cs2_ints, update_stat};
use super::session_replay::{arrivals, client_frames, mask_wall_clock, Replay, Trace};
use anyhow::Context;
use rs910_symbols::{inv, location, npc, obj, seq};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const FIXTURE: &str = "fixtures/session-replay/npc-audit";
const FIRST_CYCLE: i32 = 1;
const ZERO: i32 = 0;
const FIRST_SLOT: usize = 0;
const REQUIRED_FIGHTS: usize = 2;
const REQUIRED_NATIVE_ATTACKS: usize = 1;
const REQUIRED_NATIVE_TAKES: usize = 1;
const COMBAT_SKILL_COUNT: usize = 7;
const HIT_DAMAGE: usize = 1;
const HEALTH_BAR_END: usize = 2;
const EXPECTED_LAUNCH_DAMAGE: i32 = 226;
const CORPSE_TICKS: i64 = 3;
const INITIAL_XP: [i32; COMBAT_SKILL_COUNT] = [40000, 40000, 40000, 1154, 0, 0, 0];
const CHICKEN_REWARD_XP: [i32; COMBAT_SKILL_COUNT] = [40024, 40000, 40000, 1162, 0, 0, 0];
const FINAL_XP: [i32; COMBAT_SKILL_COUNT] = [40049, 40000, 40000, 1170, 0, 0, 0];
const SINGLE_DROP_COUNT: i32 = 1;
const FEATHER_COUNT: i32 = 10;
const EARTH_RUNE_COUNT: i32 = 4;
const CHICKEN_MAX_LIFE: i64 = 50;
const GOBLIN_MAX_LIFE: i64 = 100;
const BYTE_BITS: u32 = 8;
const BYTE_MASK: i32 = 0xff;
const ALT_BYTE_BIAS: i32 = 128;
const NPC_CTRL_OFFSET: usize = 0;
const NPC_HIGH_OFFSET: usize = 1;
const NPC_LOW_OFFSET: usize = 2;
const NPC_PAYLOAD_BYTES: usize = 3;
const OBJ_ID_OFFSET: usize = 0;
const OBJ_X_OFFSET: usize = 2;
const OBJ_Z_OFFSET: usize = 4;
const OBJ_FLAGS_OFFSET: usize = 6;
const OBJ_PAYLOAD_BYTES: usize = 7;
const NEXT_BYTE: usize = 1;
const TILE_BITS: u32 = 14;
const TILE_MASK: i32 = 0x3fff;
const LEVEL_MASK: i32 = 3;
const LEVEL_SHIFT: u32 = 28;
const TILE_COMPONENT_COUNT: usize = 3;
const TILE_X_INDEX: usize = 0;
const TILE_Z_INDEX: usize = 1;
const EAST_TILE: i32 = 1;
const WEST_TILE: i32 = -1;

#[derive(Debug)]
struct Fight {
    id: usize,
    definition: i32,
    sequence: i32,
    tile: [i32; TILE_COMPONENT_COUNT],
    drops: Vec<(i32, i32)>,
    dying: Option<i32>,
    gone: Option<i32>,
    dropped: Option<i32>,
    hit: bool,
    headbar: bool,
}

fn int(row: &Value, field: &str) -> anyhow::Result<i64> {
    row[field]
        .as_i64()
        .with_context(|| format!("{field}: {row}"))
}

fn enemy(row: &Value, id: usize) -> anyhow::Result<&Value> {
    row["enemies"]
        .as_array()
        .context("enemy observations")?
        .iter()
        .find(|enemy| enemy["id"].as_u64() == Some(id as u64))
        .context("observed enemy")
}

fn pairs(rows: &Value, key: &str) -> anyhow::Result<Vec<(i32, i32)>> {
    let mut pairs = rows
        .as_array()
        .context("drop rows")?
        .iter()
        .map(|row| Ok((int(row, key)? as i32, int(row, "count")? as i32)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    pairs.sort_unstable();
    Ok(pairs)
}

fn server_fights(root: &std::path::Path) -> anyhow::Result<Vec<Fight>> {
    let rows: Vec<Value> = std::fs::read_to_string(root.join("combat-receipts.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let deaths: Vec<_> = rows.iter().filter(|row| row["kind"] == "death").collect();
    let launches: Vec<_> = rows
        .iter()
        .filter(|row| row["kind"] == "launch" && row["source"] == "player")
        .collect();
    assert_eq!(deaths.len(), REQUIRED_FIGHTS);
    assert_eq!(launches.len(), REQUIRED_FIGHTS);
    let mut fights = Vec::new();
    for (index, death) in deaths.into_iter().enumerate() {
        let id = usize::try_from(int(death, "targetId")?)?;
        let definition = int(death, "targetDefinition")? as i32;
        let tick = int(death, "tick")?;
        let hide = int(death, "hideTick")?;
        let launch = launches
            .iter()
            .find(|row| row["targetId"] == death["targetId"])
            .context("launch")?;
        assert_eq!(launch["damage"], EXPECTED_LAUNCH_DAMAGE);
        assert_eq!(launch["delay"], ZERO);
        assert_eq!(launch["style"], "melee");
        assert_eq!(hide - tick, CORPSE_TICKS);
        let alive = rows
            .iter()
            .rev()
            .find(|row| {
                row["kind"] == "state"
                    && int(row, "tick").is_ok_and(|at| at < tick)
                    && enemy(row, id).is_ok_and(|target| target["visible"] == true)
            })
            .context("living target")?;
        let dying = rows
            .iter()
            .find(|row| row["kind"] == "state" && row["phase"] == "players" && row["tick"] == tick)
            .context("impact state")?;
        let gone = rows
            .iter()
            .find(|row| row["kind"] == "state" && row["phase"] == "npcs" && row["tick"] == hide)
            .context("corpse removal state")?;
        let corpse = enemy(dying, id)?;
        let removed = enemy(gone, id)?;
        assert_eq!(corpse["visible"], true);
        assert_eq!(corpse["hitpoints"], ZERO);
        assert_eq!(corpse["configuredDeathTicks"], CORPSE_TICKS);
        assert_eq!(removed["visible"], false);
        assert!(removed["loot"].is_null());
        let (sequence, offset, maximum, mut drops) = if definition == npc::CHICKEN.id() {
            (
                seq::CHICKEN_GROUND_DEATH.id(),
                EAST_TILE,
                CHICKEN_MAX_LIFE,
                vec![
                    (obj::BONES.id(), SINGLE_DROP_COUNT),
                    (obj::RAW_CHICKEN.id(), SINGLE_DROP_COUNT),
                    (obj::FEATHER.id(), FEATHER_COUNT),
                ],
            )
        } else {
            assert_eq!(definition, npc::GOBLIN_LEVEL_2.id());
            (
                seq::GOBLIN_DEATH.id(),
                WEST_TILE,
                GOBLIN_MAX_LIFE,
                vec![
                    (obj::BONES.id(), SINGLE_DROP_COUNT),
                    (obj::EARTH_RUNE.id(), EARTH_RUNE_COUNT),
                ],
            )
        };
        assert_eq!(enemy(alive, id)?["hitpoints"], maximum);
        assert!(i64::from(EXPECTED_LAUNCH_DAMAGE) > maximum);
        assert!(corpse["animation"]["modes"]
            .as_array()
            .context("death modes")?
            .iter()
            .all(|mode| mode == sequence));
        let tile = [
            location::NPC_AUDIT_FARM.x() + offset,
            location::NPC_AUDIT_FARM.z(),
            location::NPC_AUDIT_FARM.level(),
        ];
        assert_eq!(
            [int(corpse, "x")? as i32, int(corpse, "z")? as i32],
            [tile[TILE_X_INDEX], tile[TILE_Z_INDEX]]
        );
        drops.sort_unstable();
        assert_eq!(pairs(&corpse["loot"]["drops"], "item")?, drops);
        let ground = |row: &Value| -> anyhow::Result<Vec<(i32, i32)>> {
            let on_tile = row["ground"]
                .as_array()
                .context("ground")?
                .iter()
                .filter(|object| {
                    object["x"] == tile[TILE_X_INDEX] && object["z"] == tile[TILE_Z_INDEX]
                })
                .cloned()
                .collect::<Vec<_>>();
            pairs(&Value::Array(on_tile), "id")
        };
        assert!(
            ground(dying)?.is_empty(),
            "loot appeared before body removal"
        );
        assert_eq!(ground(gone)?, drops);
        let before = [INITIAL_XP, CHICKEN_REWARD_XP][index];
        let after = [CHICKEN_REWARD_XP, FINAL_XP][index];
        assert_eq!(
            dying["players"][FIRST_SLOT]["xp"],
            serde_json::json!(before)
        );
        assert_eq!(gone["players"][FIRST_SLOT]["xp"], serde_json::json!(after));
        fights.push(Fight {
            id,
            definition,
            sequence,
            tile,
            drops,
            dying: None,
            gone: None,
            dropped: None,
            hit: false,
            headbar: false,
        });
    }
    Ok(fights)
}

fn stack(
    game: &crate::client_game::ClientGame,
    tile: [i32; TILE_COMPONENT_COUNT],
) -> Vec<(i32, i32)> {
    let [x, z, level] = tile;
    let key = i64::from(level & LEVEL_MASK) << LEVEL_SHIFT
        | i64::from(z & TILE_MASK) << TILE_BITS
        | i64::from(x & TILE_MASK);
    let mut objects = game
        .runtime
        .feed
        .state
        .zones
        .objects
        .stacks
        .get(&key)
        .map(|rows| {
            rows.iter()
                .map(|row| (row.id, row.count))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    objects.sort_unstable();
    objects
}

fn attack(payload: &[u8]) -> anyhow::Result<(i32, usize)> {
    anyhow::ensure!(payload.len() == NPC_PAYLOAD_BYTES, "OPNPC2 length");
    Ok((
        (ALT_BYTE_BIAS - i32::from(payload[NPC_CTRL_OFFSET])) & BYTE_MASK,
        usize::from(payload[NPC_HIGH_OFFSET]) << BYTE_BITS
            | usize::from(payload[NPC_LOW_OFFSET].wrapping_sub(ALT_BYTE_BIAS as u8)),
    ))
}

fn take(payload: &[u8]) -> anyhow::Result<(i32, i32, i32, i32)> {
    anyhow::ensure!(payload.len() == OBJ_PAYLOAD_BYTES, "OPOBJ3 length");
    let little =
        |at: usize| i32::from(payload[at]) | i32::from(payload[at + NEXT_BYTE]) << BYTE_BITS;
    Ok((
        little(OBJ_ID_OFFSET),
        little(OBJ_X_OFFSET),
        i32::from(payload[OBJ_Z_OFFSET]) << BYTE_BITS
            | i32::from(payload[OBJ_Z_OFFSET + NEXT_BYTE]),
        ALT_BYTE_BIAS - i32::from(payload[OBJ_FLAGS_OFFSET]),
    ))
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_npc_audit_death_loot_pickup_and_corpse_rewards() -> anyhow::Result<()> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let mut fights = server_fights(&root)?;
    let trace = Trace::load(&root.join("session.rtr"))?;
    let packets = arrivals(&trace)?;
    let mut replay = Replay::start(&trace)?;
    let mut attacks = Vec::new();
    let mut takes = Vec::new();
    let mut processed = FIRST_SLOT;
    let mut previous_xp = INITIAL_XP;
    let mut rewards = Vec::new();
    let mut removed_at = BTreeMap::new();
    let mut last_stacks = BTreeMap::new();
    let mut hits = BTreeSet::new();
    for cycle in FIRST_CYCLE..=trace.last_cycle() {
        let output = replay.cycle(&trace, cycle)?;
        let recorded = trace.bytes(b"OUT ", cycle);
        let frames = client_frames(&recorded)?;
        let picked = frames.iter().any(|(opcode, _)| {
            [crate::proto::client::OPNPC2, crate::proto::client::OPOBJ3].contains(opcode)
        });
        for (opcode, payload) in &frames {
            if *opcode == crate::proto::client::OPNPC2 {
                attacks.push(attack(payload)?);
            }
            if *opcode == crate::proto::client::OPOBJ3 {
                takes.push(take(payload)?);
            }
            if *opcode == crate::proto::client::CLIENT_CHEAT {
                let command = String::from_utf8_lossy(payload);
                assert!(!["hit ", "setxp ", "invset "]
                    .iter()
                    .any(|bypass| command.contains(bypass)));
            }
        }
        let filter_pick = |rows: Vec<(u8, Vec<u8>)>| {
            rows.into_iter()
                .filter(|(opcode, _)| {
                    ![
                        crate::proto::client::OPNPC2,
                        crate::proto::client::OPOBJ3,
                        crate::proto::client::MOVE_GAMECLICK,
                    ]
                    .contains(opcode)
                })
                .collect::<Vec<_>>()
        };
        let actual = mask_wall_clock(&output.written)?;
        let expected = mask_wall_clock(&recorded)?;
        if picked {
            assert_eq!(
                filter_pick(actual),
                filter_pick(expected),
                "cycle {cycle}: packets beside renderer pick"
            );
        } else {
            assert_eq!(actual, expected, "cycle {cycle}: client packets");
        }
        let game = replay.game();
        for fight in &mut fights {
            if let Some(target) = game.runtime.feed.state.npcs.entities.get(&fight.id) {
                assert_eq!(target.type_id, fight.definition);
                if let Some(combat) = &target.path.combat {
                    for hit in &combat.hits {
                        hits.insert(hit[HIT_DAMAGE]);
                        fight.hit |= hit[HIT_DAMAGE] == EXPECTED_LAUNCH_DAMAGE;
                    }
                    fight.headbar |= combat.bars.iter().any(|bar| {
                        bar.updates
                            .iter()
                            .any(|update| update[HEALTH_BAR_END] == ZERO)
                    });
                }
                if target.path.animation.main.id() == fight.sequence && fight.dying.is_none() {
                    fight.dying = Some(cycle);
                }
            } else if fight.dying.is_some() && fight.gone.is_none() {
                fight.gone = Some(cycle);
                removed_at.insert(fight.id, cycle);
            }
            let objects = stack(game, fight.tile);
            if objects == fight.drops && fight.dropped.is_none() {
                fight.dropped = Some(cycle);
            }
            last_stacks.insert(fight.id, objects);
        }
        let done = replay.processed(&packets);
        let fresh = &packets[processed..done];
        processed = done;
        if fresh
            .iter()
            .any(|packet| packet.opcode == crate::proto::server::UPDATE_STAT)
        {
            let mut expected = previous_xp;
            for packet in fresh
                .iter()
                .filter(|packet| packet.opcode == crate::proto::server::UPDATE_STAT)
            {
                let [stat, xp, _] = update_stat(&packet.payload);
                if let Some(value) = expected.get_mut(stat as usize) {
                    *value = xp;
                }
            }
            if expected != previous_xp {
                assert!(
                    removed_at.values().any(|at| *at == cycle),
                    "cycle {cycle}: XP before body removal"
                );
                assert_eq!(expected, [CHICKEN_REWARD_XP, FINAL_XP][rewards.len()]);
                rewards.push(cycle);
            }
            let commands: Vec<_> = (ZERO..COMBAT_SKILL_COUNT as i32)
                .flat_map(|stat| [("push_constant_int", stat), ("stat_visible_xp", ZERO)])
                .collect();
            assert_eq!(
                cs2_ints(&mut replay, &commands)?,
                expected,
                "cycle {cycle}: native skill consumer"
            );
            previous_xp = expected;
        }
    }
    let chicken = fights
        .iter()
        .find(|fight| fight.definition == npc::CHICKEN.id())
        .context("Chicken")?;
    assert_eq!(attacks.len(), REQUIRED_NATIVE_ATTACKS);
    assert_eq!(attacks, [(ZERO, chicken.id)]);
    assert_eq!(takes.len(), REQUIRED_NATIVE_TAKES);
    assert_eq!(
        takes,
        [(
            obj::FEATHER.id(),
            chicken.tile[TILE_X_INDEX],
            chicken.tile[TILE_Z_INDEX],
            ZERO
        )]
    );
    assert_eq!(rewards.len(), REQUIRED_FIGHTS);
    assert_eq!(previous_xp, FINAL_XP);
    assert!(hits.contains(&EXPECTED_LAUNCH_DAMAGE));
    for fight in &fights {
        assert!(fight.hit && fight.headbar, "hit/headbar absent: {fight:?}");
        let dying = fight.dying.context("death sequence consumer")?;
        let gone = fight.gone.context("body removal")?;
        assert!(gone > dying, "corpse lifetime: {fight:?}");
        assert_eq!(fight.dropped, Some(gone), "ground loot at body removal");
    }
    assert_eq!(
        last_stacks[&chicken.id],
        [
            (obj::BONES.id(), SINGLE_DROP_COUNT),
            (obj::RAW_CHICKEN.id(), SINGLE_DROP_COUNT)
        ]
    );
    let backpack = ui(&mut replay)
        .engine
        .inv_cache
        .inventory(inv::BACKPACK.id(), false)
        .context("backpack consumer")?;
    assert!(backpack
        .obj_ids
        .iter()
        .copied()
        .zip(backpack.counts.iter().copied())
        .any(|entry| entry == (obj::FEATHER.id(), FEATHER_COUNT)));
    Ok(())
}
