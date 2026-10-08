//! Client scenario tests (test-audit.md fix programme item 5).
use super::session_replay::{client_frames, server_frame, CycleOutput, Replay, Trace, FIXTURE};
use super::*;
use rs910_symbols::{component, interface, inv, loc, obj, param, script};
use std::collections::BTreeMap;

pub(super) fn ui(replay: &mut Replay) -> &mut crate::ui_runtime::Runtime {
    &mut replay.core.session.as_mut().unwrap().ui
}

/// The renderer's pick frame for the current cam2 camera, built the way
/// `ViewerApp` builds it each frame without a GPU (app.rs frame path):
/// the cam2 frame rebased on its integer look-at (`update_cam2_frame`),
/// the scene viewport the UI paint published, `LiveScene::update`, then
/// `Frame::collect_scene` / `collect_active_heights`.
fn install_static_pick_frame(replay: &mut Replay) -> anyhow::Result<()> {
    let game = replay
        .core
        .session
        .as_mut()
        .unwrap()
        .game
        .as_ref()
        .context("game")?;
    let base = [game.runtime.map.base_x, game.runtime.map.base_z];
    let generation = game.runtime.terrain_generation;
    let ui = &mut replay.core.session.as_mut().unwrap().ui;
    let (viewport, _) = ui.state.viewport.context("scene viewport")?;
    let mut cam2 = ui.engine.camera.cam2.frame().context("cam2 frame")?;
    let anchor = cam2.lookat.map(|v| v as i32);
    cam2.rebase(anchor);
    let mut camera = crate::camera::SceneCamera::new(anchor);
    camera.cam2 = Some(cam2);
    camera.viewport = (viewport[2], viewport[3]);
    camera.viewport_profile = ui.state.viewport_profile;
    let assets = &mut *replay.assets;
    let scene = assets.scene_graph.as_mut().context("scene graph")?;
    let live = assets.live_scene.as_mut().context("live scene")?;
    live.update(scene, camera.clone(), (base[0], base[1]), 0, [-1, -1]);
    let mut frame = crate::player_picking::Frame::new(
        &camera,
        [base[0] << 9, base[1] << 9],
        generation,
        viewport,
    );
    frame.collect_scene(
        scene,
        live,
        ui.engine.configs.locs.as_deref(),
        &Default::default(),
    );
    let (locs, objs) = ui.active_scene_keys(base);
    frame.collect_active_heights(scene, &locs, &objs);
    ui.engine.scene.drawn_view = Some(crate::ui_scene_options::DrawnView::of_frame(&frame));
    ui.engine.scene.player_picks = Some(frame);
    Ok(())
}

/// The pick frame of a scene with animated or multi-state locs as well: a
/// dynamic loc's model is built by the app's frame after the draw
/// plan (`refresh_dynamic_scene`: the culled updates, then the dispatched
/// opaque and transparent entities, then the plan's visible lists). The frame
/// is collected again over those models.
pub(super) fn install_pick_frame(replay: &mut Replay) -> anyhow::Result<()> {
    install_static_pick_frame(replay)?;
    let cycle = replay.core.cycle;
    let sun = crate::floor::SunLighting::environment_default(3, 0.0);
    let game = replay
        .core
        .session
        .as_mut()
        .and_then(|session| session.game.as_mut())
        .context("game")?;
    let assets = &mut *replay.assets;
    let (Some(live), Some(scene), Some(materials)) = (
        assets.live_scene.as_mut(),
        assets.scene_graph.as_mut(),
        assets.material_store.as_ref(),
    ) else {
        anyhow::bail!("the startup scene has no live scene, graph or materials");
    };
    let floors = &mut assets.floors;
    let Some(dynamic) = live.dynamic.as_mut() else {
        // No animated or multi-state locs: the static frame stands.
        return Ok(());
    };
    dynamic.var_overrides = Some(&game.cutscene)
        .filter(|cutscene| cutscene.scene_state == 0)
        .map(|cutscene| cutscene.var_overrides.clone())
        .unwrap_or_default();
    let phases = [
        live.draw.plan.culled_updates.clone(),
        live.draw.plan.dispatch_opaque.clone(),
        live.draw.plan.dispatch_transparent.clone(),
    ];
    let variables = game.runtime.feed.state.varps.as_mut();
    dynamic.with_variables(variables, |dynamic| {
        for (phase, ids) in phases.iter().enumerate() {
            for &id in ids {
                if !dynamic.contains(id) {
                    continue;
                }
                dynamic.refresh(
                    id,
                    phase != 0,
                    cycle,
                    &mut live.entities[id],
                    crate::dynamic_scene::RefreshWorld {
                        scene,
                        floors,
                        materials,
                        sun: &sun,
                    },
                )?;
            }
        }
        Ok(())
    })?;
    let planes = &live.draw.plan.model_planes;
    let visible = |id: &usize| {
        crate::dynamic_scene::model(scene, live.entities[*id].source).is_some()
            && crate::draw::entity_model_visible(&live.entities[*id], planes)
    };
    live.draw.plan.opaque = live
        .draw
        .plan
        .dispatch_opaque
        .iter()
        .copied()
        .filter(visible)
        .collect();
    live.draw.plan.transparent = live
        .draw
        .plan
        .dispatch_transparent
        .iter()
        .copied()
        .filter(visible)
        .collect();
    let ui = &mut replay.core.session.as_mut().context("session")?.ui;
    let frame = ui
        .engine
        .scene
        .player_picks
        .as_mut()
        .context("pick frame")?;
    frame.collect_scene(
        scene,
        live,
        ui.engine.configs.locs.as_deref(),
        &Default::default(),
    );
    ui.engine.scene.drawn_view = Some(crate::ui_scene_options::DrawnView::of_frame(frame));
    Ok(())
}

/// One live cycle whose renderer pick refresh is `ViewerApp`'s
/// `refresh_picks` closure (app.rs): the loc picks re-tested at the
/// queued click, else the mouse.
pub(super) fn pick_step(
    replay: &mut Replay,
    cycle: i32,
    inbound: &[u8],
) -> anyhow::Result<CycleOutput> {
    let mut scene = replay.assets.scene_graph.take().context("scene graph")?;
    let out = replay.step_live(cycle, inbound, &mut |_, ui| {
        let mouse = ui.input.click.unwrap_or(ui.engine.platform.mouse);
        if let Some(frame) = ui.engine.scene.player_picks.as_mut() {
            frame.refresh_locs(&mut scene, mouse);
        }
    });
    replay.assets.scene_graph = Some(scene);
    out
}

/// A canvas point where the pick frame's hit test (`refresh_locs`, the
/// model/clickbox pick) finds loc `id`,
/// nearest the middle of its screen capsule.
pub(super) fn hit_point(replay: &mut Replay, id: i32) -> anyhow::Result<[i32; 2]> {
    let ui = &mut replay.core.session.as_mut().unwrap().ui;
    let scene = replay.assets.scene_graph.as_mut().context("scene graph")?;
    let frame = ui
        .engine
        .scene
        .player_picks
        .as_mut()
        .context("pick frame")?;
    let k = frame
        .loc_picks
        .iter()
        .position(|p| p.id == id)
        .with_context(|| format!("loc {id} not drawn"))?;
    let c = frame.loc_picks[k].capsule;
    let mid = [(c.a[0] + c.b[0]) / 2, (c.a[1] + c.b[1]) / 2];
    let r = c.radius + (c.a[1] - c.b[1]).abs();
    let mut best: Option<(i32, [i32; 2])> = None;
    for y in (mid[1] - r..=mid[1] + r).step_by(2) {
        for x in (mid[0] - r..=mid[0] + r).step_by(2) {
            frame.refresh_locs(scene, [x, y]);
            if frame.loc_picks[k].hit {
                let d = (x - mid[0]).pow(2) + (y - mid[1]).pow(2);
                if best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, [x, y]));
                }
            }
        }
    }
    best.map(|(_, p)| p)
        .with_context(|| format!("loc {id} is never hit"))
}

/// The minimenu's `allEntries` after the last tick.
fn menu_entries(ui: &crate::ui_runtime::Runtime) -> Vec<crate::ui_minimenu::Entry> {
    let m = &ui.state.minimenu;
    m.entries.iter().map(|&e| m.entry(e).clone()).collect()
}

/// The minimenu's `allEntries` after the last tick: `(op, target, action)`.
pub(super) fn menu(replay: &mut Replay) -> Vec<(String, String, i32)> {
    let m = &ui(replay).state.minimenu;
    m.entries
        .iter()
        .map(|&e| {
            let e = m.entry(e);
            (e.op.clone(), e.target.clone().unwrap_or_default(), e.action)
        })
        .collect()
}

fn now() -> i64 {
    crate::logic_clock::monotonic_millis()
}

/// The recorded session up to `cycles`, replayed through the production
/// owners (`session_replay`), as the starting world of a scenario.
pub(super) fn replay_to(cycles: i32) -> anyhow::Result<Replay> {
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let mut replay = Replay::start(&trace)?;
    for cycle in 1..=cycles {
        replay.cycle(&trace, cycle)?;
    }
    Ok(replay)
}

/// Every client frame written by `cycles` further live cycles (server silent).
fn written(
    replay: &mut Replay,
    cycle: &mut i32,
    cycles: i32,
) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    let mut out = Vec::new();
    for _ in 0..cycles {
        *cycle += 1;
        out.extend(client_frames(&pick_step(replay, *cycle, &[])?.written)?);
    }
    Ok(out)
}

/// Packet writers (p2, p4, p1_alt2, p2_alt2/p2_alt3), for hand-derived
/// expected payloads.
pub(super) mod writers {
    pub fn p2(v: i32) -> [u8; 2] {
        [(v >> 8) as u8, v as u8]
    }
    pub fn p4(v: i32) -> [u8; 4] {
        v.to_be_bytes()
    }
    pub fn p1_alt2(v: i32) -> u8 {
        (-v) as u8
    }
    pub fn p2_alt2(v: i32) -> [u8; 2] {
        [(v >> 8) as u8, (v + 128) as u8]
    }
    pub fn p2_alt3(v: i32) -> [u8; 2] {
        [(v + 128) as u8, (v >> 8) as u8]
    }
}

/// The server packets that change interface state count towards the verify
/// id the client reports (`TRANSMITVAR_VERIFYID`) after the cycle they were
/// read in, as the original counts them: the interface and camera packets,
/// the draw order and "face here" do; the move action, the NPC attack
/// priority and the map flag do not. A quiet cycle reports nothing.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn interface_changes_are_reported_with_their_count() -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let mut replay = replay_to(130)?;
    let mut cycle = 130;
    // The counts of the recorded session go out first.
    live_step(&mut replay, &mut cycle, &[])?;
    live_step(&mut replay, &mut cycle, &[])?;
    let before = ui(&mut replay).state.life.verify;
    let move_action = [b"Go here\0".as_slice(), &123u16.to_be_bytes()].concat();
    let mut frames = live_step(
        &mut replay,
        &mut cycle,
        &[
            server_frame(sp::SETDRAWORDER, &[128 - 1]),
            server_frame(sp::SET_MOVEACTION, &move_action),
            server_frame(sp::REDUCE_NPC_ATTACK_PRIORITY, &[128 - 2]),
            server_frame(sp::SET_MAP_FLAG, &[128 + 10, 10]),
            server_frame(sp::SHOW_FACE_HERE, &[127]),
            server_frame(sp::CAM_RESET, &[]),
        ],
    )?;
    frames.extend(live_step(&mut replay, &mut cycle, &[])?);
    assert_eq!(
        ui(&mut replay).engine.menu.npc_attack_priority,
        crate::ui_player_options::AttackPriority::AlwaysRight,
        "the NPC attack priority reaches the menu"
    );
    let reports: Vec<_> = frames
        .iter()
        .filter(|(opcode, _)| *opcode == cp::TRANSMITVAR_VERIFYID)
        .collect();
    assert_eq!(
        reports,
        [&(
            cp::TRANSMITVAR_VERIFYID,
            (before + 3).to_be_bytes().to_vec()
        )],
        "three of the six packets count, reported once: {frames:02x?}"
    );
    let quiet = live_step(&mut replay, &mut cycle, &[])?;
    assert!(
        quiet
            .iter()
            .all(|(opcode, _)| *opcode != cp::TRANSMITVAR_VERIFYID),
        "nothing changed: {quiet:02x?}"
    );
    Ok(())
}

/// A world that stays silent for 2250 logic cycles (once its scene has been
/// drawn a few times) is treated as lost and reconnected; any byte starts the
/// count again. Meanwhile the client reports its round trip every 30 s of the
/// logic clock.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn a_silent_world_is_reconnected_after_2250_cycles() -> anyhow::Result<()> {
    use crate::proto::{client as cp, server as sp};
    let mut replay = replay_to(130)?;
    let mut cycle = 130;
    {
        let session = replay.core.session.as_mut().unwrap();
        session.io.incoming_idle = Default::default();
        session.machine.state_ticks = 11;
    }
    let mut pings = Vec::new();
    for silent in 1..=2000 {
        let frames = live_step(&mut replay, &mut cycle, &[])?;
        pings.extend(
            frames
                .into_iter()
                .filter(|(opcode, _)| *opcode == cp::PING_STATISTICS)
                .map(|(_, payload)| (silent, payload)),
        );
    }
    // A byte from the server: the count starts again.
    live_step(
        &mut replay,
        &mut cycle,
        &[server_frame(sp::NO_TIMEOUT, &[])],
    )?;
    // The cycle that took the byte counts as the first of the silence.
    for _ in 1..2250 {
        live_step(&mut replay, &mut cycle, &[])?;
    }
    let lost = live_step(&mut replay, &mut cycle, &[]).unwrap_err();
    assert!(
        format!("{lost:#}").contains("2250 cycles without a byte"),
        "{lost:#}"
    );
    // One report in the first 2000 quiet cycles (30 s after the recorded
    // session's last), the fixed round trip, the frame rate and no collector.
    assert_eq!(pings.len(), 1, "{pings:02x?}");
    let (at, payload) = &pings[0];
    assert!((1300..1450).contains(at), "reported at cycle {at}");
    assert_eq!(
        (payload[..2].to_vec(), payload[3]),
        (
            super::session_replay::REPLAY_PING_MS.to_le_bytes()[..2].to_vec(),
            255
        )
    );
    Ok(())
}

/// `UPDATE_REBOOT_TIMER` counts 30 logic cycles per tick in the world and
/// 2.5 per tick in a lobby state (13, 6, 15, 16), whatever interface is on top.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn the_reboot_timer_scale_follows_the_client_state() -> anyhow::Result<()> {
    use crate::login_state::{GAME, LOBBY, LOBBY_ENTER_GAME};
    use crate::proto::server as sp;
    let ticks = 4u16.to_be_bytes();
    for (state, expected) in [
        (GAME, 4 * 30 - 1),
        (LOBBY, 10 - 1),
        (LOBBY_ENTER_GAME, 10 - 1),
    ] {
        let mut replay = replay_to(130)?;
        let mut cycle = 130;
        replay.core.session.as_mut().unwrap().machine.state = state;
        live_step(
            &mut replay,
            &mut cycle,
            &[server_frame(sp::UPDATE_REBOOT_TIMER, &ticks)],
        )?;
        // The tick that followed the read counted one down.
        assert_eq!(
            ui(&mut replay).engine.reboot_timer,
            expected,
            "state {state}"
        );
    }
    Ok(())
}

/// `LAST_LOGIN_INFO` starts a lookup of the host name the account last
/// connected from; `lastlogin` reads it empty until it resolves, then the name
/// (the dotted address when the address has none and when the lookup does not
/// answer in time).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn the_last_login_packet_starts_a_host_name_lookup() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let mut replay = replay_to(130)?;
    let mut cycle = 130;
    assert!(ui(&mut replay).engine.login.last_login.is_none());
    live_step(
        &mut replay,
        &mut cycle,
        &[server_frame(sp::LAST_LOGIN_INFO, &[192, 0, 2, 7])],
    )?;
    let lookup = ui(&mut replay)
        .engine
        .login
        .last_login
        .as_mut()
        .context("a lookup")?;
    // Documentation addresses have no name: the dotted address once the
    // lookup answers or its time is up.
    let late = now() + rs910_ui::host_name::LOOKUP_TIMEOUT_MS + 1;
    assert_eq!(lookup.text(late), "192.0.2.7");
    Ok(())
}

/// A packet the client cannot decode ends the session as in the original: a
/// known packet with a payload that does not decode is reported and logs the
/// player out (a session event for the shell), while a byte that is no packet
/// at all breaks the stream, which the shell answers by reconnecting.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn a_packet_that_cannot_be_decoded_ends_the_session() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let mut replay = replay_to(130)?;
    let mut cycle = 130;
    // IF_SETTEXT cut short.
    let error = live_step(
        &mut replay,
        &mut cycle,
        &[server_frame(sp::IF_SETTEXT, &[0, 1])],
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("MalformedPacket"),
        "a session event for the shell: {error:#}"
    );
    let mut replay = replay_to(130)?;
    let mut cycle = 130;
    // Opcode 256 is outside the table (a two-byte opcode: 128 + 1, then 0).
    let error = live_step(&mut replay, &mut cycle, &[vec![129, 0]]).unwrap_err();
    assert!(
        format!("{error:#}").contains("connection lost: unknown server opcode 256"),
        "{error:#}"
    );
    Ok(())
}

/// The lobby connection keeps itself alive like the world's: the interface
/// change count in the lobby, a `NO_TIMEOUT` after fifty idle cycles, while
/// the lobby is the connection the player waits on (the lobby, or a world
/// login parked on its device check) and not otherwise.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn the_lobby_connection_sends_its_keepalive() -> anyhow::Result<()> {
    use crate::client_core::LiveIo;
    use crate::login_state::{LOBBY, LOBBY_ENTER_GAME};
    use std::io::Read;
    let mut replay = replay_to(130)?;
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let lobby = std::net::TcpStream::connect(listener.local_addr()?)?;
    let (mut peer, _) = listener.accept()?;
    lobby.set_nonblocking(true)?;
    peer.set_nonblocking(true)?;
    {
        let session = replay.core.session.as_mut().unwrap();
        session.io.lobby.stream = Some(crate::wire_stream::WireStream::plain(lobby));
        session.machine.state = LOBBY;
        session.reconnect_started = false;
        session.ui.state.life.verify = 7;
        session.ui.state.life.verify_changed = true;
    }
    let mut poll = |replay: &mut Replay| -> anyhow::Result<Vec<u8>> {
        replay.core.poll_lobby(&mut LiveIo)?;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut got = Vec::new();
        let mut buf = [0u8; 64];
        while let Ok(n @ 1..) = peer.read(&mut buf) {
            got.extend_from_slice(&buf[..n]);
        }
        Ok(got)
    };
    assert_eq!(
        poll(&mut replay)?,
        [22, 0, 0, 0, 7],
        "the lobby reports the count"
    );
    for quiet in 1..=50 {
        assert_eq!(poll(&mut replay)?, [] as [u8; 0], "idle cycle {quiet}");
    }
    assert_eq!(
        poll(&mut replay)?,
        [103],
        "NO_TIMEOUT after fifty idle cycles"
    );
    assert_eq!(
        poll(&mut replay)?,
        [] as [u8; 0],
        "the write started the count again"
    );

    // A world login parked on its device check (reply 42) keeps the lobby
    // alive; one that is not parked does not.
    {
        let session = replay.core.session.as_mut().unwrap();
        session.machine.state = LOBBY_ENTER_GAME;
        session.reconnect_started = true;
        session.ui.engine.login.reply = -3;
        session.io.lobby_idle_connection = Default::default();
    }
    for _ in 0..120 {
        assert_eq!(poll(&mut replay)?, [] as [u8; 0], "not parked");
    }
    replay.core.session.as_mut().unwrap().ui.engine.login.reply = 42;
    let mut seen = Vec::new();
    for _ in 0..52 {
        seen.extend(poll(&mut replay)?);
    }
    assert_eq!(seen, [103], "parked on the device check");
    Ok(())
}

/// Scenario 1: a loc under the mouse in the rebuilt Lumbridge scene → the
/// minimenu → the OPLOC1 bytes on the world socket.
///
/// The recorded session (`fixtures/session-replay`) leaves the player idle
/// at (3224, 3221); the camera is then fixed by real input (mouse wheel zoom
/// out, the left arrow key held twice per round for three rounds). The
/// pick frame is the renderer's (`install_pick_frame`) and the mouse sits on
/// a point where its hit test finds the Lumbridge courtyard ladder (loc
/// 36768 "Ladder" at (3229, 3224), cache ops `[Climb-up]`).
///
/// Expected, from the wire protocol and the cache:
/// - the scene options add Face here (the session's SHOW_FACE_HERE) then
///   Walk here (23); the loc loop adds ops last to first with white-tagged
///   names: Examine (1002, the default op 5) then Climb-up (3); the cancel
///   option heads `allEntries`.
/// - the menu update keeps actions >= 1000 first, then the normal entries in
///   insertion order; the active (left-click) entry is the tail, Climb-up.
/// - using the menu option writes OPLOC1 (client packet 12, size 9):
///   `p1_alt2(ctrl=0) p2(z) p4(id) p2_alt3(x)` with the absolute tile.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn loc_under_the_mouse_builds_the_menu_and_writes_oploc1() -> anyhow::Result<()> {
    const LADDER: i32 = loc::LUMBRIDGE_CASTLE_LADDER.id();
    let mut replay = replay_to(130)?;
    let mut cycle = 130;
    let step = |replay: &mut Replay, cycle: &mut i32, n: i32| -> anyhow::Result<()> {
        for _ in 0..n {
            *cycle += 1;
            pick_step(replay, *cycle, &[])?;
        }
        Ok(())
    };
    retained_mouse_move(ui(&mut replay), [500, 400], now());
    for _ in 0..15 {
        // AWT wheel rotation +1: zoom out.
        ui(&mut replay).input.wheel += 1;
        step(&mut replay, &mut cycle, 1)?;
    }
    for _ in 0..3 {
        // KeyEvent.VK_LEFT (37): rotate the camera.
        retained_key(ui(&mut replay), 37, true, None, now());
        step(&mut replay, &mut cycle, 2)?;
        retained_key(ui(&mut replay), 37, false, None, now());
        step(&mut replay, &mut cycle, 10)?;
    }
    ui(&mut replay).paint(cycle, true, [0.; 3])?;
    ui(&mut replay).paint(cycle, true, [0.; 3])?;
    install_pick_frame(&mut replay)?;
    let mouse = hit_point(&mut replay, LADDER)?;
    retained_mouse_move(ui(&mut replay), mouse, now());
    step(&mut replay, &mut cycle, 1)?;
    let ladder = "<col=ffff>Ladder".to_string();
    let entry = |op: &str, target: &str, action| (op.to_string(), target.to_string(), action);
    assert_eq!(
        menu(&mut replay),
        [
            entry("Cancel", "", 1006),
            entry("Examine", &ladder, 1002),
            entry("Face here", "", 60),
            entry("Walk here", "", 23),
            entry("Climb-up", &ladder, 3),
        ],
        "minimenu under the ladder at {mouse:?}"
    );
    let m = &ui(&mut replay).state.minimenu;
    assert_eq!(
        m.active.map(|a| m.entry(a).op.clone()).as_deref(),
        Some("Climb-up")
    );
    // The pointer the client shows follows the active option: its cursor is
    // the loc type's own for that operation, else the menu default.
    let entries = menu_entries(ui(&mut replay));
    let climb = entries.iter().find(|e| e.op == "Climb-up").unwrap();
    let ladder_type = ui(&mut replay)
        .engine
        .configs
        .locs
        .as_deref()
        .and_then(|locs| locs.get(LADDER as u32))
        .cloned()
        .context("ladder type")?;
    let default_menu_cursor = ui(&mut replay).engine.menu.default_cursors[1];
    let expected = if ladder_type.cursor[0] != -1 {
        ladder_type.cursor[0]
    } else {
        default_menu_cursor
    };
    assert_eq!(climb.cursor, expected);
    assert_eq!(
        ui(&mut replay).cursor_id(),
        expected,
        "pointer over the ladder"
    );
    // A left click on the scene: the active option.
    retained_mouse_button(ui(&mut replay), 0, true, now());
    let mut frames = written(&mut replay, &mut cycle, 1)?;
    retained_mouse_button(ui(&mut replay), 0, false, now());
    frames.extend(written(&mut replay, &mut cycle, 2)?);
    let oplocs: Vec<_> = frames
        .iter()
        // Client packets OPLOC1..6 / OPLOCT.
        .filter(|(op, _)| [12, 96, 27, 61, 119, 39, 21].contains(op))
        .cloned()
        .collect();
    let mut expected = vec![writers::p1_alt2(0)];
    expected.extend(writers::p2(3224));
    expected.extend(writers::p4(LADDER));
    expected.extend(writers::p2_alt3(3229));
    assert_eq!(
        oplocs,
        [(12u8, expected)],
        "OPLOC1 on the socket; all frames {frames:02x?}"
    );
    Ok(())
}

/// Type `text` into the client the way winit delivers it: per character
/// `keyPressed` (the AWT key code) with its typed text, then `keyReleased`
/// (`retained_key`), one character per logic cycle.
fn type_text(
    replay: &mut Replay,
    cycle: &mut i32,
    text: &str,
) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    let mut frames = Vec::new();
    for ch in text.chars() {
        // AWT key events: VK_ENTER 10, VK_SPACE 32, VK_A..VK_Z 65..90.
        let awt = match ch {
            '\n' => 10,
            'a'..='z' => ch.to_ascii_uppercase() as i32,
            ' ' => 32,
            other => anyhow::bail!("no key for {other:?}"),
        };
        let typed = ch.to_string();
        retained_key(ui(replay), awt, true, Some(&typed), now());
        retained_key(ui(replay), awt, false, None, now());
        frames.extend(written(replay, cycle, 1)?);
    }
    frames.extend(written(replay, cycle, 2)?);
    Ok(frames)
}

/// Scenario 4: chat typed into the chatbox with key events → Enter →
/// MESSAGE_PUBLIC on the world socket, through the cache chat scripts and
/// `chat_sendpublic` with the cache Huffman table; and the send gate.
///
/// Expected, independent of the client:
/// - the word-pack bytes are the dev server's own Huffman encoding of the
///   same text: its MESSAGE_PUBLIC echo in the recording (which reads
///   `g2 pid, g2 colour/effect, g1 crown` then the word-pack string),
///   produced by the server's word-pack encoder;
/// - `chat_sendpublic`: `p1(size) p1(colour 0) p1(effect 0)` word pack,
///   client packet MESSAGE_PUBLIC 95 (size -1), and nothing at all when
///   `staffModLevel == 0 && (dobVerified && !playerIsQuickChat ||
///   loggedInQuickChat)`;
/// - UPDATE_DOB (server packet 126, size 4) sets `lobbyDOB = g3s`,
///   `dobVerified = g1 == 1`.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn typed_chat_sends_message_public_with_the_cache_huffman_and_the_send_gate() -> anyhow::Result<()>
{
    const MESSAGE_PUBLIC: u8 = 95;
    let root = &rs910_core::test_support::client_dir();
    let trace = Trace::load(&root.join(FIXTURE))?;
    let echo = super::session_replay::arrivals(&trace)?
        .into_iter()
        .find(|a| a.opcode == crate::proto::server::MESSAGE_PUBLIC)
        .context("the recorded server echo")?;
    // g2 pid, g2 colour/effect, g1 crown, then the word pack (smart length 12).
    let server_wordpack = echo.payload[5..].to_vec();
    assert_eq!(server_wordpack[0], "hello replay".len() as u8);
    let mut expected = vec![0u8, 0];
    expected.extend(&server_wordpack);

    let mut replay = replay_to(30)?;
    let mut cycle = 30;
    let public = |frames: &[(u8, Vec<u8>)]| -> Vec<Vec<u8>> {
        frames
            .iter()
            .filter(|(op, _)| *op == MESSAGE_PUBLIC)
            .map(|(_, p)| p.clone())
            .collect()
    };
    // The recorded login reply: staffModLevel 2, dobVerified false.
    assert_eq!(ui(&mut replay).engine.account.staff_mod_level, 2);
    let sent = type_text(&mut replay, &mut cycle, "\nhello replay\n")?;
    assert_eq!(public(&sent), [expected.clone()], "staff account");

    // A player account (the login reply's staffModLevel 0) whose date of
    // birth the server has not verified still chats.
    ui(&mut replay).engine.account.staff_mod_level = 0;
    let sent = type_text(&mut replay, &mut cycle, "\nhello replay\n")?;
    assert_eq!(
        public(&sent),
        [expected.clone()],
        "player account, DOB unverified"
    );

    // UPDATE_DOB: lobbyDOB 0, dobVerified 1 -> the gate drops the message.
    cycle += 1;
    pick_step(&mut replay, cycle, &[126, 0, 0, 0, 1])?;
    assert!(
        ui(&mut replay).engine.account.dob_verified,
        "UPDATE_DOB applied"
    );
    let sent = type_text(&mut replay, &mut cycle, "\nhello replay\n")?;
    assert_eq!(public(&sent), Vec::<Vec<u8>>::new(), "gated player account");

    // Staff chat is never gated.
    ui(&mut replay).engine.account.staff_mod_level = 2;
    let sent = type_text(&mut replay, &mut cycle, "\nhello replay\n")?;
    assert_eq!(public(&sent), [expected], "staff account, DOB verified");
    Ok(())
}

/// The canvas rectangle `[x, y, w, h]` of the first visible component
/// matching `pred`, found with the layer walk's own offsets (ui_loop.rs
/// `layer`: `x + parent offset`, children at `x - scrollx`, sub-interfaces
/// under their mount component, hidden layers skipped).
pub(super) fn component_rect(
    ui: &crate::ui_runtime::Runtime,
    pred: &dyn Fn(&crate::ui_components::Component) -> bool,
) -> Option<[i32; 4]> {
    component_rects(ui, pred).into_iter().next()
}

/// Every visible component matching `pred`, in layer-walk order (see
/// [`component_rect`]).
pub(super) fn component_rects(
    ui: &crate::ui_runtime::Runtime,
    pred: &dyn Fn(&crate::ui_components::Component) -> bool,
) -> Vec<[i32; 4]> {
    let mut found = Vec::new();
    walk_components(ui, &mut |rect, c| {
        if pred(c) {
            found.push(rect);
        }
    });
    found
}

/// Calls `visit` with the canvas rectangle of every visible component, in
/// layer-walk order (see [`component_rect`]).
pub(super) fn walk_components(
    ui: &crate::ui_runtime::Runtime,
    visit: &mut dyn FnMut([i32; 4], &crate::ui_components::Component),
) {
    fn layer(
        ui: &crate::ui_runtime::Runtime,
        array: &crate::ui_components::Array,
        parent: i32,
        offset: [i32; 2],
        visit: &mut dyn FnMut([i32; 4], &crate::ui_components::Component),
    ) {
        let items: Vec<_> = array.borrow().iter().flatten().cloned().collect();
        for c in items {
            let comp = c.borrow();
            let f = &comp.f;
            if f.layer != parent || f.hide {
                continue;
            }
            let (x, y) = (f.x + offset[0], f.y + offset[1]);
            visit([x, y, f.width, f.height], &comp);
            if f.r#type == 0 {
                let inner = [x - f.scrollx, y - f.scrolly];
                layer(ui, array, f.parentlayer, inner, visit);
                if let Some(dynamic) = comp.sorted.as_ref().or(comp.children.as_ref()) {
                    layer(ui, dynamic, f.parentlayer, inner, visit);
                }
                if let Some(sub) = ui.state.life.subs.get(f.parentlayer) {
                    let id = sub.borrow().id;
                    if let Some(i) = ui.store.interfaces.get(&id) {
                        let i = i.borrow();
                        let array = i.sorted.as_ref().unwrap_or(&i.components).clone();
                        drop(i);
                        layer(ui, &array, -1, inner, visit);
                    }
                }
            }
        }
    }
    let Some(top) = ui.store.interfaces.get(&ui.state.life.top) else {
        return;
    };
    let top = top.borrow();
    let array = top.sorted.as_ref().unwrap_or(&top.components).clone();
    drop(top);
    layer(ui, &array, -1, [0, 0], visit);
}

/// The recorded dev-server frame `opcode` whose payload starts with `head`
/// in row `row` of a `fixtures/replays` corpus.
fn corpus_frame(scenario: &str, row: usize, opcode: u8, head: &[u8]) -> anyhow::Result<Vec<u8>> {
    let rows = crate::test_support::replay_json(scenario, "frames.json");
    rows[row]["frames"]
        .as_array()
        .context("frames")?
        .iter()
        .map(|f| {
            f.as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect::<Vec<u8>>()
        })
        .find(|f| {
            f[0] == opcode
                && crate::net::decode_frame(f)
                    .ok()
                    .flatten()
                    .is_some_and(|(frame, _)| frame.payload.starts_with(head))
        })
        .with_context(|| format!("{scenario} row {row}: no frame {opcode}"))
}

/// Scenario 2: UPDATE_INV_FULL → the cache backpack's `oninvtransmit`
/// shows the item → right click on its slot → the minimenu → Drop →
/// IF_BUTTON8 on the world socket.
///
/// Inputs: the dev server's recorded UPDATE_INV_FULL for the backpack
/// (inv 93, `fixtures/replays/consumable-drink` row 0: slot 0 holds obj 3008
/// "Energy potion (4)", slot 1 obj 3016), written to the world socket of the
/// recorded session, whose server opened the backpack 1473 under 1477:104.
///
/// Expected, from the wire protocol and the cache:
/// - UPDATE_INV_FULL: `g2 inv, g1 flags, g2 size`, per slot `g2 obj+1, g1
///   count`; the cache backpack script builds the slot components of 1473:7
///   (child = slot) with `invobject`.
/// - The obj 3008 iops are `[Drink, -, -, Empty, Drop]`; the backpack
///   script installs them as component ops 1, 7, 8 and op 10 Examine, with
///   the target verb "Use". The component options add ops 10..6 (action
///   1007), the target verb (25), ops 5..1 (57) with `entityId = op + 1`,
///   after the cancel option; `opbase` is the name in the cache's backpack
///   colour.
/// - The open menu (custom formatting) lists `allEntries` bottom-up; row
///   `k` (from the tail) has baseline `rowHeight * (count-1-k) + ascent +
///   menuY + 21` and takes clicks strictly between `baseline - ascent - 1`
///   and `baseline + descent`.
/// - Action 1007 with entityId 8 → the op trigger → IF_BUTTON8 (client
///   packet 159, opcode 50, size 8) with its button trailer:
///   `p2_alt3(obj) p2_alt2(slot) p4(parent)`.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn backpack_item_right_click_drop_writes_if_button8() -> anyhow::Result<()> {
    const BACKPACK: i32 = component::backpack::SLOTS.packed();
    const POTION: i32 = obj::ENERGY_POTION_4.id();
    let inv = corpus_frame(
        "consumable-drink",
        0,
        6,
        &(inv::BACKPACK.id() as u16).to_be_bytes(),
    )?;
    let mut replay = replay_to(30)?;
    let mut cycle = 30;
    cycle += 1;
    pick_step(&mut replay, cycle, &inv)?;
    written(&mut replay, &mut cycle, 3)?;
    let slot0 = |c: &crate::ui_components::Component| {
        c.f.parentlayer == BACKPACK && c.f.id == 0 && c.f.invobject == POTION
    };
    let rect = component_rect(ui(&mut replay), &slot0)
        .context("backpack slot 0 does not show obj 3008")?;
    let pos = [rect[0] + rect[2] / 2, rect[1] + rect[3] / 2];
    retained_mouse_move(ui(&mut replay), pos, now());
    retained_mouse_button(ui(&mut replay), 2, true, now());
    written(&mut replay, &mut cycle, 1)?;
    retained_mouse_button(ui(&mut replay), 2, false, now());
    written(&mut replay, &mut cycle, 1)?;
    let name = "<col=b8d1d1>Energy potion (4)".to_string();
    let e = |op: &str, target: &str, action: i32| (op.to_string(), target.to_string(), action);
    let expected = [
        e("Cancel", "", 1006),
        e("Examine", &name, 1007),
        e("Drop", &name, 1007),
        e("Empty", &name, 1007),
        e("Use", &name, 25),
        e("Drink", &name, 57),
    ];
    assert_eq!(menu(&mut replay), expected);
    let m = &ui(&mut replay).state.minimenu;
    assert!(m.open && !m.grouped, "the right click opened a plain menu");
    let ops: Vec<i64> = m.entries.iter().map(|&e| m.entry(e).entity_id).collect();
    assert_eq!(ops, [0, 10, 8, 7, 0, 1], "component op + 1 per entry");
    // Click the Drop row (allEntries index 2).
    let [menu_x, menu_y, _, _] = m.popup.bounds;
    let (ascent, count) = (m.popup.ascent, m.entries.len() as i32);
    let baseline = m.row_height * (count - 1 - 2) + ascent + menu_y + 21;
    let row = [menu_x + 20, baseline - ascent / 2];
    retained_mouse_move(ui(&mut replay), row, now());
    retained_mouse_button(ui(&mut replay), 0, true, now());
    let mut frames = written(&mut replay, &mut cycle, 1)?;
    retained_mouse_button(ui(&mut replay), 0, false, now());
    frames.extend(written(&mut replay, &mut cycle, 2)?);
    // Client packets IF_BUTTON1..10.
    let buttons = [111u8, 68, 86, 29, 70, 83, 63, 50, 66, 67];
    let sent: Vec<_> = frames
        .iter()
        .filter(|(op, _)| buttons.contains(op))
        .cloned()
        .collect();
    let mut payload = writers::p2_alt3(POTION).to_vec();
    payload.extend(writers::p2_alt2(0));
    payload.extend(writers::p4(BACKPACK));
    assert_eq!(
        sent,
        [(50u8, payload)],
        "IF_BUTTON8 Drop; all frames {frames:02x?}"
    );
    assert!(!ui(&mut replay).state.minimenu.open, "the menu closed");
    Ok(())
}

/// A logged-in retained UI without a world socket (the settings corpus
/// harness of `ui_settings_replay`): `Runtime::new` over the pack, a
/// `Game::login` and the simulated logic clock.
struct Direct {
    pack: Pack,
    ui: crate::ui_runtime::Runtime,
    game: crate::client_game::ClientGame,
    clock: crate::test_support::SimClock,
}

impl Direct {
    fn new(canvas: [i32; 2]) -> anyhow::Result<Self> {
        let pack = crate::test_support::require_pack("client.config.js5");
        let game = crate::client_game::ClientGame::login(
            &pack,
            1,
            crate::protocol910::live::Feed::default(),
            910,
            true,
        )?;
        let mut ui = crate::ui_runtime::Runtime::new(pack.clone())?;
        ui.resize(canvas)?;
        ui.engine.account.logged_in_members = true;
        ui.diagnostics.capture = true;
        // The window owner's fullscreen display modes (install_canvas_state),
        // read by the settings scripts' `fullscreen_modecount`.
        ui.engine.platform.fullscreen_modes = Some(vec![crate::ui_runtime::FullscreenMode {
            width: canvas[0],
            height: canvas[1],
            bit_depth: 24,
            refresh: 60,
        }]);
        // A fixed clock on this test thread (scripts read `clientclock`,
        // varp/varc timers and the world map animate on it).
        let clock = crate::test_support::SimClock(1_000_000);
        crate::logic_clock::set_test_now(Some(clock.0));
        Ok(Self {
            pack,
            ui,
            game,
            clock,
        })
    }

    /// Recorded server frames through the production decoders: entity
    /// packets to `Game::apply_next` (and the map install), UI packets
    /// through `session::parse_ui_event` → `Runtime::packet`.
    fn frames(&mut self, frames: &serde_json::Value) -> anyhow::Result<()> {
        for raw in frames.as_array().context("frames")? {
            let bytes: Vec<u8> = raw
                .as_array()
                .context("frame")?
                .iter()
                .map(|n| n.as_u64().unwrap() as u8)
                .collect();
            let (frame, used) = crate::net::decode_frame(&bytes)?.context("frame")?;
            anyhow::ensure!(used == bytes.len(), "frame length");
            let game = &mut self.game;
            if game.runtime.feed.enqueue(frame.opcode, &frame.payload) {
                game.apply_next(game.cycle as i64)
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                if game.runtime.map_request.is_some() {
                    let map = game.runtime.prepare_map(&self.pack)?;
                    game.runtime
                        .install_map(map)
                        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                }
            } else if let Some(event) =
                crate::session::parse_ui_event(frame.opcode, &frame.payload)?
            {
                let ui = &mut self.ui;
                self.clock.with(game, |vars| ui.packet(vars, &event))?;
            }
        }
        let now = self.clock.0;
        self.game
            .poll_vars(|| now)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        self.ticks(40)
    }

    /// Logic cycles: `begin_session_cycle`'s mouse latch, then `ui.tick`.
    fn ticks(&mut self, n: i32) -> anyhow::Result<()> {
        for _ in 0..n {
            self.ui.engine.platform.mouse = self.ui.engine.platform.pending_mouse;
            self.game.cycle += 1;
            crate::logic_clock::set_test_now(Some(
                self.clock.0 + crate::logic_clock::INTERVAL_NS / 1_000_000,
            ));
            self.clock.tick(&mut self.game, &mut self.ui)?;
        }
        Ok(())
    }

    /// A left click at `pos`: move, press (one cycle), release.
    fn click(&mut self, pos: [i32; 2]) -> anyhow::Result<()> {
        retained_mouse_move(&mut self.ui, pos, self.clock.0);
        self.ticks(1)?;
        retained_mouse_button(&mut self.ui, 0, true, self.clock.0);
        self.ticks(1)?;
        retained_mouse_button(&mut self.ui, 0, false, self.clock.0);
        self.ticks(3)
    }
}

impl Drop for Direct {
    fn drop(&mut self) {
        crate::logic_clock::set_test_now(None);
    }
}

/// One recorded preferences row: field values and the serialised block.
type RecordedPreferences = (BTreeMap<String, i32>, Vec<u8>);

/// Recorded preferences rows (`fixtures/scenarios/settings`), one per
/// input line.
fn recorded_preferences() -> anyhow::Result<Vec<RecordedPreferences>> {
    let root = rs910_core::test_support::client_dir().join("fixtures/scenarios/settings");
    std::fs::read_to_string(root.join("recorded-output.txt"))?
        .lines()
        .map(|line| {
            let mut parts = line.split('|');
            let fields = parts
                .next()
                .context("fields")?
                .split(',')
                .map(|kv| {
                    let (k, v) = kv.split_once('=').context("field")?;
                    Ok((k.to_string(), v.parse()?))
                })
                .collect::<anyhow::Result<_>>()?;
            let hex = parts.next().context("block")?;
            let block = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
                .collect::<Result<_, _>>()?;
            Ok((fields, block))
        })
        .collect()
}

/// Scenario 3: the settings window opened by the dev server's IF_OPENTOP /
/// IF_OPENSUB bytes (`fixtures/replays/settings-tabs`, through the
/// Graphics tab), a mouse click on the cache Ground decoration control,
/// the saved ClientOptions v38 file, and the scene-rebuild effect.
///
/// The preferences file installed first holds a v38 block whose every
/// field is non-default (the original forces `customCursors`, `safeMode`,
/// `unused2`, `unused6` back to their defaults on load; the two reserved
/// bytes are constant 0); the five volumes are 100/90/80/70/60, so a slot
/// swap cannot hide. Expected bytes: the recorded client options
/// (`fixtures/scenarios/settings/recorded-output.txt`, recorded once from
/// the original client for `oracle-input.txt`): row 1 is the decode of that
/// block (read + clamp), the last row the block after the preference
/// commands the cache "Toggle" onop script runs for the click, each with
/// its effect: `detail_grounddecor_on(1)` → groundDecoration 1 (then the
/// world rebuild and the preference save); its consistency pass
/// `detail_texturing(0)` → textures 0, `detail_hardshadows(0)` →
/// sceneryShadows 0, `detail_bloom(0)` → bloom off, a no-op while the
/// toolkit's bloom is off, `detail_waterdetail_high(0)` → waterDetail 0,
/// `detail_dof` pops only and `autosetup_setcustom` → preset 0.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn settings_click_saves_the_recorded_preferences_block_and_rebuilds() -> anyhow::Result<()> {
    let recorded = recorded_preferences()?;
    let [_, (loaded_fields, loaded), .., (clicked_fields, clicked)] = &recorded[..] else {
        anyhow::bail!("oracle rows");
    };
    let input = std::fs::read_to_string(
        rs910_core::test_support::client_dir().join("fixtures/scenarios/settings/oracle-input.txt"),
    )?;
    let hex = input
        .lines()
        .find_map(|l| l.strip_prefix("decode "))
        .context("decode line")?;
    let file_block: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<Result<_, _>>()?;
    assert_eq!(loaded_fields["groundDecoration"], 0);
    assert_eq!(clicked_fields["groundDecoration"], 1);

    let mut d = Direct::new([1280, 720])?;
    let dir = std::env::temp_dir().join(format!(
        "client910-scenario-settings-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("preferences.dat");
    std::fs::write(&path, &file_block)?;
    d.game
        .ui_variables
        .queries
        .preferences
        .install(path.clone());
    // Loading the preferences: the Rust decode + clamp of the file equals the recording's.
    assert_eq!(
        &d.game.ui_variables.queries.preferences.options.encode(),
        loaded
    );

    // The dev server's settings-navigation session: login, then the
    // Options, Settings and Graphics clicks, each followed by the frames
    // the server answered with. The clicks are real mouse clicks on the
    // cache components and must write the IF_BUTTON1 bytes the server
    // accepted (`wire`).
    let rows = crate::test_support::replay_json("settings-navigation", "frames.json");
    d.frames(&rows[0]["frames"])?;
    for row in &rows.as_array().context("rows")?[1..=3] {
        let action = row["action"].as_array().context("action")?;
        let (uid, child) = (
            action[0].as_i64().unwrap() as i32,
            action[1].as_i64().unwrap() as i32,
        );
        d.ui.engine.outgoing.clear();
        if let Some(key) = row["keyboard"].as_i64() {
            // The recorded key (Esc, 27, opens the Options menu).
            retained_key(&mut d.ui, key as i32, true, None, d.clock.0);
            d.ticks(1)?;
            retained_key(&mut d.ui, key as i32, false, None, d.clock.0);
            d.ticks(3)?;
        } else {
            let rect = component_rect(&d.ui, &|c| c.f.parentlayer == uid && c.f.id == child)
                .filter(|r| r[2] > 0 && r[3] > 0)
                .with_context(|| format!("{} {uid}:{child} not on screen", row["label"]))?;
            d.click([rect[0] + rect[2] / 2, rect[1] + rect[3] / 2])?;
        }
        let wire: Vec<u8> = row["wire"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        let buttons: Vec<_> = client_frames(&d.ui.engine.outgoing)?
            .into_iter()
            .filter(|(op, _)| *op == 111)
            .map(|(op, p)| [vec![op], p].concat())
            .collect();
        assert_eq!(buttons, [wire], "{} click", row["label"]);
        d.frames(&row["frames"])?;
    }
    let list = component::graphics_settings_panel::CONTROL_LIST.packed();
    let rect = component_rect(&d.ui, &|c| c.f.parentlayer == list && c.f.id == 10)
        .context("the Ground decoration control is not on screen")?;
    let pos = [rect[0] + rect[2] / 2, rect[1] + rect[3] / 2];
    d.game
        .ui_variables
        .queries
        .preferences
        .pending_effects
        .clear();
    d.click(pos)?;
    let saved = std::fs::read(&path)?;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        saved, *clicked,
        "saved v38 block vs the recorded preferences block"
    );
    let prefs = &mut d.game.ui_variables.queries.preferences;
    // The side effects in command order: world rebuild (grounddecor),
    // resetModelCaches (texturing), world.rebuild (hardshadows, waterdetail),
    // queued for the scene rebuild owner (app.rs `apply_scene_preferences`),
    // which rebuilds from `BuildPrefs::from_options`.
    use crate::ui_preferences::PreferenceEffect::{ResetModelCaches, SceneRebuild};
    assert_eq!(
        prefs.drain_effects(),
        [SceneRebuild, ResetModelCaches, SceneRebuild, SceneRebuild]
    );
    assert_eq!(
        crate::rebuild::BuildPrefs::from_options(&prefs.options).ground_decoration,
        1
    );
    Ok(())
}

/// Scenario 6: the world map opened from the minimap globe with a real
/// mouse click, the server's answer, loading, the real paint path, and the
/// minimenu of a hovered map element.
///
/// Inputs: `fixtures/replays/world-map` (dev server `export-fixtures`
/// scenario `world-map`): the login frames, then what `Player.
/// worldMapOperation` answers to IF_BUTTON1 1465:9 (CLIENT_SETVARC_LARGE
/// 674, IF_OPENSUB 1421 into 1477:28 and 1422 into 1477:33) and the request
/// bytes it accepted (`wire`).
///
/// Expected, from the wire protocol and the cache:
/// - the globe is cache component 1465:9 (op 1 "World Map"): IF_BUTTON1
///   (client packet 342) `p2_alt3(-1) p2_alt2(-1) p4(1465:9)`, the
///   server's accepted `wire`;
/// - the world map draw inside the interface draw's clientcode-1400
///   branch: the bounds reset to the map component, the area's chunk
///   images, then the element labels — the cache world-map element 82
///   "Lumbridge" (and its neighbours) drawn as glyph runs inside the clip;
/// - a hovered element with ops adds them last to first with action
///   `1008 + slot`, `entityId` the element id and the element category,
///   after the map's jump/zoom animation has settled.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn world_map_opens_from_the_globe_paints_lumbridge_and_offers_element_ops() -> anyhow::Result<()> {
    let rows = crate::test_support::replay_json("world-map", "frames.json");
    let mut d = Direct::new([1280, 720])?;
    d.frames(&rows[0]["frames"])?;
    let globe = component::minimap::WORLD_MAP_BUTTON.packed();
    let r = component_rect(&d.ui, &|c| c.f.parentlayer == globe && c.f.id == -1)
        .filter(|r| r[2] > 0)
        .context("the minimap globe is not on screen")?;
    d.ui.engine.outgoing.clear();
    d.click([r[0] + r[2] / 2, r[1] + r[3] / 2])?;
    let wire: Vec<u8> = rows[1]["wire"]
        .as_array()
        .context("wire")?
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let mut expected = vec![111u8];
    expected.extend(writers::p2_alt3(-1));
    expected.extend(writers::p2_alt2(-1));
    expected.extend(writers::p4(globe));
    assert_eq!(
        wire, expected,
        "the dev server accepted the recorded IF_BUTTON1"
    );
    assert_eq!(d.ui.engine.outgoing, wire, "the globe click");
    d.frames(&rows[1]["frames"])?;
    let map = component_rect(&d.ui, &|c| c.f.clientcode == crate::ui_loop::WORLD_MAP)
        .context("the world map component is not open")?;
    // Loaded, re-centred on the player by the map's own scripts, and the
    // jump/zoom animation settled.
    d.settle_world_map()?;
    d.ticks(80)?;
    d.settle_world_map()?;
    // app.rs frame path: the renderer's world-map owner, then the UI paint.
    d.ui.target.world_map = Some(d.ui.engine.world_map.clone());
    let out = d.ui.paint(d.game.cycle, true, [0.; 3])?;
    let clip = [map[0], map[1], map[0] + map[2], map[1] + map[3]];
    let start = out
        .recording
        .ops
        .iter()
        .position(|o| matches!(o, crate::ui_paint::Op::ResetBounds(b) if *b == clip))
        .context("no world map draw inside the component clip")?;
    let inside = |p: [i32; 2]| {
        p[0] < clip[2] && p[1] < clip[3] && p[0] > clip[0] - 512 && p[1] > clip[1] - 512
    };
    let (mut images, mut labels) = (0, String::new());
    for op in &out.recording.ops[start + 1..] {
        match op {
            crate::ui_paint::Op::ResetBounds(_) => break,
            crate::ui_paint::Op::Sprite(_, pos, _) => {
                assert!(inside(*pos), "map sprite at {pos:?} outside {clip:?}");
                images += 1;
            }
            crate::ui_paint::Op::Scaled(_, rect, _) => {
                assert!(
                    inside([rect[0], rect[1]]),
                    "map image at {rect:?} outside {clip:?}"
                );
                images += 1;
            }
            // Labels at the edge are drawn and cut by the clip.
            crate::ui_paint::Op::Glyph(
                _,
                crate::font_layout::Draw::Glyph {
                    code,
                    shadow: false,
                    ..
                },
                _,
            ) => labels.push(char::from(*code)),
            _ => {}
        }
    }
    assert!(images > 20, "{images} chunk/element images");
    for town in ["Lumbridge", "LumbridgeSwamp", "DraynorVillage", "AlKharid"] {
        assert!(labels.contains(town), "label {town} not drawn: {labels}");
    }
    // Hover the drawn element with ops nearest the map centre.
    let (element, category, [x0, x1, y0, y1]) = {
        let m = d.ui.engine.world_map.borrow();
        let area = m.map.area.as_ref().context("area")?;
        let centre = [map[0] + map[2] / 2, map[1] + map[3] / 2];
        m.containers
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| {
                let e = &area.elements[c.element];
                let t = m.element_types.get(e.id)?;
                t.ops
                    .iter()
                    .any(Option::is_some)
                    .then_some((e.id, t.category, c.sprite))
            })
            .min_by_key(|(_, _, b)| {
                (b[0] + b[1] - 2 * centre[0]).abs() + (b[2] + b[3] - 2 * centre[1]).abs()
            })
            .context("no element with ops drawn")?
    };
    retained_mouse_move(&mut d.ui, [(x0 + x1) / 2, (y0 + y1) / 2], d.clock.0);
    d.ticks(2)?;
    let options: Vec<_> = menu_entries(&d.ui)
        .into_iter()
        .filter(|e| (1008..=1012).contains(&e.action))
        .map(|e| (e.op, e.target, e.action, e.entity_id, e.tile_x))
        .collect();
    // Cache map element ops of the "Map" link elements: op 1 "Open".
    assert_eq!(
        options,
        [(
            "Open".to_string(),
            Some("Map".to_string()),
            1008,
            i64::from(element),
            category
        )]
    );
    // Presses on the map: the press becomes
    // `dx = (mouseX - x - w/2) * 2 / zoom`, `dz = -(mouseY - y -
    // h/2) * 2 / zoom` display tiles from the view centre, then
    // the game update sends CLICKWORLDMAP (client packet 18, size 4)
    // `p4_alt3(level << 28 | x << 14 | z)`. A press `k` pixels above another
    // lands `2k / zoom` tiles further north (+z) on the same x.
    let zoom = f64::from(d.ui.engine.world_map.borrow().map.zoom);
    let centre = [map[0] + map[2] / 2, map[1] + map[3] / 2];
    let mut press = |pos: [i32; 2]| -> anyhow::Result<[i32; 3]> {
        // The map's own timer scripts move the view a moment after a press;
        // let them finish so that the next press starts from a still map.
        d.ticks(60)?;
        d.settle_world_map()?;
        d.ui.engine.outgoing.clear();
        d.click(pos)?;
        let clicks: Vec<_> = client_frames(&d.ui.engine.outgoing)?
            .into_iter()
            .filter(|(op, _)| *op == 18)
            .collect();
        let [(_, p)] = &clicks[..] else {
            anyhow::bail!("CLICKWORLDMAP at {pos:?}: {clicks:?}")
        };
        // p4_alt3: bytes 16..23, 24..31, 0..7, 8..15.
        let v =
            i32::from(p[1]) << 24 | i32::from(p[0]) << 16 | i32::from(p[3]) << 8 | i32::from(p[2]);
        Ok([v >> 28 & 3, v >> 14 & 0x3FFF, v & 0x3FFF])
    };
    // The first press makes the map's scripts re-centre on the player; the
    // presses below start from the settled view.
    press(centre)?;
    let at_centre = press(centre)?;
    for k in [60, -90] {
        let moved = press([centre[0], centre[1] - k])?;
        let north = (f64::from(k) * 2.0 / zoom) as i32;
        assert_eq!(
            moved,
            [at_centre[0], at_centre[1], at_centre[2] + north],
            "press {k} px above centre"
        );
    }
    Ok(())
}

/// A logged-in retained UI with the world map opened from the minimap globe
/// (the dev server's answer replayed), loaded and settled: the map component's
/// canvas rectangle comes with it.
fn open_world_map() -> anyhow::Result<(Direct, [i32; 4])> {
    open_world_map_at([1280, 720])
}

fn open_world_map_at(canvas: [i32; 2]) -> anyhow::Result<(Direct, [i32; 4])> {
    let rows = crate::test_support::replay_json("world-map", "frames.json");
    let mut d = Direct::new(canvas)?;
    d.frames(&rows[0]["frames"])?;
    let globe = component::minimap::WORLD_MAP_BUTTON.packed();
    let r = component_rect(&d.ui, &|c| c.f.parentlayer == globe && c.f.id == -1)
        .filter(|r| r[2] > 0)
        .context("the minimap globe is not on screen")?;
    d.click([r[0] + r[2] / 2, r[1] + r[3] / 2])?;
    d.frames(&rows[1]["frames"])?;
    let map = component_rect(&d.ui, &|c| c.f.clientcode == crate::ui_loop::WORLD_MAP)
        .context("the world map component is not open")?;
    d.settle_world_map()?;
    d.ticks(80)?;
    d.settle_world_map()?;
    Ok((d, map))
}

impl Direct {
    /// Logic cycles until the world map has loaded and its zoom and jump
    /// animations have finished.
    fn settle_world_map(&mut self) -> anyhow::Result<()> {
        for _ in 0..600 {
            {
                let m = self.ui.engine.world_map.borrow();
                // A jump target with one axis cleared (`setzoom` clears z) is inert.
                let jumping = m.jump[0] != -1 && m.jump[1] != -1;
                if m.loading == 100 && !jumping && m.map.zoom == m.map.target_zoom {
                    return Ok(());
                }
            }
            self.ticks(1)?;
        }
        anyhow::bail!("the world map never finished loading")
    }
}

/// The ops (texts) of a component.
fn op_texts(c: &crate::ui_components::Component) -> Vec<String> {
    c.ops
        .as_ref()
        .map(|o| {
            o.iter()
                .flatten()
                .map(|t| String::from_utf16_lossy(t))
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The centre of the first visible component with the op `op` (in the cache
/// interface `iface`).
fn op_centre(d: &Direct, iface: i32, op: &str) -> anyhow::Result<[i32; 2]> {
    let mut found = None;
    walk_components(&d.ui, &mut |r, c| {
        if found.is_none() && c.f.parentlayer >> 16 == iface && op_texts(c).iter().any(|t| t == op)
        {
            found = Some([r[0] + r[2] / 2, r[1] + r[3] / 2]);
        }
    });
    found.with_context(|| format!("no visible component with op {op:?} in interface {iface}"))
}

impl Direct {
    /// A left-button drag from `from` to `to` in `steps` cycles: press, move,
    /// and (when `release`) release.
    fn drag(
        &mut self,
        from: [i32; 2],
        to: [i32; 2],
        steps: i32,
        release: bool,
    ) -> anyhow::Result<()> {
        retained_mouse_move(&mut self.ui, from, self.clock.0);
        self.ticks(1)?;
        retained_mouse_button(&mut self.ui, 0, true, self.clock.0);
        self.ticks(1)?;
        for step in 1..=steps {
            let at = [
                from[0] + (to[0] - from[0]) * step / steps,
                from[1] + (to[1] - from[1]) * step / steps,
            ];
            retained_mouse_move(&mut self.ui, at, self.clock.0);
            self.ticks(1)?;
        }
        if release {
            retained_mouse_button(&mut self.ui, 0, false, self.clock.0);
            self.ticks(3)?;
        }
        Ok(())
    }
}

/// The world map component's paint: the ops between the map's clip reset and
/// the next one (a fresh paint through the renderer's world-map owner).
fn world_map_ops(d: &mut Direct, clip: [i32; 4]) -> anyhow::Result<Vec<crate::ui_paint::Op>> {
    d.ui.target.world_map = Some(d.ui.engine.world_map.clone());
    let out = d.ui.paint(d.game.cycle, true, [0.; 3])?;
    let ops = &out.recording.ops;
    let start = ops
        .iter()
        .position(|o| matches!(o, crate::ui_paint::Op::ResetBounds(b) if *b == clip))
        .with_context(|| format!("no draw inside the clip {clip:?}"))?;
    Ok(ops[start + 1..]
        .iter()
        .take_while(|o| !matches!(o, crate::ui_paint::Op::ResetBounds(_)))
        .cloned()
        .collect())
}

/// The widths of the map images (chunk sprites and scaled chunks) in `ops`
/// that are at least 32 pixels wide, most common first.
fn image_widths(ops: &[crate::ui_paint::Op]) -> Vec<(i32, usize)> {
    let mut counts = std::collections::BTreeMap::new();
    for op in ops {
        let width = match op {
            crate::ui_paint::Op::Sprite(s, _, _) => s.size[0],
            crate::ui_paint::Op::Scaled(_, r, _) => r[2],
            _ => continue,
        };
        if width >= 32 {
            *counts.entry(width).or_insert(0usize) += 1;
        }
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    v
}

/// The zoom factor of a config zoom percentage and the chunk size it draws.
fn chunk_size(percent: i32) -> i32 {
    let factor = match percent {
        25 => 2,
        37 => 3,
        50 => 4,
        75 => 6,
        100 => 8,
        _ => 16,
    };
    factor * 64 / 2
}

/// Puts the world map's view on the display tile `(x, z)` (the CS2 jump
/// command's effect, without an animation).
fn wm_jump(d: &mut Direct, tile: (i32, i32)) {
    d.ui.engine
        .world_map
        .borrow_mut()
        .jump_to_instant(tile.0, tile.1);
}

/// Whether a map element is a link ("Open", with a target coordinate in
/// parameter 4148) to a coordinate of another map area than the surface.
fn is_area_link(
    m: &rs910_ui::world_map_client::ClientWorldMap,
    t: &rs910_scene::minimap::MapElement,
) -> bool {
    let target = t.params.iter().find_map(|(k, v)| match (*k, v) {
        (k, crate::config::ParamValue::Int(v)) if k == param::MAP_LINK_COORD.id() => Some(*v),
        _ => None,
    });
    t.ops
        .first()
        .is_some_and(|op| op.as_deref() == Some("Open"))
        && target.is_some_and(|v| {
            m.map
                .map_at(v >> 14 & 0x3FFF, v & 0x3FFF)
                .is_some_and(|i| m.map.areas[i].id != 28)
        })
}

/// The CLICKWORLDMAP presses (client packet 18) among `outgoing`.
fn map_presses(outgoing: &[u8]) -> anyhow::Result<usize> {
    Ok(client_frames(outgoing)?
        .into_iter()
        .filter(|(op, _)| *op == 18)
        .count())
}

/// Scenario 7: the world map used through its cache interface: a drag pans
/// the view, the zoom buttons step the zoom ladder and redraw the chunks at
/// the new size, the overview shows the whole area and moves the view on a
/// click, the legend rows hide and show a category (showing it flashes it),
/// an element link switches the area, and Close ends the session.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn world_map_interface_drags_zooms_filters_flashes_switches_area_and_closes() -> anyhow::Result<()>
{
    let (mut d, map) = open_world_map()?;
    let clip = [map[0], map[1], map[0] + map[2], map[1] + map[3]];
    let centre = [map[0] + map[2] / 2, map[1] + map[3] / 2];
    fn wm(d: &Direct) -> std::cell::Ref<'_, rs910_ui::world_map_client::ClientWorldMap> {
        d.ui.engine.world_map.borrow()
    }
    // Composition: the map and its chrome are overlays under the window
    // frame, which does not block clicks.
    assert_eq!(
        d.ui.state
            .life
            .subs
            .get(component::game_window::WORLD_VIEW_SLOT.packed())
            .map(|s| s.borrow().id),
        Some(interface::WORLD_MAP.id())
    );
    assert_eq!(
        d.ui.state
            .life
            .subs
            .get(component::game_window::WORLD_MAP_CONTROLS_SLOT.packed())
            .map(|s| s.borrow().id),
        Some(interface::WORLD_MAP_CONTROLS.id())
    );
    let mut blocking = 0;
    walk_components(&d.ui, &mut |r, c| {
        if c.f.noclickthrough && r[2] >= map[2] && r[3] >= map[3] {
            blocking += 1;
        }
    });
    assert_eq!(blocking, 0, "a full-size layer blocks the map");

    // Drag: 80 px left and 40 px down moves the view 2 tiles per pixel over
    // the zoom east and north, without a menu and with one press packet.
    let (start, zoom) = (wm(&d).position, f64::from(wm(&d).map.target_zoom));
    d.ui.engine.outgoing.clear();
    d.drag(centre, [centre[0] - 80, centre[1] + 40], 8, true)?;
    let expected = [
        start[0] + (80.0 * 2.0 / zoom) as i32,
        start[1] + (40.0 * 2.0 / zoom) as i32,
    ];
    assert_eq!(wm(&d).position, expected, "drag from {start:?}");
    assert!(!d.ui.state.minimenu.open, "a drag does not open the menu");
    assert_eq!(
        map_presses(&d.ui.engine.outgoing)?,
        1,
        "one CLICKWORLDMAP, at the press"
    );
    d.settle_world_map()?;

    // Zoom ladder, out and back in, with the chunks drawn at each size.
    let mut seen = Vec::new();
    for (button, percent) in [
        ("Zoom out", 50),
        ("Zoom out", 37),
        ("Zoom out", 25),
        ("Zoom in", 37),
        ("Zoom in", 50),
        ("Zoom in", 75),
        ("Zoom in", 100),
        ("Zoom in", 200),
    ] {
        let at = op_centre(&d, interface::WORLD_MAP_CONTROLS.id(), button)?;
        d.click(at)?;
        d.settle_world_map()?;
        assert_eq!(wm(&d).get_zoom(), percent, "{button}");
        let ops = world_map_ops(&mut d, clip)?;
        let widths = image_widths(&ops);
        assert_eq!(
            widths.first().map(|(w, _)| *w),
            Some(chunk_size(percent)),
            "chunks at {percent}%: {widths:?}"
        );
        seen.push(percent);
    }
    assert_eq!(seen, [50, 37, 25, 37, 50, 75, 100, 200]);
    for _ in 0..3 {
        let at = op_centre(&d, interface::WORLD_MAP_CONTROLS.id(), "Zoom out")?;
        d.click(at)?;
        d.settle_world_map()?;
    }
    assert_eq!(wm(&d).get_zoom(), 50);

    // Overview: drawn as its own component, and a click moves the view to
    // the clicked share of the area.
    let overview = component_rect(&d.ui, &|c| {
        c.f.clientcode == crate::ui_loop::WORLD_MAP_OVERVIEW
    })
    .context("no overview component")?;
    let overview_clip = [
        overview[0],
        overview[1],
        overview[0] + overview[2],
        overview[1] + overview[3],
    ];
    let ops = world_map_ops(&mut d, overview_clip)?;
    let images = ops
        .iter()
        .filter(|o| {
            matches!(
                o,
                crate::ui_paint::Op::Sprite(..) | crate::ui_paint::Op::Scaled(..)
            )
        })
        .count();
    assert!(
        images > 50,
        "the overview draws the area's chunks: {images} images"
    );
    assert!(
        ops.iter()
            .any(|o| matches!(o, crate::ui_paint::Op::Fill(_, -1_996_554_240))),
        "the overview marks the visible window"
    );
    let size = wm(&d).map.area.as_ref().context("area")?.size;
    d.click([overview[0] + overview[2] / 4, overview[1] + overview[3] / 4])?;
    let position = wm(&d).position;
    assert!(
        position[0] < size[0] / 2 && position[1] > size[1] / 2,
        "a click in the overview's upper left moved the view to {position:?} of {size:?}"
    );
    d.settle_world_map()?;

    // Legend rows: the leaf rows (script 288) toggle a category; showing a
    // hidden one flashes it. Use a category that has icons in view.
    let at = op_centre(&d, interface::WORLD_MAP_CONTROLS.id(), "Select/deselect")?;
    d.click(at)?;
    assert!(
        wm(&d).disabled_categories.is_empty(),
        "select all shows every category"
    );
    wm_jump(&mut d, (3222, 3222));
    world_map_ops(&mut d, clip)?;
    let drawn: std::collections::BTreeSet<i32> = {
        let m = wm(&d);
        let area = m.map.area.as_ref().context("area")?;
        m.containers
            .iter()
            .flatten()
            .filter_map(|c| m.visible_type(area.elements[c.element].id))
            .filter(|t| t.sprite != -1)
            .map(|t| t.category)
            .collect()
    };
    let mut rows = Vec::new();
    walk_components(&d.ui, &mut |r, c| {
        let leaf = c
            .hooks
            .get("onop")
            .and_then(|h| match (h.first(), h.get(1)) {
                (
                    Some(crate::ui_components::Arg::Int(hook)),
                    Some(crate::ui_components::Arg::Int(category)),
                ) if *hook == script::WORLD_MAP_FLASH_CATEGORY.id() => Some(*category),
                _ => None,
            });
        if let Some(category) =
            leaf.filter(|_| c.f.parentlayer >> 16 == interface::WORLD_MAP_CONTROLS.id())
        {
            rows.push(([r[0] + r[2] / 2, r[1] + r[3] / 2], category));
        }
    });
    let (row, category) = *rows
        .iter()
        .find(|(_, category)| drawn.contains(category))
        .with_context(|| {
            format!("no legend row among {rows:?} for the categories in view {drawn:?}")
        })?;
    let count = |d: &Direct| -> usize {
        let m = d.ui.engine.world_map.borrow();
        let area = m.map.area.as_ref().unwrap();
        m.containers
            .iter()
            .flatten()
            .filter(|c| {
                m.visible_type(area.elements[c.element].id)
                    .is_some_and(|t| t.category == category)
            })
            .count()
    };
    world_map_ops(&mut d, clip)?;
    let shown = count(&d);
    assert!(shown > 0, "category {category} has icons in view");
    d.click(row)?;
    assert!(
        wm(&d).disabled_categories.contains(&category),
        "the row hides category {category}"
    );
    assert!(wm(&d).flash_categories.is_empty());
    world_map_ops(&mut d, clip)?;
    assert_eq!(count(&d), 0, "the hidden category is not drawn");
    d.click(row)?;
    assert!(
        !wm(&d).disabled_categories.contains(&category),
        "the row shows category {category} again"
    );
    assert!(
        wm(&d).flash_categories.contains_key(&category),
        "and flashes it"
    );
    d.ticks(12)?;
    let ops = world_map_ops(&mut d, clip)?;
    assert_eq!(count(&d), shown, "the category is drawn again");
    // The pulse: a yellow box behind each icon, or the yellow-tinted flash
    // sprite, at a partial alpha.
    let pulse = |colour: i32| {
        colour as u32 & 0xFF_FFFF == 0xFF_FF00 && (1..0xFF).contains(&(colour as u32 >> 24))
    };
    let marks = ops
        .iter()
        .filter(|o| match o {
            crate::ui_paint::Op::Fill(_, colour) => pulse(*colour),
            crate::ui_paint::Op::Sprite(_, _, colour) => pulse(*colour),
            _ => false,
        })
        .count();
    assert!(
        marks >= shown,
        "{marks} pulse draws for {shown} flashing icons"
    );
    // The flash runs out after its loops (3 x 50 cycles).
    d.ticks(3 * 50)?;
    assert!(wm(&d).flash_categories.is_empty(), "the flash ended");

    // An element link (parameter 4148 names its target) opens another map
    // area; its op is the left-click default, so the click runs it.
    let link = {
        let m = wm(&d);
        let area = m.map.area.as_ref().context("area")?;
        area.elements
            .iter()
            .find(|e| m.visible_type(e.id).is_some_and(|t| is_area_link(&m, &t)))
            .map(|e| (e.x + area.origin[0], e.z + area.origin[1]))
            .context("no map link element on the surface")?
    };
    let before = wm(&d).map.metadata().map(|m| m.id);
    wm_jump(&mut d, link);
    world_map_ops(&mut d, clip)?;
    let at = {
        let m = wm(&d);
        let area = m.map.area.as_ref().context("area")?;
        m.containers
            .iter()
            .flatten()
            .find(|c| {
                m.visible_type(area.elements[c.element].id)
                    .is_some_and(|t| is_area_link(&m, &t))
            })
            .map(|c| {
                [
                    (c.sprite[0] + c.sprite[1]) / 2,
                    (c.sprite[2] + c.sprite[3]) / 2,
                ]
            })
            .context("the link is not drawn")?
    };
    d.click(at)?;
    d.ticks(5)?;
    d.settle_world_map()?;
    let after = wm(&d).map.metadata().map(|m| m.id);
    assert!(
        after.is_some() && after != before,
        "the link switched the area from {before:?} to {after:?}"
    );
    let ops = world_map_ops(&mut d, clip)?;
    assert!(
        image_widths(&ops).iter().map(|(_, n)| n).sum::<usize>() > 4,
        "the new area is drawn"
    );

    // Close: the chrome's own button; the server closes both overlays.
    let rows = crate::test_support::replay_json("world-map", "frames.json");
    let at = op_centre(&d, interface::WORLD_MAP_CONTROLS.id(), "Close")?;
    d.ui.engine.outgoing.clear();
    d.click(at)?;
    let wire: Vec<u8> = rows[2]["wire"]
        .as_array()
        .context("close wire")?
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let mut expected = vec![111u8];
    expected.extend(writers::p2_alt3(-1));
    expected.extend(writers::p2_alt2(-1));
    expected.extend(writers::p4(
        component::world_map_controls::CLOSE_BUTTON.packed(),
    ));
    assert_eq!(
        wire, expected,
        "the dev server accepted the recorded IF_BUTTON1"
    );
    assert!(
        d.ui.engine
            .outgoing
            .windows(wire.len())
            .any(|w| w == &wire[..]),
        "Close writes its IF_BUTTON1: {:02x?}",
        d.ui.engine.outgoing
    );
    d.frames(&rows[2]["frames"])?;
    assert_eq!(
        d.ui.state.life.subs.get(0x5c5_0016).map(|s| s.borrow().id),
        None
    );
    assert_eq!(
        d.ui.state.life.subs.get(0x5c5_0017).map(|s| s.borrow().id),
        None
    );
    Ok(())
}

/// The retained title UI (the login screen): a logged-out entity runtime
/// (`title_game`) with the defaults' `login_interface` as the top level.
fn title() -> anyhow::Result<Direct> {
    title_sized([800, 600])
}

fn title_sized(canvas: [i32; 2]) -> anyhow::Result<Direct> {
    let mut d = Direct::new(canvas)?;
    d.game = title_game(&d.pack)?;
    d.ui.engine.account.logged_in_members = false;
    let (login, _) = title_interfaces(&d.pack)?;
    let (ui, game) = (&mut d.ui, &mut d.game);
    crate::client_game::with_game(game, |v| ui.show_top_level(v, login))?;
    // The login manager's ready state (the session's `Machine::login_ready`).
    d.ui.engine.login.ready = true;
    d.ticks(5)?;
    Ok(d)
}

/// Typed text through `retained_key`, one character per logic cycle (AWT
/// VK codes: Enter 10, Tab 9, letters their capitals).
fn type_direct(d: &mut Direct, text: &str) -> anyhow::Result<()> {
    for ch in text.chars() {
        let awt = match ch {
            '\n' => 10,
            '\t' => 9,
            'a'..='z' => ch.to_ascii_uppercase() as i32,
            other => anyhow::bail!("no key for {other:?}"),
        };
        let typed = ch.to_string();
        retained_key(&mut d.ui, awt, true, Some(&typed), d.clock.0);
        retained_key(&mut d.ui, awt, false, None, d.clock.0);
        d.ticks(1)?;
    }
    d.ticks(3)
}

/// One direction of the recorded lobby exchange (`fixtures/scenarios/login`).
#[derive(Clone, Debug)]
struct Segment {
    to_server: bool,
    bytes: Vec<u8>,
}

fn login_fixture() -> std::path::PathBuf {
    rs910_core::test_support::client_dir().join("fixtures/scenarios/login/exchange.json")
}

/// The production lobby login worker (`spawn_lobby_login_worker`, the
/// `ViewerApp::request_lobby_login` owner) for `request`, against `port`.
fn lobby_login(
    ui: &crate::ui_runtime::Runtime,
    request: &crate::ui_runtime::LoginRequest,
    port: u16,
) -> anyhow::Result<crate::net::LoginOk> {
    let (tx, rx) = std::sync::mpsc::channel();
    spawn_lobby_login_worker(
        1,
        Arc::new(AtomicBool::new(false)),
        crate::net::LoginParams {
            auth: crate::net::AuthOptions {
                new_auth_preference: request.new_auth_preference.clone(),
                auth_dont_trust: request.auth_dont_trust,
                reconnect: false,
            },
            site_settings: ui.engine.login.site_settings.clone(),
            uid192: ui.engine.login.uid192,
            ..crate::net::LoginParams::new("127.0.0.1", port, &request.username, &request.password)
        },
        tx,
    );
    match rx.recv_timeout(std::time::Duration::from_secs(8))? {
        ReconnectResponse::LobbyReady { login, .. } => Ok(*login),
        ReconnectResponse::Failed { error, .. } => anyhow::bail!("lobby login failed: {error}"),
        _ => anyhow::bail!("unexpected worker response"),
    }
}

/// Regenerate `fixtures/scenarios/login/exchange.json`: the title-screen
/// login of scenario 5 against a running dev lobby
/// (`ALTO_LOBBY_PORT=35594 node ... src/lostcity/lobby.ts` with a scratch
/// `ALTO_PLAYER_DATA_DIR`), through a recording TCP proxy:
/// `CLIENT910_SCENARIO_LOBBY=35594 cargo test --lib
/// record_lobby_login_exchange -- --ignored`.
#[test]
#[ignore = "records fixtures/scenarios/login from a running dev lobby"]
fn record_lobby_login_exchange() -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let upstream: u16 = std::env::var("CLIENT910_SCENARIO_LOBBY")?.parse()?;
    let mut d = title()?;
    type_direct(&mut d, "scenario\tsecret\n")?;
    let request = d.ui.engine.login.request.clone().context("login request")?;
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let log: Arc<std::sync::Mutex<Vec<Segment>>> = Arc::default();
    let proxy_log = log.clone();
    std::thread::spawn(move || -> anyhow::Result<()> {
        let (client, _) = listener.accept()?;
        let server = std::net::TcpStream::connect(("127.0.0.1", upstream))?;
        let pump = |mut from: std::net::TcpStream,
                    mut to: std::net::TcpStream,
                    to_server: bool,
                    log: Arc<std::sync::Mutex<Vec<Segment>>>| {
            std::thread::spawn(move || {
                let mut buf = [0u8; 65536];
                while let Ok(n) = from.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut log = log.lock().unwrap();
                    match log.last_mut() {
                        Some(last) if last.to_server == to_server => last.bytes.extend(&buf[..n]),
                        _ => log.push(Segment {
                            to_server,
                            bytes: buf[..n].to_vec(),
                        }),
                    }
                    drop(log);
                    if to.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            })
        };
        pump(
            client.try_clone()?,
            server.try_clone()?,
            true,
            proxy_log.clone(),
        );
        pump(server, client, false, proxy_log);
        Ok(())
    });
    let login = lobby_login(&d.ui, &request, port)?;
    std::thread::sleep(std::time::Duration::from_millis(300));
    let segments = log.lock().unwrap().clone();
    let json: Vec<serde_json::Value> = segments
        .iter()
        .map(|s| {
            serde_json::json!({
                "to_server": s.to_server,
                "hex": s.bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            })
        })
        .collect();
    std::fs::create_dir_all(login_fixture().parent().unwrap())?;
    std::fs::write(login_fixture(), serde_json::to_string_pretty(&json)? + "\n")?;
    eprintln!("{login:?}");
    Ok(())
}

/// Bit-packet reads (g1/g2/g3s/g4s/g5/g8/gjstr2) over the recorded lobby
/// reply, in the original's field order.
struct LobbyReplyReader<'a>(&'a [u8], usize);
impl LobbyReplyReader<'_> {
    fn g(&mut self, n: usize) -> i64 {
        let v = self.0[self.1..self.1 + n]
            .iter()
            .fold(0i64, |a, &b| a << 8 | i64::from(b));
        self.1 += n;
        v
    }
    fn g1(&mut self) -> i32 {
        self.g(1) as i32
    }
    fn g2(&mut self) -> i32 {
        self.g(2) as i32
    }
    fn g3s(&mut self) -> i32 {
        (self.g(3) as i32) << 8 >> 8
    }
    fn g4s(&mut self) -> i32 {
        self.g(4) as i32
    }
    /// `gjstr2`: a 0 version byte, then a NUL-terminated CP1252 string.
    fn gjstr2(&mut self) -> String {
        assert_eq!(self.g1(), 0, "gjstr2 version");
        let end = self.0[self.1..].iter().position(|&b| b == 0).unwrap();
        let s = self.0[self.1..self.1 + end]
            .iter()
            .map(|&b| char::from(b))
            .collect();
        self.1 += end + 1;
        s
    }
}

/// Scenario 5: the title screen's cached login interface, credentials typed
/// as key events → the lobby login block on the wire → the recorded dev
/// lobby reply → the lobby profile the UI reads.
///
/// Inputs: the defaults' `login_interface` (744) and its cache scripts
/// (`lobby_enterlobby` with the typed username/password), then the
/// production lobby login worker (`spawn_lobby_login_worker`) against a
/// local server that replays `fixtures/scenarios/login/exchange.json`,
/// recorded from the dev lobby by `record_lobby_login_exchange`.
///
/// Expected, independent of the client:
/// - the login block bytes are exactly the ones the dev lobby received and
///   accepted in the recording (its reply 2), and their framing is the
///   original's: the LOBBYLOGIN packet (opcode 19, size -2), `p4(910)
///   p4(1)`, then the login block the original RSA-encrypts, sent in the
///   clear in the dev server's layout: marker `p1(10)` with the server's
///   session key echoed as `p8` and the typed password as `pjstr`;
/// - the lobby profile is the recorded reply read in the original's field
///   order by `LobbyReplyReader`.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn title_login_types_credentials_and_installs_the_lobby_reply() -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let exchange: Vec<Segment> =
        serde_json::from_slice::<Vec<serde_json::Value>>(&std::fs::read(login_fixture())?)?
            .iter()
            .map(|s| Segment {
                to_server: s["to_server"].as_bool().unwrap(),
                bytes: (0..s["hex"].as_str().unwrap().len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&s["hex"].as_str().unwrap()[i..i + 2], 16).unwrap())
                    .collect(),
            })
            .collect();
    let mut d = title()?;
    type_direct(&mut d, "scenario\tsecret\n")?;
    let request =
        d.ui.engine
            .login
            .request
            .clone()
            .context("the login script sent no request")?;
    assert_eq!(
        request,
        crate::ui_runtime::LoginRequest {
            username: "scenario".into(),
            password: "secret".into(),
            new_auth_preference: String::new(),
            auth_dont_trust: false,
            lobby: true,
            sso: None,
        }
    );
    // The dev lobby, replayed: read what the client sends, answer as recorded.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let replay = exchange.clone();
    let server = std::thread::spawn(move || -> anyhow::Result<Vec<Vec<u8>>> {
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        let mut received = Vec::new();
        for segment in &replay {
            if segment.to_server {
                let mut buf = vec![0u8; segment.bytes.len()];
                socket.read_exact(&mut buf)?;
                received.push(buf);
            } else {
                socket.write_all(&segment.bytes)?;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        Ok(received)
    });
    let login = lobby_login(&d.ui, &request, port)?;
    let received = server.join().expect("lobby thread")?;
    let recorded: Vec<Vec<u8>> = exchange
        .iter()
        .filter(|s| s.to_server)
        .map(|s| s.bytes.clone())
        .collect();
    assert_eq!(
        received, recorded,
        "the client's login bytes vs the accepted recording"
    );
    // Framing of the block (login packet layout).
    let key = &exchange.iter().find(|s| !s.to_server).unwrap().bytes[1..9];
    let block = &received[1];
    assert_eq!(block[0], 19, "LOBBYLOGIN opcode");
    assert_eq!(
        usize::from(u16::from_be_bytes([block[1], block[2]])),
        block.len() - 3
    );
    assert_eq!(
        block[3..11],
        [0, 0, 0x03, 0x8E, 0, 0, 0, 1],
        "p4(910) p4(1)"
    );
    assert_eq!(block[11], 10, "RSA block marker");
    assert!(
        block.windows(8).any(|w| w == key),
        "the server session key is echoed"
    );
    assert!(block.windows(7).any(|w| w == b"secret\0"), "pjstr password");
    // The reply (loginStep 141/157): g1 reply 2, g1 size, then the profile.
    let reply = &exchange.iter().rfind(|s| !s.to_server).unwrap().bytes;
    assert_eq!(reply[0], 2, "login reply OK");
    let mut r = LobbyReplyReader(&reply[2..2 + usize::from(reply[1])], 0);
    assert_eq!(r.g1(), 0, "no auth preferences");
    let staff = r.g1();
    let player_mod = r.g1();
    let dob_verified = r.g1() == 1;
    let lobby_dob = r.g3s();
    let _gender = r.g1();
    let quickchat = r.g1() == 1;
    let _zap = r.g1();
    let membership = r.g(8);
    let membership_delay = r.g(5);
    let member_flags = r.g1();
    let jcoins = r.g4s();
    let loyalty = r.g4s();
    let recovery_day = r.g2();
    let unread = r.g2();
    let last_login_day = r.g2();
    let host = r.g4s();
    let email = r.g1();
    let cc_expiry = r.g2();
    let grace_expiry = r.g2();
    let dob_requested = r.g1() == 1;
    let name = r.gjstr2();
    let members_stats = r.g1();
    let play_age = r.g4s();
    let world = r.g2();
    let world_host = r.gjstr2();
    let world_port = r.g2();
    let world_port2 = r.g2();
    assert_eq!(
        (
            login.profile.staff_mod_level,
            login.profile.player_mod_level,
            login.profile.dob_verified,
            login.profile.lobby_dob,
            login.profile.player_is_quickchat
        ),
        (staff, player_mod, dob_verified, lobby_dob, quickchat)
    );
    assert_eq!(
        (login.lobby.membership, login.lobby.membership_delay),
        (membership, membership_delay)
    );
    assert_eq!(login.profile.player_is_members, member_flags & 1 != 0);
    assert_eq!(
        (
            login.lobby.jcoins_balance,
            login.lobby.loyalty_balance,
            login.lobby.recovery_day,
            login.lobby.unread_messages,
            login.lobby.last_login_day,
            login.lobby.player_host,
            login.lobby.email_status
        ),
        (
            jcoins,
            loyalty,
            recovery_day,
            unread,
            last_login_day,
            host,
            email
        )
    );
    assert_eq!(
        (
            login.lobby.cc_expiry,
            login.lobby.grace_expiry,
            login.lobby.dob_requested,
            login.lobby.members_stats,
            login.lobby.play_age
        ),
        (
            cc_expiry,
            grace_expiry,
            dob_requested,
            members_stats,
            play_age
        )
    );
    assert_eq!(login.lobby.player_name, name);
    assert_eq!(name, "scenario");
    assert_eq!(
        (
            login.lobby.world_id,
            login.lobby.world_host.as_str(),
            i32::from(login.lobby.world_port),
            i32::from(login.lobby.world_port2)
        ),
        (
            Some(world as u16),
            world_host.as_str(),
            world_port,
            world_port2
        )
    );
    // `install_lobby_connection`'s profile for the lobby scripts: what the
    // CS2 `lobby_enterlobbyreply` / `userdetail_*` commands read.
    apply_lobby_profile(&mut d.ui, &login, d.clock.0);
    let e = &d.ui.engine;
    assert_eq!(
        (
            e.login.lobby_reply,
            e.account.staff_mod_level,
            e.lobby.jcoins_balance,
            e.login.lobby_player_name.as_str()
        ),
        (2, staff, jcoins, "scenario")
    );
    Ok(())
}

/// One live cycle with `frames` on the world socket, through the headless
/// `ClientCore` (`poll_live` -> `drain_game_frames` -> the retained UI), with
/// no renderer pick refresh. Fails if the live reader stopped: the session
/// logs and sets `polling_dead` instead of returning a decoder or VM error.
pub(super) fn live_step(
    replay: &mut Replay,
    cycle: &mut i32,
    frames: &[Vec<u8>],
) -> anyhow::Result<Vec<(u8, Vec<u8>)>> {
    *cycle += 1;
    let out = replay.step_live(*cycle, &frames.concat(), &mut |_, _| {})?;
    anyhow::ensure!(
        !replay.session().polling_dead,
        "cycle {cycle}: the live reader stopped"
    );
    client_frames(&out.written)
}

/// A second player standing on the local player's tile, `level` combat
/// levels above it, and a renderer pick frame whose only pickable is that
/// player: `ViewerApp`'s frame path (`install_pick_frame`) with a fixed
/// person-sized pick box instead of the drawn body. Returns a canvas point
/// that hits it.
fn other_player_under_the_mouse(
    replay: &mut Replay,
    pid: usize,
    level: i32,
) -> anyhow::Result<[i32; 2]> {
    let game = replay
        .core
        .session
        .as_mut()
        .unwrap()
        .game
        .as_mut()
        .context("game")?;
    let local = game.runtime.map.local;
    let players = &mut game.runtime.feed.state.players;
    anyhow::ensure!(players.players[pid].is_none(), "player slot {pid} in use");
    let mut other = players.players[local].clone().context("local player")?;
    other.appearance.name = Some("Other player".into());
    other.appearance.combat += level;
    other.appearance.max_combat = other.appearance.combat;
    players.players[pid] = Some(other.clone());
    let generation = players.generations[pid];
    let base = [game.runtime.map.base_x, game.runtime.map.base_z];
    let terrain = game.runtime.terrain_generation;
    let ui = ui(replay);
    let (viewport, _) = ui.state.viewport.context("scene viewport")?;
    let mut cam2 = ui.engine.camera.cam2.frame().context("cam2 frame")?;
    let anchor = cam2.lookat.map(|v| v as i32);
    cam2.rebase(anchor);
    let mut camera = crate::camera::SceneCamera::new(anchor);
    camera.cam2 = Some(cam2);
    camera.viewport = (viewport[2], viewport[3]);
    camera.viewport_profile = ui.state.viewport_profile;
    let mut frame =
        crate::player_picking::Frame::new(&camera, [base[0] << 9, base[1] << 9], terrain, viewport);
    let position = [other.fine_x, other.motion.y, other.fine_z];
    let actor = crate::actor_matrix::Matrix::actor(other.actor.rotation, position, 0.);
    let pick = crate::scene_player_pick::PickablePlayer {
        id: crate::scene_player_pick::PlayerPickId {
            pid: pid as i32,
            generation,
        },
        bounds: crate::scene_player_pick::ModelBounds {
            min: [-64, -200, -64],
            max: [64, 0, 64],
        },
        screen_bounds: None,
        projected_depth: 0,
        matrix: crate::camera::multiply(&actor.entries(), &frame.vp),
        active: true,
    };
    // The canvas point nearest the middle of the hit area.
    let mut hits = Vec::new();
    for y in (viewport[1]..viewport[1] + viewport[3]).step_by(4) {
        for x in (viewport[0]..viewport[0] + viewport[2]).step_by(4) {
            if crate::scene_player_pick::pick_one(&pick, frame.screen, [x, y], [0, 0]) {
                hits.push([x, y]);
            }
        }
    }
    anyhow::ensure!(!hits.is_empty(), "the other player is off screen");
    let n = hits.len() as i32;
    let mid = [
        hits.iter().map(|h| h[0]).sum::<i32>() / n,
        hits.iter().map(|h| h[1]).sum::<i32>() / n,
    ];
    let mouse = *hits
        .iter()
        .min_by_key(|h| (h[0] - mid[0]).pow(2) + (h[1] - mid[1]).pow(2))
        .unwrap();
    frame.picks.push(pick);
    frame.order.push(crate::player_picking::PickRef::Player(0));
    ui.engine.scene.player_picks = Some(frame);
    Ok(mouse)
}

/// Live server packets reach their consumers: scripted server frames on the
/// world socket of the recorded Lumbridge session, read by the headless
/// `ClientCore` (`poll_live` -> `drain_game_frames` -> `handle_sync_frame`
/// -> `Runtime::packet`), change the state the player sees or the bytes the
/// client writes back.
///
/// Expected, from the wire protocol and the cache:
/// - `SET_MOVEACTION` renames the ground row and sets its cursor;
///   `SHOW_FACE_HERE` adds Face here (60); `SET_PLAYER_OP` adds the
///   player's ops last slot first (action 44 + slot, +2000 when
///   deprioritised) with white-tagged names and the combat colour
///   (`ff0000` for 10 levels above). The menu update keeps actions >= 1000
///   first, the Cancel row heading `allEntries`.
/// - `REDUCE_PLAYER_ATTACK_PRIORITY`: 0 left-click Attack (44), 1 and
///   unknown ids right-click it for a higher level (2044), 2 always
///   right-click, 3 hide it. A left click with the Attack row active writes
///   OPPLAYER1 (client packet 117, `p2(index) p1_alt1(ctrl)`).
/// - `IF_MOVESUB` moves the source sub over the target's (closing it);
///   `IF_CLOSESUB` closes it.
/// - `UPDATE_STAT` split over two reads lands once complete.
/// - `SEND_PING` is answered with SEND_PING_REPLY
///   (`p4_alt1 p4_alt3 p1_alt2(fps)`); `NO_TIMEOUT` is not answered.
/// - `CAM_SHAKE`, `HINT_TRAIL`,
///   `CREATE_CHECK_NAME_REPLY` and
///   `CREATE_SUGGEST_NAME_REPLY`, once parser-only, reach the
///   camera, scene and account-creation owners.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn live_server_packets_reach_their_consumers() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    const OTHER: usize = 2;
    let mut replay = replay_to(130)?;
    let mut cycle = 130;
    ui(&mut replay).paint(cycle, true, [0.; 3])?;
    ui(&mut replay).paint(cycle, true, [0.; 3])?;
    let mouse = other_player_under_the_mouse(&mut replay, OTHER, 10)?;
    retained_mouse_move(ui(&mut replay), mouse, now());
    let local_combat = {
        let game = replay.game();
        game.runtime.feed.state.players.players[game.runtime.map.local]
            .as_ref()
            .unwrap()
            .appearance
            .combat
    };
    let other = format!(
        "<col=ffffff>Other player<col=ff0000> (level: {})",
        local_combat + 10
    );

    // Server menu state.
    let move_action = [b"Go here\0".as_slice(), &123u16.to_be_bytes()].concat();
    // SET_PLAYER_OP: p1_alt2(slot), p2_alt3(cursor, 65535 = none), pjstr,
    // p1_alt3(0 = deprioritised).
    let attack_op = [&[255u8, 127, 255][..], b"Attack\0", &[127]].concat();
    let follow_op = [&[253u8, 126, 255][..], b"Follow\0", &[128]].concat();
    let priority = |value: u8| server_frame(sp::REDUCE_PLAYER_ATTACK_PRIORITY, &[value]);
    live_step(
        &mut replay,
        &mut cycle,
        &[
            server_frame(sp::SET_MOVEACTION, &move_action),
            server_frame(sp::SHOW_FACE_HERE, &[127]),
            server_frame(sp::SET_PLAYER_OP, &attack_op),
            server_frame(sp::SET_PLAYER_OP, &follow_op),
            priority(0),
        ],
    )?;
    let rows = |replay: &mut Replay| -> Vec<(String, String, i32, i32)> {
        menu_entries(ui(replay))
            .into_iter()
            .map(|e| (e.op, e.target.unwrap_or_default(), e.action, e.cursor))
            .collect()
    };
    let row = |op: &str, target: &str, action, cursor| {
        (op.to_string(), target.to_string(), action, cursor)
    };
    assert_eq!(
        rows(&mut replay),
        [
            row("Cancel", "", 1006, -1),
            row("Follow", &other, 2046, 65534),
            row("Face here", "", 60, -1),
            row("Go here", "", 23, 123),
            // No server cursor: the second default cursor the cache
            // scripts set (`setdefaultcursors` 36, 41).
            row("Attack", &other, 44, 41),
        ],
        "menu over the other player at {mouse:?}"
    );
    let attack = |replay: &mut Replay| -> Option<i32> {
        menu_entries(ui(replay))
            .iter()
            .find(|e| e.op == "Attack")
            .map(|e| e.action)
    };
    for (value, action) in [
        (1, Some(2044)),
        (2, Some(2044)),
        (3, None),
        (255, Some(2044)),
        (0, Some(44)),
    ] {
        live_step(&mut replay, &mut cycle, &[priority(value)])?;
        assert_eq!(attack(&mut replay), action, "attack priority {value}");
    }
    // Left click: the active row is Attack.
    retained_mouse_button(ui(&mut replay), 0, true, now());
    let mut frames = live_step(&mut replay, &mut cycle, &[])?;
    retained_mouse_button(ui(&mut replay), 0, false, now());
    frames.extend(live_step(&mut replay, &mut cycle, &[])?);
    let opplayer: Vec<_> = frames.iter().filter(|(op, _)| *op == 117).collect();
    assert_eq!(
        opplayer,
        [&(117u8, vec![0, OTHER as u8, 128])],
        "all frames {frames:02x?}"
    );

    // Retained interface tree: the subs the recorded session opened on 1477.
    let sub = |replay: &mut Replay, component: i32| {
        ui(replay)
            .state
            .life
            .subs
            .get(component)
            .map(|s| s.borrow().id)
    };
    let (source, target) = (
        component::game_window::FRIENDS_CHAT_SETTINGS_SLOT.packed(),
        component::game_window::WORN_EQUIPMENT_SLOT.packed(),
    );
    assert_eq!(
        (sub(&mut replay, source), sub(&mut replay, target)),
        (
            Some(interface::FRIENDS_CHAT_SETTINGS.id()),
            Some(interface::WORN_EQUIPMENT.id())
        )
    );
    let packed = |v: i32| {
        // p4_alt2: v >> 8, v, v >> 24, v >> 16.
        let b = v.to_be_bytes();
        [b[2], b[3], b[0], b[1]]
    };
    let move_sub = [packed(source), packed(target)].concat();
    live_step(
        &mut replay,
        &mut cycle,
        &[server_frame(sp::IF_MOVESUB, &move_sub)],
    )?;
    assert_eq!(
        (sub(&mut replay, source), sub(&mut replay, target)),
        (None, Some(interface::FRIENDS_CHAT_SETTINGS.id()))
    );
    assert!(!ui(&mut replay)
        .store
        .interfaces
        .contains_key(&interface::WORN_EQUIPMENT.id()));
    live_step(
        &mut replay,
        &mut cycle,
        &[server_frame(sp::IF_CLOSESUB, &packed(target))],
    )?;
    assert_eq!(sub(&mut replay, target), None);
    assert!(!ui(&mut replay)
        .store
        .interfaces
        .contains_key(&interface::FRIENDS_CHAT_SETTINGS.id()));

    // UPDATE_STAT split over two reads: p1_alt2(level 50),
    // p4_alt1(xp 1234567), p1_alt3(skill 15).
    let stat = server_frame(
        sp::UPDATE_STAT,
        &[50u8.wrapping_neg(), 0x87, 0xD6, 0x12, 0x00, 128 - 15],
    );
    let stat_of = |replay: &Replay| -> anyhow::Result<(i32, i32)> {
        let stats = replay.game().ui_variables.stats.as_ref().context("stats")?;
        Ok((stats.stat_level(15)?, stats.stat_xp_actual(15)?))
    };
    let before = stat_of(&replay)?;
    live_step(&mut replay, &mut cycle, &[stat[..4].to_vec()])?;
    assert_eq!(stat_of(&replay)?, before, "half a frame applies nothing");
    live_step(&mut replay, &mut cycle, &[stat[4..].to_vec()])?;
    assert_eq!(stat_of(&replay)?, (50, 1_234_567));

    // `SEND_PING` is answered; the server's `NO_TIMEOUT` is not (the client's
    // own keepalive is a timer: start it afresh so none is due).
    replay.core.session.as_mut().unwrap().io.idle_connection = Default::default();
    let ping = [0x0102_0304_i32.to_be_bytes(), 0x0a0b_0c0d_i32.to_be_bytes()].concat();
    let frames = live_step(
        &mut replay,
        &mut cycle,
        &[
            server_frame(sp::SEND_PING, &ping),
            server_frame(sp::NO_TIMEOUT, &[]),
        ],
    )?;
    let reply = frames
        .iter()
        .find(|(op, _)| *op == crate::proto::client::SEND_PING_REPLY)
        .with_context(|| format!("no SEND_PING_REPLY in {frames:02x?}"))?;
    assert_eq!(reply.1[..8], [4, 3, 2, 1, 0x0b, 0x0a, 0x0d, 0x0c]);
    assert!(
        !frames.contains(&(crate::proto::client::NO_TIMEOUT, vec![])),
        "the server's NO_TIMEOUT is not answered: {frames:02x?}"
    );

    // Once parser-only packets reach their owners.
    // CAM_SHAKE: p2_alt2(speed 300), p1_alt1(channel 2), p1_alt2(jitter 10),
    // p1_alt3(scale 20), p1(cycle 30).
    let shake = [
        1,
        300u16.wrapping_add(128) as u8,
        2 + 128,
        10u8.wrapping_neg(),
        128 - 20,
        30,
    ];
    // HINT_TRAIL: p1(slot 1), psmart2or4(model 1234), psmart1or2s(2 points),
    // p2(x 3222), p2(z 3223), then (dx, dz) steps.
    let trail = [1, 0x04, 0xD2, 64 + 2, 0x0C, 0x96, 0x0C, 0x97, 1, 0, 0, 1];
    live_step(
        &mut replay,
        &mut cycle,
        &[
            server_frame(sp::CAM_SHAKE, &shake),
            server_frame(sp::HINT_TRAIL, &trail),
            server_frame(sp::CREATE_CHECK_NAME_REPLY, &[5]),
            server_frame(sp::CREATE_SUGGEST_NAME_REPLY, b"Rune\0"),
        ],
    )?;
    let engine = &ui(&mut replay).engine;
    let m = engine.camera.cam2.legacy.modifiers[2];
    assert_eq!(
        (m.enabled, m.jitter, m.wobble_scale, m.wobble_speed),
        (true, 10, 20, 300)
    );
    let trail = engine.scene.hint_trails[1]
        .as_ref()
        .context("hint trail 1")?;
    assert_eq!(
        (trail.model, trail.points.clone()),
        (1234, vec![[3223, 3223], [3223, 3224]])
    );
    assert_eq!(engine.creation.name_reply, 5);
    assert_eq!(engine.creation.suggested_name.as_deref(), Some("Rune"));
    Ok(())
}

/// Scenario: the server takes the camera for a cutscene (CAM_MOVETO and
/// CAM_LOOKAT put it in the explicit-pose camera state), and a click on the
/// ground in the scene viewport still offers "Walk here" and writes the walk
/// packet for the tile that was drawn under the mouse. The original client
/// builds its scene options from the last draw's matrices whatever the camera
/// mode, so a cutscene does not turn picking off.
///
/// The drawn view is installed the way `ViewerApp` does for this state: the
/// pose the legacy camera retained, the frame camera, the pick frame. The
/// mouse sits on the pixel the chosen ground tile is drawn at.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn click_on_the_ground_under_the_cutscene_camera_writes_the_walk_packet() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let mut replay = replay_to(130)?;
    let mut cycle = 130;
    ui(&mut replay).paint(cycle, true, [0.; 3])?;
    ui(&mut replay).paint(cycle, true, [0.; 3])?;
    let (player_tile, base, size) = {
        let game = replay.core.session.as_ref().unwrap().game.as_ref().unwrap();
        let map = &game.runtime.map;
        let player = game.runtime.feed.state.players.players[map.local]
            .as_ref()
            .context("local player")?;
        (
            [player.fine_x as i32 >> 9, player.fine_z as i32 >> 9],
            [map.base_x, map.base_z],
            [map.width, map.height],
        )
    };
    // CAM_MOVETO (x, z, height, acceleration, speed) and CAM_LOOKAT, both
    // instant (speed 100): the eye sits seven tiles south-west of the player
    // at 500 height units, looking at the player's tile.
    let eye = [player_tile[0] - 4, player_tile[1] - 7];
    let (acceleration, speed, height) = (50u8, 100u8, 500u16);
    let move_to = [
        (eye[0] as u8).wrapping_neg(),
        acceleration.wrapping_add(128),
        eye[1] as u8,
        128u8.wrapping_sub(speed),
        (height >> 8) as u8,
        height as u8,
    ];
    let look_at = [
        (player_tile[0] as u8).wrapping_neg(),
        player_tile[1] as u8,
        128u8.wrapping_sub(speed),
        acceleration.wrapping_neg(),
        0,
        128,
    ];
    live_step(
        &mut replay,
        &mut cycle,
        &[
            server_frame(sp::CAM_MOVETO, &move_to),
            server_frame(sp::CAM_LOOKAT, &look_at),
        ],
    )?;
    live_step(&mut replay, &mut cycle, &[])?;
    assert_eq!(
        ui(&mut replay).engine.camera.cam2.camera_state,
        5,
        "the cutscene camera state"
    );
    // The redraw's frame camera for this state.
    let ui_rt = &mut replay.core.session.as_mut().unwrap().ui;
    let (viewport, zoom) = ui_rt.state.viewport.context("scene viewport")?;
    let drawn = ui_rt
        .engine
        .camera
        .cam2
        .legacy
        .drawn(size, &mut crate::ui_cam2::random_unit);
    let mut view = ViewCamera {
        camera: OrbitCamera::new(glam::Vec3::ZERO),
        follow: true,
        last_redraw: None,
        camera_unready: false,
    };
    let base_fine = [base[0] << 9, base[1] << 9];
    view.set_legacy_frame(base_fine, drawn);
    view.camera.zoom = Some(zoom);
    view.camera.scene_viewport = Some(viewport);
    let camera = view
        .camera
        .scene_camera((viewport[2] as u32, viewport[3] as u32));
    let frame = crate::player_picking::Frame::new(&camera, base_fine, 1, viewport);
    ui_rt.engine.scene.drawn_view = Some(crate::ui_scene_options::DrawnView::of_frame(&frame));
    // The ground tile two east and one south-ish of the player, as drawn.
    let tile = [player_tile[0] + 2, player_tile[1] + 1];
    let heightmap = ui_rt
        .engine
        .camera
        .cam2
        .scene
        .heightmap
        .as_ref()
        .context("heightmap")?;
    let ground = heightmap.heightmap_y(tile[0] * 512 + 256, tile[1] * 512 + 256, 0);
    let clip = crate::ui_scene_options::transform(
        &frame.vp,
        (tile[0] * 512 + 256) as f32,
        ground as f32,
        (tile[1] * 512 + 256) as f32,
    );
    anyhow::ensure!(clip[3] > 0.0, "the tile is behind the camera");
    let mouse = [
        (frame.screen[0] + frame.screen[2] * clip[0] / clip[3]) as i32,
        (frame.screen[1] + frame.screen[3] * clip[1] / clip[3]) as i32,
    ];
    anyhow::ensure!(
        mouse[0] > viewport[0]
            && mouse[1] > viewport[1]
            && mouse[0] < viewport[0] + viewport[2]
            && mouse[1] < viewport[1] + viewport[3],
        "tile drawn off the viewport at {mouse:?}"
    );
    retained_mouse_move(ui(&mut replay), mouse, now());
    live_step(&mut replay, &mut cycle, &[])?;
    let walk = menu(&mut replay)
        .into_iter()
        .find(|(_, _, action)| *action == 23);
    assert_eq!(
        walk,
        Some(("Walk here".to_string(), String::new(), 23)),
        "the menu under the cutscene camera at {mouse:?}: {:?}",
        menu(&mut replay)
    );
    let picked = ui(&mut replay)
        .input
        .scene_options
        .iter()
        .find(|o| o.action == 23)
        .map(|o| o.tile);
    assert_eq!(picked, Some(tile), "the tile under {mouse:?}");
    // A left click on the scene takes the active option; Walk here is the
    // only ground option, so it is the left-click action.
    retained_mouse_button(ui(&mut replay), 0, true, now());
    let mut frames = written(&mut replay, &mut cycle, 1)?;
    retained_mouse_button(ui(&mut replay), 0, false, now());
    frames.extend(written(&mut replay, &mut cycle, 2)?);
    let walks: Vec<_> = frames
        .iter()
        .filter(|(op, _)| *op == crate::proto::client::MOVE_GAMECLICK)
        .cloned()
        .collect();
    let mut expected = writers::p2(base[1] + tile[1]).to_vec();
    expected.push(0);
    expected.extend(writers::p2_alt3(base[0] + tile[0]));
    assert_eq!(
        walks,
        [(crate::proto::client::MOVE_GAMECLICK, expected)],
        "MOVE_GAMECLICK on the socket; all frames {frames:02x?}"
    );
    Ok(())
}

/// The lobby screen the server's `LoginLayout.ts` opens: top interface 906
/// and its nine sub-interfaces.
fn lobby_screen() -> anyhow::Result<Direct> {
    let mut d = title_sized([1280, 720])?;
    use component::lobby_window as lobby;
    let subs = [
        (lobby::PLAYER_INFO_SLOT, interface::LOBBY_PLAYER_INFO),
        (lobby::WORLD_SELECT_SLOT, interface::LOBBY_WORLD_SELECT),
        (lobby::FRIENDS_SLOT, interface::LOBBY_FRIENDS),
        (lobby::FRIENDS_CHAT_SLOT, interface::LOBBY_FRIENDS_CHAT),
        (lobby::CLAN_CHAT_SLOT, interface::LOBBY_CLAN_CHAT),
        (lobby::OPTIONS_SLOT, interface::LOBBY_OPTIONS),
        (lobby::REPORT_ABUSE_SLOT, interface::LOBBY_REPORT_ABUSE),
        (
            lobby::REPORT_ABUSE_DETAILS_SLOT,
            interface::LOBBY_REPORT_ABUSE_DETAILS,
        ),
        (lobby::REPORT_IGNORE_SLOT, interface::LOBBY_REPORT_IGNORE),
    ];
    {
        let (ui, game) = (&mut d.ui, &mut d.game);
        crate::client_game::with_game(game, |v| {
            ui.packet(
                v,
                &crate::session::UiEvent::OpenTop {
                    interface_id: interface::LOBBY_WINDOW.id() as u32,
                    keys: [0; 4],
                },
            )?;
            for (slot, sub) in subs {
                ui.packet(
                    v,
                    &crate::session::UiEvent::OpenSub {
                        parent_packed: slot.packed() as u32,
                        sub_id: sub.id() as u32,
                        kind: 1,
                        keys: [0; 4],
                    },
                )?;
            }
            Ok(())
        })?;
    }
    d.ticks(5)?;
    Ok(d)
}

/// The lobby's popup (906:236, hidden until a script shows it) holds two
/// fixed-model components. Shown through the ordinary draw walk, both reach
/// the model owner and come out as two model draws inside the canvas.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn lobby_popup_models_draw_through_the_interface_walk() -> anyhow::Result<()> {
    let mut d = lobby_screen()?;
    let popup =
        d.ui.store
            .get(component::lobby_window::POPUP_LAYER.packed(), -1)?
            .context("popup layer")?;
    popup.borrow_mut().f.hide = false;
    d.ui.paint(d.game.cycle, true, [0.; 3])?;
    let output = d.ui.paint(d.game.cycle, true, [0.; 3])?;
    assert!(d.ui.target.missing.is_empty(), "{:?}", d.ui.target.missing);
    assert_eq!(output.models.len(), 2, "calls {:?}", d.ui.target.calls);
    for m in &output.models {
        assert!(m.model.vertex_count > 0);
        assert!(m.clip[0] >= 0 && m.clip[2] <= 1280 && m.clip[1] >= 0 && m.clip[3] <= 720);
        assert!(m.depth_write && m.draw_particles);
    }
    // Two different models.
    assert_ne!(
        output.models[0].model.position_stream(),
        output.models[1].model.position_stream()
    );
    Ok(())
}

/// The lobby popup's two cache models reach pixels: the same frame with the
/// models switched off differs inside the popup. Renders through the GPU
/// capture (pre-release, `--ignored`); the PNGs land in the proof directory.
#[test]
#[ignore = "gpu: renders through ui_model_gpu::Capture (desktop GPU); run with --ignored"]
fn lobby_popup_models_reach_pixels() -> anyhow::Result<()> {
    let mut d = lobby_screen()?;
    let popup =
        d.ui.store
            .get(component::lobby_window::POPUP_LAYER.packed(), -1)?
            .context("popup layer")?;
    popup.borrow_mut().f.hide = false;
    let path = crate::test_support::proof_dir("lobby-popup-models");
    let mut capture = crate::ui_model_gpu::Capture::new([1280, 720])?;
    d.ui.paint(d.game.cycle, true, [0.05; 3])?;
    let output = d.ui.paint(d.game.cycle, true, [0.05; 3])?;
    anyhow::ensure!(output.models.len() == 2, "{} models", output.models.len());
    let with = capture.render(output, &path.join("lobby-popup-models.png"))?;
    // The same frame without the models.
    let models: Vec<_> = {
        let iface = d.ui.store.interfaces[&interface::LOBBY_WINDOW.id()].borrow();
        let comps: Vec<_> = iface
            .components
            .borrow()
            .iter()
            .flatten()
            .filter(|c| c.borrow().f.r#type == 6 && c.borrow().f.model >= 0)
            .cloned()
            .collect();
        comps
    };
    for c in &models {
        c.borrow_mut().f.model = -1;
    }
    d.ui.paint(d.game.cycle, true, [0.05; 3])?;
    let output = d.ui.paint(d.game.cycle, true, [0.05; 3])?;
    anyhow::ensure!(output.models.is_empty());
    let without = capture.render(output, &path.join("lobby-popup-without-models.png"))?;
    let changed = with
        .chunks_exact(4)
        .zip(without.chunks_exact(4))
        .filter(|(a, b)| a[..3] != b[..3])
        .count();
    anyhow::ensure!(changed > 500, "only {changed} pixels belong to the models");
    eprintln!("lobby popup models cover {changed} pixels; proof in {path:?}");
    Ok(())
}

/// Interfaces of the cache whose fixed-model components (the default model
/// kind) are visible as soon as the interface opens, mounted in the world's
/// modal slot: every model reaches the model owner, and switching them off
/// changes pixels. Renders through the GPU capture (pre-release, `--ignored`).
#[test]
#[ignore = "gpu: renders through ui_model_gpu::Capture (desktop GPU); run with --ignored"]
fn cache_model_interfaces_reach_pixels() -> anyhow::Result<()> {
    let mut d = title_sized([1280, 720])?;
    {
        let (ui, game) = (&mut d.ui, &mut d.game);
        crate::client_game::with_game(game, |v| {
            ui.packet(
                v,
                &crate::session::UiEvent::OpenTop {
                    interface_id: interface::GAME_WINDOW.id() as u32,
                    keys: [0; 4],
                },
            )
        })?;
    }
    d.ticks(3)?;
    let path = crate::test_support::proof_dir("cache-model-interfaces");
    let mut capture = crate::ui_model_gpu::Capture::new([1280, 720])?;
    let parent = component::game_window::MODAL_FRAME_SLOT.packed() as u32;
    for id in [4u32, 9, 44, 52, 80] {
        crate::client_game::with_game(&mut d.game, |v| {
            d.ui.packet(
                v,
                &crate::session::UiEvent::OpenSub {
                    parent_packed: parent,
                    sub_id: id,
                    kind: 0,
                    keys: [0; 4],
                },
            )
        })?;
        d.ticks(2)?;
        d.ui.paint(d.game.cycle, true, [0.05; 3])?;
        let output = d.ui.paint(d.game.cycle, true, [0.05; 3])?;
        anyhow::ensure!(!output.models.is_empty(), "interface {id} drew no models");
        let count = output.models.len();
        let with = capture.render(output, &path.join(format!("interface-{id}.png")))?;
        let comps: Vec<_> = d.ui.store.interfaces[&(id as i32)]
            .borrow()
            .components
            .borrow()
            .iter()
            .flatten()
            .filter(|c| c.borrow().f.r#type == 6 && c.borrow().f.model >= 0)
            .cloned()
            .collect();
        let saved: Vec<_> = comps.iter().map(|c| c.borrow().f.model).collect();
        for c in &comps {
            c.borrow_mut().f.model = -1;
        }
        d.ui.paint(d.game.cycle, true, [0.05; 3])?;
        let output = d.ui.paint(d.game.cycle, true, [0.05; 3])?;
        let without = capture.render(output, &path.join(format!("interface-{id}-without.png")))?;
        for (c, model) in comps.iter().zip(saved) {
            c.borrow_mut().f.model = model;
        }
        let changed = with
            .chunks_exact(4)
            .zip(without.chunks_exact(4))
            .filter(|(a, b)| a[..3] != b[..3])
            .count();
        anyhow::ensure!(
            changed > 300,
            "interface {id}: {count} models, {changed} pixels"
        );
        eprintln!("interface {id}: {count} models cover {changed} pixels");
        crate::client_game::with_game(&mut d.game, |v| {
            d.ui.packet(
                v,
                &crate::session::UiEvent::CloseSub {
                    parent_packed: parent,
                },
            )
        })?;
    }
    eprintln!("proof in {path:?}");
    Ok(())
}
