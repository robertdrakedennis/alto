//! The world basics on the real client: dev-server sessions recorded with the release client
//! (`fixtures/session-replay/world-*/record.sh`), replayed through the production owners (`session_replay`).
use super::scenario_tests::{component_rect, hit_point, install_pick_frame, ui};
use super::session_replay::{Replay, Trace};
use super::*;
use rs910_symbols::{component, interface, inv, obj, ComponentId};

/// Where the recorded fixtures live: the one place this file reads a path from.
fn fixtures() -> std::path::PathBuf {
    rs910_core::test_support::client_fixtures().join("session-replay")
}

/// The trace in `dir` (a fixture directory, or an absolute directory holding a raw `session.rtr`/`raw.rtr`) replayed to `cycle`.
fn replay_to(dir: &Path, file: &str, cycle: i32) -> anyhow::Result<Replay> {
    let trace = Trace::load(&dir.join(file))?;
    let mut replay = Replay::start(&trace)?;
    for c in 1..=cycle {
        replay.cycle(&trace, c)?;
    }
    Ok(replay)
}

/// A developer aid, not a check: print where the loc picks of the scene stand on the canvas at `SCOUT_CYCLE` of
/// the recording `SCOUT_TRACE` (a path to a `.rtr`), to find the pixels a click script needs.
/// `SCOUT_TRACE=/path/raw.rtr SCOUT_CYCLE=390 SCOUT_IDS=45476,45481 cargo test --release -p client910 --lib scout_pixels -- --ignored --nocapture`
#[test]
#[ignore = "a developer aid: needs SCOUT_TRACE"]
fn scout_pixels() -> anyhow::Result<()> {
    let trace = std::env::var("SCOUT_TRACE")?;
    let cycle: i32 = std::env::var("SCOUT_CYCLE")?.parse()?;
    let ids: Vec<i32> = std::env::var("SCOUT_IDS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.parse().ok())
        .collect();
    let path = Path::new(&trace);
    let mut replay = replay_to(
        path.parent().unwrap(),
        path.file_name().unwrap().to_str().unwrap(),
        cycle,
    )?;
    ui(&mut replay).paint(cycle, true, [0.; 3])?;
    install_pick_frame(&mut replay)?;
    let frame = ui(&mut replay).engine.scene.player_picks.as_ref().unwrap();
    for pick in &frame.loc_picks {
        let c = pick.capsule;
        println!(
            "loc {} level {} tile {:?} capsule mid ({}, {})",
            pick.id,
            pick.level,
            pick.tile,
            (c.a[0] + c.b[0]) / 2,
            (c.a[1] + c.b[1]) / 2
        );
    }
    for pick in &frame.npc_picks {
        let Some(c) = pick.screen_bounds else {
            continue;
        };
        println!(
            "npc {} capsule mid ({}, {})",
            pick.id.pid,
            (c.a[0] + c.b[0]) / 2,
            (c.a[1] + c.b[1]) / 2
        );
    }
    for id in ids {
        match hit_point(&mut replay, id) {
            Ok(p) => println!("HIT loc {id} at {p:?}"),
            Err(e) => println!("HIT loc {id}: {e}"),
        }
    }
    // `SCOUT_COMPONENTS=interface:component,...`: the canvas rectangle each is drawn at (x, y, width, height).
    for spec in std::env::var("SCOUT_COMPONENTS")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
    {
        let (interface, component) = spec.split_once(':').context("interface:component")?;
        let packed = interface.parse::<i32>()? << 16 | component.parse::<i32>()?;
        let rect = component_rect(ui(&mut replay), &|c| {
            c.f.parentlayer == packed && c.f.id == -1
        });
        println!("COMPONENT {spec}: {rect:?}");
    }
    Ok(())
}

/// Whether interface `id` is mounted in a window slot right now.
fn mounted(replay: &mut Replay, id: i32) -> bool {
    ui(replay)
        .state
        .layout
        .subs
        .iter()
        .any(|&(_, sub)| sub == id)
}

/// The text of static component `component`, when it is drawn.
fn text_of(replay: &mut Replay, component: ComponentId) -> Option<String> {
    let packed = component.packed();
    super::scenario_woodcutting::all_components(ui(replay))
        .into_iter()
        .find(|c| c.borrow().f.parentlayer == packed && c.borrow().f.id == -1)
        .map(|c| String::from_utf16_lossy(c.borrow().f.text.as_deref().unwrap_or(&[])))
}

/// The player's inventory `inv` as `(obj, count)` of its filled slots.
fn slots(replay: &mut Replay, inv: i32) -> Vec<(i32, i32)> {
    ui(replay)
        .engine
        .inv_cache
        .inventory(inv, false)
        .map(|i| {
            i.obj_ids
                .iter()
                .zip(&i.counts)
                .filter(|(&id, _)| id >= 0)
                .map(|(&id, &n)| (id, n))
                .collect()
        })
        .unwrap_or_default()
}

/// `fixtures/session-replay/world-store/record.sh`: the cycles of its clicks and operations.
const CONTINUE_CYCLES: [i32; 2] = [701, 901];
const OPTION_CYCLE: i32 = 801;
const HOME_TELEPORT_CYCLE: i32 = 1801;
const LODESTONE_CYCLE: i32 = 1851;
/// After the Buy click (1201) and before the close click (1301): the shop's stock is on the client.
const BUY_SETTLED_CYCLE: i32 = 1290;

/// The Lumbridge General Store (the whole session of `world-store`), from the real client's own clicks:
///
/// - a conversation (NPC 520, a shopkeeper, started beside the player): the NPC chat interface (1184) in the dialogue slot
///   with the words the wiki's transcript gives, Continue (the pause button the client arms, a `RESUME_PAUSEBUTTON`),
///   the options (1188, drawn by the cache's script 5589 from the options the server sent), the first option, the
///   player's own words (1191), Continue; then the shop the option leads to (1265);
/// - the shop: a click on the first stock row selects it, a click on Buy buys one (coins 1000 -> 999, an empty pot in the
///   backpack, the stock line 30 -> 29), and the close button closes the window;
/// - the lodestone network: the Home Teleport button of the magic book (1465:18) opens the network window (1092), its
///   Lumbridge button casts and puts the player beside the lodestone at (3233, 3222), the window is gone.
///
/// Two clicks at the door and at the stairs landed on the ground and walked (the recording keeps them: their pixels
/// are not reproduced by the replay's scene, so only that they were walks is checked).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_store_session_talks_buys_and_teleports() -> anyhow::Result<()> {
    use super::session_replay::{client_frames, mask_wall_clock};
    use crate::proto::client as cp;
    let trace = Trace::load(&fixtures().join("world-store/session.rtr"))?;
    let mut replay = Replay::start(&trace)?;
    let mut windows: Vec<(i32, Vec<i32>)> = Vec::new();
    let mut sent: Vec<(i32, u8, Vec<u8>)> = Vec::new();
    let mut greeting = None;
    let mut stock = Vec::new();
    for cycle in 1..=trace.last_cycle() {
        let out = replay.cycle(&trace, cycle)?;
        let walks = |frames: Vec<(u8, Vec<u8>)>| -> Vec<(u8, Vec<u8>)> {
            frames
                .into_iter()
                .map(|(op, payload)| {
                    if op == cp::MOVE_GAMECLICK {
                        (op, vec![])
                    } else {
                        (op, payload)
                    }
                })
                .collect()
        };
        assert_eq!(
            walks(mask_wall_clock(&out.written)?),
            walks(mask_wall_clock(&trace.bytes(b"OUT ", cycle))?),
            "cycle {cycle}: client packets differ from the recording"
        );
        for (op, payload) in client_frames(&out.written)? {
            if [cp::RESUME_PAUSEBUTTON, cp::IF_BUTTON1, cp::MOVE_GAMECLICK].contains(&op) {
                sent.push((cycle, op, payload));
            }
        }
        let now: Vec<i32> = [
            interface::NPC_CHAT,
            interface::DIALOGUE_OPTIONS,
            interface::PLAYER_CHAT,
            interface::SHOP,
            interface::LODESTONE_NETWORK,
        ]
        .map(|i| i.id())
        .into_iter()
        .filter(|&i| mounted(&mut replay, i))
        .collect();
        if windows.last().is_none_or(|(_, last)| *last != now) {
            windows.push((cycle, now));
        }
        if cycle == BUY_SETTLED_CYCLE {
            stock = slots(&mut replay, inv::GENERAL_STORE_STOCK.id());
        }

        if greeting.is_none() && mounted(&mut replay, interface::NPC_CHAT.id()) {
            greeting = text_of(&mut replay, component::npc_chat::TEXT);
        }
    }
    // The windows, in order: the shopkeeper speaks, the options, the player speaks, the shop, nothing, the network, nothing.
    assert_eq!(
        windows.iter().map(|(_, w)| w.clone()).collect::<Vec<_>>(),
        [
            vec![],
            vec![interface::NPC_CHAT.id()],
            vec![interface::DIALOGUE_OPTIONS.id()],
            vec![interface::PLAYER_CHAT.id()],
            vec![interface::SHOP.id()],
            vec![],
            vec![interface::LODESTONE_NETWORK.id()],
            vec![]
        ]
    );
    assert_eq!(greeting.as_deref(), Some("Can I help you at all?"));
    // The answers: three pause buttons (Continue on 1184:15, the first option 1188:8, Continue on 1191:15), then the shop.
    let pauses: Vec<(i32, i32)> = sent
        .iter()
        .filter(|(_, op, _)| *op == cp::RESUME_PAUSEBUTTON)
        .map(|(cycle, _, p)| (*cycle, i32::from_be_bytes([p[1], p[0], p[3], p[2]])))
        .collect();
    assert_eq!(
        pauses,
        [
            (
                CONTINUE_CYCLES[0],
                component::npc_chat::CONTINUE_BUTTON.packed()
            ),
            (
                OPTION_CYCLE,
                component::dialogue_options::FIRST_OPTION.packed()
            ),
            (
                CONTINUE_CYCLES[1],
                component::player_chat::CONTINUE_BUTTON.packed()
            )
        ]
    );
    // Buying, the close button, Home Teleport and the lodestone window's Lumbridge button are IF_BUTTON1 on these components.
    let buttons: Vec<(i32, i32)> = sent
        .iter()
        .filter(|(_, op, _)| *op == cp::IF_BUTTON1)
        .map(|(cycle, _, p)| (*cycle, i32::from_be_bytes([p[4], p[5], p[6], p[7]])))
        .collect();
    assert_eq!(
        buttons,
        [
            (1101, component::shop::STOCK_ROWS.packed()),
            (1201, component::shop::CONFIRM_BUTTON.packed()),
            (1301, component::shop::CLOSE_BUTTON.packed()),
            (
                HOME_TELEPORT_CYCLE,
                component::minimap::HOME_TELEPORT_BUTTON.packed()
            ),
            (
                LODESTONE_CYCLE,
                component::lodestone_network::LUMBRIDGE_BUTTON.packed()
            )
        ]
    );
    // The purchase: a coin gone, a pot in the backpack, a pot fewer in the stock.
    assert_eq!(
        slots(&mut replay, inv::BACKPACK.id()),
        [(obj::COINS.id(), 999), (obj::EMPTY_POT.id(), 1)]
    );
    assert_eq!(stock.first(), Some(&(obj::EMPTY_POT.id(), 29)));
    // The teleport: beside the Lumbridge lodestone, on the ground floor.
    let game = replay.game();
    let tile = super::session_replay::observe(game)
        .local
        .context("the local player")?
        .tile;
    assert_eq!(tile[2], 0);
    assert!(
        (tile[0] - 3233).abs() <= 3 && (tile[1] - 3222).abs() <= 3,
        "the player stands at {tile:?}"
    );
    Ok(())
}
