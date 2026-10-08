//! Slice W1 (woodcutting), client side: a recorded dev-server session in
//! which the real client chops the Lumbridge tree, replayed through the
//! production owners (`session_replay`), with the renderer's headless pick
//! frame (`scenario_tests::install_pick_frame`) while the mouse is on the
//! tree.
//!
//! Fixture: `fixtures/session-replay/woodcutting/` (`record.sh`; the release
//! client with `CLIENT910_RECORD` against the dev lobby/world with the
//! TEST-ONLY `ALTO_WC_ALWAYS=1`, so the first woodcutting roll succeeds).
use super::scenario_tests::{
    component_rect, hit_point, install_pick_frame, menu, pick_step, ui, writers,
};
use super::session_replay::{
    arrivals, client_frames, mask_wall_clock, observe, server_trace_file, Replay, Trace,
};
use super::*;
use native910::script::{CompiledScript, Counts, Instruction, Operand};
use rs910_symbols::{component, interface, inv, loc, obj, seq, varc};

const FIXTURE: &str = "fixtures/session-replay/woodcutting";
/// `record.sh`: the mouse on the trunk from HOVER_CYCLE, the left click at
/// CLICK_CYCLE (`CLIENT910_UI_HOVER` / `CLIENT910_UI_CLICKS`).
const HOVER_CYCLE: i32 = 700;
const CLICK_CYCLE: i32 = 750;
const CLICK: [i32; 2] = [447, 97];

/// Cache facts (the server's own loc, enum and seq decoders, spec §1).
const TREE: i32 = loc::TREE.id();
const TREE_TILE: [i32; 3] = [3228, 3228, 0]; // shape 10, angle 3
const STUMP: i32 = loc::TREE_STUMP.id();
const CANOPY: i32 = loc::TREE_CANOPY.id();
const CANOPY_TILE: [i32; 3] = [3227, 3227, 1];
const BRONZE_HATCHET: i32 = obj::BRONZE_HATCHET.id();
const LOGS: i32 = obj::LOGS.id();
const CHOP_SEQ: i32 = seq::CHOP_BRONZE_HATCHET.id();
const WOODCUTTING: i32 = 8;
const BACKPACK_INV: i32 = inv::BACKPACK.id();
/// Server content texts (Woodcutting.ts MESSAGES; unverified against 910).
const SWING: &str = "You swing your hatchet at the tree.";
const SUCCESS: &str = "You get some logs.";

/// Packet reads (g1, g1_alt2, g1_alt3, g2, g2_alt2, g4s, g4_alt1,
/// gSmart1or2, gjstr) for locating the recorded server frames.
pub(super) struct FrameReader<'a>(pub(super) &'a [u8], pub(super) usize);
impl FrameReader<'_> {
    pub(super) fn g1(&mut self) -> i32 {
        self.1 += 1;
        i32::from(self.0[self.1 - 1])
    }
    pub(super) fn g1_alt2(&mut self) -> i32 {
        (-self.g1()) & 0xFF
    }
    pub(super) fn g1_alt3(&mut self) -> i32 {
        (128 - self.g1()) & 0xFF
    }
    pub(super) fn g2(&mut self) -> i32 {
        self.g1() << 8 | self.g1()
    }
    pub(super) fn g2_alt2(&mut self) -> i32 {
        let hi = self.g1();
        hi << 8 | ((self.g1() - 128) & 0xFF)
    }
    pub(super) fn g4s(&mut self) -> i32 {
        self.g2() << 16 | self.g2()
    }
    pub(super) fn g4_alt1(&mut self) -> i32 {
        self.g1() | self.g1() << 8 | self.g1() << 16 | self.g1() << 24
    }
    pub(super) fn skip(&mut self, n: usize) {
        self.1 += n;
    }
    pub(super) fn g_smart1or2(&mut self) -> i32 {
        if self.0[self.1] < 128 {
            self.g1()
        } else {
            self.g2() - 32768
        }
    }
    pub(super) fn gjstr(&mut self) -> String {
        let end = self.0[self.1..].iter().position(|&b| b == 0).unwrap();
        let s = self.0[self.1..self.1 + end]
            .iter()
            .map(|&b| char::from(b))
            .collect();
        self.1 += end + 1;
        s
    }
}

/// `MESSAGE_GAME`: type, g4s, flags, names, text.
pub(super) fn message_game(payload: &[u8]) -> (i32, String) {
    let mut r = FrameReader(payload, 0);
    let kind = r.g_smart1or2();
    r.g4s();
    let flags = r.g1();
    if flags & 1 != 0 {
        r.gjstr();
        if flags & 2 != 0 {
            r.gjstr();
        }
    }
    (kind, r.gjstr())
}

/// `UPDATE_STAT`: `(stat, xp, level)`.
pub(super) fn update_stat(payload: &[u8]) -> [i32; 3] {
    let mut r = FrameReader(payload, 0);
    let level = r.g1_alt2();
    let xp = r.g4_alt1();
    [r.g1_alt3(), xp, level]
}

/// `UPDATE_INV_PARTIAL` without var blocks:
/// `(inv, [(slot, obj, count)])`.
pub(super) fn update_inv_partial(payload: &[u8]) -> (i32, Vec<[i32; 3]>) {
    let mut r = FrameReader(payload, 0);
    let inv = r.g2();
    assert_eq!(r.g1() & 2, 0, "no var blocks in this session");
    let mut slots = Vec::new();
    while r.1 < payload.len() {
        let slot = r.g_smart1or2();
        let obj = r.g2() - 1;
        let mut count = 0;
        if obj >= 0 {
            count = r.g1();
            if count == 255 {
                count = r.g4s();
            }
        }
        slots.push([slot, obj, count]);
    }
    (inv, slots)
}

/// `IF_OPENSUB`: `(parent, id, type)`.
pub(super) fn if_opensub(payload: &[u8]) -> [i32; 3] {
    let mut r = FrameReader(payload, 0);
    r.skip(4);
    let parent = r.g4_alt1();
    let kind = r.g1_alt2();
    r.skip(4);
    [parent, r.g2(), kind]
}

/// `CLIENT_SETVARC_LARGE`: `(varc, value)`.
pub(super) fn setvarc_large(payload: &[u8]) -> [i32; 2] {
    let mut r = FrameReader(payload, 0);
    let value = r.g4_alt1();
    [r.g2_alt2(), value]
}

/// The facing angle from `from` towards `to`.
fn facing_towards(from: [i32; 2], to: [i32; 2]) -> i32 {
    ((f64::from(from[0] - to[0]).atan2(f64::from(from[1] - to[1])) * 2_607.594_587_617_613_3)
        as i32)
        & 0x3FFF
}

/// Run a CS2 script of `(command, operand)` pairs through the retained
/// runtime's own VM, command dispatch and game variable domain (the
/// `Runner` every hook uses), returning its int stack.
pub(super) fn cs2_ints(replay: &mut Replay, code: &[(&str, i32)]) -> anyhow::Result<Vec<i32>> {
    let script = CompiledScript {
        name: Some("scenario:woodcutting".into()),
        locals: Counts::default(),
        args: Counts::default(),
        code: code
            .iter()
            .map(|&(command, v)| Instruction {
                opcode: 0,
                command: command.into(),
                operand: if command == "push_constant_int" {
                    Operand::Int(v)
                } else {
                    Operand::Byte(0)
                },
            })
            .chain([Instruction {
                opcode: 0,
                command: "return".into(),
                operand: Operand::Byte(0),
            }])
            .collect(),
    };
    let session = replay.core.session.as_mut().context("session")?;
    let rt = &mut session.ui;
    let game = session.game.as_mut().context("game")?;
    crate::client_game::with_game(game, |vars| {
        let provider = crate::ui_scripts::Provider {
            scripts: &rt.scripts,
            definitions: vars.definitions,
        };
        let mut runner = crate::ui_hook_host::Runner {
            pool: &mut rt.pool,
            provider: &provider,
            engine: &mut rt.engine,
            domains: crate::ui_hook_host::Domains::Game(vars),
            executions: vec![],
            missing: vec![],
        };
        runner.run_compiled(&mut rt.store, &mut rt.state, -1, &script, 1000)?;
        let execution = runner.executions.pop().context("execution")?;
        execution
            .result
            .map_err(|e| anyhow::anyhow!("scenario script: {e:?}"))?;
        Ok(execution.snapshot.ints)
    })
}

/// Every component of every loaded interface, dynamic children included.
pub(super) fn all_components(rt: &crate::ui_runtime::Runtime) -> Vec<crate::ui_components::Ref> {
    let mut out = Vec::new();
    let mut arrays: Vec<crate::ui_components::Array> = rt
        .store
        .interfaces
        .values()
        .map(|i| i.borrow().components.clone())
        .collect();
    while let Some(array) = arrays.pop() {
        for c in array.borrow().iter().flatten() {
            if let Some(children) = &c.borrow().children {
                arrays.push(children.clone());
            }
            out.push(c.clone());
        }
    }
    out
}

/// The texts of the components whose `onstattransmit` hook is `script` and
/// whose transmit list holds `stat` (the skills tab rows 8489 builds).
fn stat_hook_texts(rt: &crate::ui_runtime::Runtime, script: i32, stat: i32) -> Vec<String> {
    use crate::ui_components::Arg;
    all_components(rt)
        .into_iter()
        .filter_map(|c| {
            let c = c.borrow();
            let hooked = matches!(c.hooks.get("onstattransmit").and_then(|h| h.first()), Some(Arg::Int(id)) if *id == script);
            let listed = c
                .transmits
                .get("onstattransmitlist")
                .is_some_and(|l| l.contains(&stat));
            (hooked && listed).then(|| String::from_utf16_lossy(c.f.text.as_deref().unwrap_or(&[])))
        })
        .collect()
}

/// The zone snapshot's loc-change request at an absolute tile, layer 2
/// (the loc-change request, the scene's entity layer for shape 10):
/// `(id, shape, angle)`.
pub(super) fn loc_request(
    game: &crate::client_game::ClientGame,
    tile: [i32; 3],
) -> Option<(i32, i32, i32)> {
    let base = [game.runtime.map.base_x, game.runtime.map.base_z];
    game.runtime
        .feed
        .state
        .zones
        .locations
        .iter()
        .find(|r| {
            (r.level, r.layer, r.x, r.z) == (tile[2], 2, tile[0] - base[0], tile[1] - base[1])
        })
        .map(|r| (r.id, r.shape, r.angle))
}

pub(super) fn chat(rt: &crate::ui_runtime::Runtime) -> Vec<(i32, String)> {
    (0..=rt.engine.messages.history.last_uid())
        .filter_map(|uid| rt.engine.messages.history.get_by_uid(uid))
        .map(|l| (l.chat_type, l.message.clone()))
        .collect()
}

/// Stand-in for the renderer's loc-slot swap: the loc replacement puts the
/// request's loc in the slot, which the app draws as a
/// `TemporaryPick::Loc` labelled with the request's id/shape/angle
/// (app.rs transient locs) after `apply_location_changes` empties the static
/// slot through `SlotMeshes` (GPU-only, not run headless). The static pick
/// on a changed layer-2 slot takes the request's label here; removals
/// (id -1) keep their pick (only the level-1 canopy is removed, and a
/// level-0 player's menu never lists level-1 picks).
fn relabel_changed_locs(replay: &mut Replay) -> anyhow::Result<()> {
    let requests: Vec<(i32, i32, i32, i32, i32, i32)> = replay
        .game()
        .runtime
        .feed
        .state
        .zones
        .locations
        .iter()
        .filter(|r| r.layer == 2 && r.id >= 0)
        .map(|r| (r.level, r.x, r.z, r.id, r.shape, r.angle))
        .collect();
    let rt = ui(replay);
    let frame = rt
        .engine
        .scene
        .player_picks
        .as_mut()
        .context("pick frame")?;
    for pick in &mut frame.loc_picks {
        // Loc shape layers: 9..=21 are the entity layer 2.
        if !(9..=21).contains(&pick.shape) {
            continue;
        }
        if let Some(&(.., id, shape, angle)) = requests
            .iter()
            .find(|r| (r.0, r.1, r.2) == (pick.level, pick.tile[0], pick.tile[1]))
        {
            (pick.id, pick.shape, pick.angle) = (id, shape, angle);
        }
    }
    Ok(())
}

/// The menu with the mouse on the (relabelled) loc `id` on `tile` in a fresh
/// pick frame, past the recording (cycles >= 100000 carry no injected input).
pub(super) fn menu_on_loc(
    replay: &mut Replay,
    cycle: &mut i32,
    shown: i32,
) -> anyhow::Result<Vec<(String, String, i32)>> {
    ui(replay).paint(*cycle, true, [0.; 3])?;
    install_pick_frame(replay)?;
    relabel_changed_locs(replay)?;
    let mouse = hit_point(replay, shown)?;
    retained_mouse_move(ui(replay), mouse, crate::logic_clock::monotonic_millis());
    for _ in 0..2 {
        *cycle += 1;
        relabel_changed_locs(replay)?;
        pick_step(replay, *cycle, &[])?;
    }
    Ok(menu(replay))
}

/// Located server frames of the recording (decoders above).
struct Events {
    swing: usize,
    success: usize,
    logs: usize,
    setxp: usize,
    stat: usize,
    popup: usize,
    varc: usize,
    fell: usize,
    canopy_del: usize,
    respawn: usize,
}

fn events(arrivals: &[super::session_replay::Arrival]) -> anyhow::Result<Events> {
    use crate::proto::server as sp;
    let find = |what: &str, pred: &dyn Fn(&super::session_replay::Arrival) -> bool| {
        arrivals
            .iter()
            .position(pred)
            .with_context(|| format!("{what} not in the fixture"))
    };
    let fell = find("stump", &|a| a.opcode == sp::LOC_ADD_CHANGE)?;
    let e = Events {
        swing: find("swing message", &|a| {
            a.opcode == sp::MESSAGE_GAME && message_game(&a.payload) == (0, SWING.into())
        })?,
        success: find("success message", &|a| {
            a.opcode == sp::MESSAGE_GAME && message_game(&a.payload) == (0, SUCCESS.into())
        })?,
        logs: find("backpack logs", &|a| {
            a.opcode == sp::UPDATE_INV_PARTIAL
                && update_inv_partial(&a.payload) == (BACKPACK_INV, vec![[1, LOGS, 1]])
        })?,
        setxp: find("setxp stat", &|a| {
            a.opcode == sp::UPDATE_STAT && update_stat(&a.payload) == [WOODCUTTING, 58, 1]
        })?,
        stat: find("woodcutting stat", &|a| {
            a.opcode == sp::UPDATE_STAT && update_stat(&a.payload) == [WOODCUTTING, 83, 2]
        })?,
        popup: find("level-up popup", &|a| {
            a.opcode == sp::IF_OPENSUB && if_opensub(&a.payload)[1] == interface::LEVELUP_POPUP.id()
        })?,
        varc: find("varc 5188", &|a| {
            a.opcode == sp::CLIENT_SETVARC_LARGE
                && setvarc_large(&a.payload)[0] == varc::LEVELUP_POPUP.id()
        })?,
        fell,
        canopy_del: find("canopy removal", &|a| a.opcode == sp::LOC_DEL)?,
        respawn: fell
            + 1
            + arrivals[fell + 1..]
                .iter()
                .position(|a| a.opcode == sp::LOC_ADD_CHANGE)
                .context("respawn")?,
    };
    // The server's side of the contract, decoded as the original does.
    assert_eq!(
        if_opensub(&arrivals[e.popup].payload),
        [
            component::game_window::LEVELUP_SLOT.packed(),
            interface::LEVELUP_POPUP.id(),
            1,
        ],
        "the server's popup mount"
    );
    assert_eq!(
        setvarc_large(&arrivals[e.varc].payload),
        [varc::LEVELUP_POPUP.id(), WOODCUTTING | 2 << 23]
    );
    assert!(
        e.setxp < e.swing
            && e.swing < e.logs
            && e.logs < e.fell
            && e.fell < e.canopy_del
            && e.canopy_del < e.respawn,
        "server event order"
    );
    Ok(e)
}

/// One recorded cycle through the production owners, with the renderer's
/// pick frame while the mouse is on the tree (ViewerApp's `refresh_picks`
/// loc re-test), checking the exact outgoing bytes.
fn replay_cycle(replay: &mut Replay, trace: &Trace, cycle: i32) -> anyhow::Result<Vec<u8>> {
    let written = replay_cycle_output(replay, trace, cycle)?;
    assert_eq!(
        mask_wall_clock(&written)?,
        mask_wall_clock(&trace.bytes(b"OUT ", cycle))?,
        "cycle {cycle}: client packets differ from the recording"
    );
    Ok(written)
}

/// [`replay_cycle`] without the comparison: the bytes the client writes.
pub(super) fn replay_cycle_output(
    replay: &mut Replay,
    trace: &Trace,
    cycle: i32,
) -> anyhow::Result<Vec<u8>> {
    let out = if (HOVER_CYCLE..=CLICK_CYCLE + 1).contains(&cycle) {
        ui(replay).paint(cycle, true, [0.; 3])?;
        install_pick_frame(replay)?;
        let mut scene = replay.assets.scene_graph.take().context("scene graph")?;
        let out = replay.cycle_with_picks(trace, cycle, &mut |_, rt| {
            let mouse = rt.input.click.unwrap_or(rt.engine.platform.mouse);
            if let Some(frame) = rt.engine.scene.player_picks.as_mut() {
                frame.refresh_locs(&mut scene, mouse);
            }
        });
        replay.assets.scene_graph = Some(scene);
        out?
    } else {
        replay.cycle(trace, cycle)?
    };
    Ok(out.written)
}

fn fixture() -> anyhow::Result<(PathBuf, Trace)> {
    let root = rs910_core::test_support::client_dir().join(FIXTURE);
    let trace = Trace::load(&root.join("session.rtr"))?;
    Ok((root, trace))
}

/// Slice W1 on the real client: the recorded woodcutting session replayed
/// through the production owners, cycle by cycle, with the exact outgoing
/// bytes of every cycle.
///
/// Expected values, none from the client under test:
/// - menu: the scene options (Face here / Walk here, the loc loop adding
///   ops last to first with white-tagged names) and the menu update
///   (actions >= 1000 first, the tail is the left-click option) over cache
///   loc 38760 ops `[Chop down, .., Examine]` (op 0 is action 3, Examine
///   1002);
/// - OPLOC1 (client packet 12): `p1_alt2(ctrl 0)
///   p2(z) p4(38760) p2_alt3(x)` of the absolute tree tile, flushed by the
///   next cycle's socket write (the press is at CLICK_CYCLE);
/// - positions: the dev server's own trace (`server-trace.jsonl`,
///   ALTO_TRACE_INFO), one entry per PLAYER_INFO; the reach tile is an
///   orthogonal neighbour of the 1x1 tree; the facing is the facing angle
///   from the reach tile to the tree;
/// - chop seq 21191 (cache seq whose offhand is obj 1351) from the arrival
///   tick until the success tick resets it (-1);
/// - chat: MESSAGE_GAME type 0 texts of the server content (Woodcutting.ts);
/// - logs 1511 into backpack slot 1 (slot 0 holds the `invset` hatchet),
///   read with CS2 `inv_getobj`/`inv_getnum` and shown by the cache backpack
///   1473:7 slot component (`invobject`);
/// - 58 + 25 XP = 83 = level 2 (the skill XP table), read with CS2
///   `stat_base`/`stat_visible_xp`/`stat`; the Skills tab 1466's
///   onstattransmit hooks (8488 on 1466:0, cache `stats_stat` 525 /
///   `stats_statbase` 532 on the rows 8489 builds) turn the Woodcutting row
///   texts from "1" to "2" (`stat(8)` / `stat_base_actual(8)`);
/// - level-up: IF_OPENSUB 1216 into 1477:34 and varc 5188 = 8 | 2 << 23
///   (spec §1, read back through the client varc domain); cache script 336
///   `levelup_start` (onvarctransmit 5188 on 1216:0) sets 1216:17's graphic
///   to enum 745[8] = 1485 (cache enum via the server's EnumConfig decoder);
/// - zone state: LOC_ADD_CHANGE stump 40350 on the tree tile and LOC_DEL of
///   the canopy 38789 at (3227, 3227, level 1) in the client's
///   loc-change snapshot, then the tree and canopy back on
///   respawn; the respawned tree offers "Chop down" again.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_woodcutting_session_chops_levels_up_fells_and_respawns() -> anyhow::Result<()> {
    use crate::proto::server as sp;
    let (root, trace) = fixture()?;
    let arrivals = arrivals(&trace)?;
    let (server_players, _) = server_trace_file(&root.join("server-trace.jsonl"))?;
    let e = events(&arrivals)?;
    let mut replay = Replay::start(&trace)?;
    let base = {
        let g = replay.game();
        [g.runtime.map.base_x, g.runtime.map.base_z]
    };
    let tree_local = [TREE_TILE[0] - base[0], TREE_TILE[1] - base[1]];
    let mut done = 0;
    let mut oplocs = Vec::new();
    let mut path: Vec<[i32; 3]> = Vec::new();
    let mut swing_cycle = None;
    let mut success_cycle = None;
    let mut chop_anim = None;
    let mut anim_reset = None;
    let mut arrival = None;
    let mut row_texts_before = false;
    let mut row_texts_after = None;
    for cycle in 1..=trace.last_cycle() {
        let written = replay_cycle(&mut replay, &trace, cycle)?;
        oplocs.extend(
            client_frames(&written)?
                .into_iter()
                // Client packets OPLOC1..5, OPLOC6 (examine), OPLOCT.
                .filter(|(op, _)| [12u8, 96, 27, 61, 119, 39, 21].contains(op))
                .map(|f| (cycle, f)),
        );
        let start = done;
        done = replay.processed(&arrivals);
        let has = |index: usize| done > index;
        let fresh = |index: usize| (start..done).contains(&index);

        // The mouse on the trunk: the menu, "Chop down" on top.
        if cycle == CLICK_CYCLE - 1 {
            let tree = "<col=ffff>Tree".to_string();
            let entry =
                |op: &str, target: &str, action| (op.to_string(), target.to_string(), action);
            assert_eq!(ui(&mut replay).engine.platform.mouse, CLICK);
            assert_eq!(
                menu(&mut replay),
                [
                    entry("Cancel", "", 1006),
                    entry("Examine", &tree, 1002),
                    entry("Face here", "", 60),
                    entry("Walk here", "", 23),
                    entry("Chop down", &tree, 3),
                ],
                "minimenu with the mouse on the tree"
            );
            let m = &ui(&mut replay).state.minimenu;
            let active = m.active.map(|a| m.entry(a).clone()).context("active")?;
            assert_eq!(
                (
                    active.op.as_str(),
                    (active.entity_id >> 32) as i32,
                    active.tile_x,
                    active.tile_z
                ),
                ("Chop down", TREE, tree_local[0], tree_local[1]),
                "the left-click option targets loc 38760 on the tree tile"
            );
        }

        // Positions against the server's own trace, one per PLAYER_INFO.
        let local = observe(replay.game()).local.context("local player")?;
        if arrivals[start..done]
            .iter()
            .any(|a| a.opcode == sp::PLAYER_INFO)
        {
            let k = arrivals[..done]
                .iter()
                .filter(|a| a.opcode == sp::PLAYER_INFO)
                .count();
            let [x, z, level, _] = server_players[k - 1];
            assert_eq!(local.tile, [x, z, level], "cycle {cycle}: tile vs server");
            if cycle > CLICK_CYCLE && path.last() != Some(&local.tile) {
                path.push(local.tile);
            }
        }
        if has(e.success) && success_cycle.is_none() {
            success_cycle = Some(cycle);
        }
        if has(e.swing) && swing_cycle.is_none() {
            swing_cycle = Some(cycle);
            arrival = Some(local.clone());
        }
        if local.main_anim == CHOP_SEQ && chop_anim.is_none() {
            chop_anim = Some(cycle);
        }
        if let Some(from) = chop_anim.filter(|_| anim_reset.is_none()) {
            if local.main_anim != CHOP_SEQ {
                assert_eq!(local.main_anim, -1, "cycle {cycle}: the chop seq reset");
                anim_reset = Some(cycle);
            } else {
                assert!(cycle - from < 5 * 30, "the chop seq outlived the chop");
            }
        }

        // Chat history (MESSAGE_GAME type 0).
        let mut expected_chat = Vec::new();
        if has(e.swing) {
            expected_chat.push((0, SWING.to_string()));
        }
        if has(e.success) {
            expected_chat.push((0, SUCCESS.to_string()));
        }
        assert_eq!(
            chat(ui(&mut replay)),
            expected_chat,
            "cycle {cycle}: chat history"
        );

        // Backpack, stats, the skills rows and the level-up (every 25
        // cycles and on the cycles their packets are processed).
        let probe = cycle % 25 == 0
            || [e.logs, e.stat, e.setxp, e.popup, e.varc]
                .iter()
                .any(|&i| fresh(i));
        if !probe {
            continue;
        }
        let inv = cs2_ints(
            &mut replay,
            &[
                ("push_constant_int", BACKPACK_INV),
                ("push_constant_int", 0),
                ("inv_getobj", 0),
                ("push_constant_int", BACKPACK_INV),
                ("push_constant_int", 1),
                ("inv_getobj", 0),
                ("push_constant_int", BACKPACK_INV),
                ("push_constant_int", 1),
                ("inv_getnum", 0),
            ],
        )?;
        let slot = |slot: i32, obj: i32| {
            move |c: &crate::ui_components::Component| {
                c.f.parentlayer == component::backpack::SLOTS.packed()
                    && c.f.id == slot
                    && c.f.invobject == obj
            }
        };
        if has(e.logs) {
            assert_eq!(inv, [BRONZE_HATCHET, LOGS, 1], "cycle {cycle}: backpack");
        } else {
            assert_eq!(inv[1..], [-1, 0], "cycle {cycle}: backpack slot 1 empty");
            assert!(component_rect(ui(&mut replay), &slot(1, LOGS)).is_none());
        }
        let stats = cs2_ints(
            &mut replay,
            &[
                ("push_constant_int", WOODCUTTING),
                ("stat_base", 0),
                ("push_constant_int", WOODCUTTING),
                ("stat_visible_xp", 0),
                ("push_constant_int", WOODCUTTING),
                ("stat", 0),
            ],
        )?;
        let expected = if has(e.stat) {
            [2, 83, 2]
        } else if has(e.setxp) {
            [1, 58, 1]
        } else {
            [1, 0, 1]
        };
        assert_eq!(
            stats, expected,
            "cycle {cycle}: stat_base/stat_visible_xp/stat of Woodcutting"
        );
        let rows = (
            stat_hook_texts(ui(&mut replay), 525, WOODCUTTING),
            stat_hook_texts(ui(&mut replay), 532, WOODCUTTING),
        );
        if !has(e.stat) && !rows.0.is_empty() {
            assert_eq!(rows, (vec!["1".into()], vec!["1".into()]), "cycle {cycle}");
            row_texts_before = true;
        }
        let varc = {
            let game = replay
                .core
                .session
                .as_mut()
                .unwrap()
                .game
                .as_mut()
                .context("game")?;
            crate::client_game::with_game(game, |v| {
                v.get(
                    native910::vars::VarScope::Client,
                    varc::LEVELUP_POPUP.id() as u16,
                    false,
                )
            })?
        };
        let rt = ui(&mut replay);
        let popup = rt
            .state
            .life
            .subs
            .get(component::game_window::LEVELUP_SLOT.packed())
            .map(|s| s.borrow().id);
        if !has(e.popup) {
            assert_eq!(popup, None, "cycle {cycle}: no popup before IF_OPENSUB");
            continue;
        }
        if cycle % 25 != 0 {
            // The packets' own cycle: the hooks run on the following ticks.
            continue;
        }
        // The cycles after the log tick.
        assert_eq!(
            popup,
            Some(interface::LEVELUP_POPUP.id()),
            "cycle {cycle}: 1216 in 1477:34"
        );
        // Script 336 consumed varc 5188: its last instructions are
        // `push_constant_int 0; pop_var client:5188` (cache script 336).
        assert_eq!(
            varc,
            native910::vm::Value::Int(0),
            "cycle {cycle}: varc 5188 after levelup_start"
        );
        let graphic = rt
            .store
            .interfaces
            .get(&interface::LEVELUP_POPUP.id())
            .and_then(|i| i.borrow().components.borrow().get(17).cloned().flatten())
            .map(|c| c.borrow().f.graphic);
        assert_eq!(graphic, Some(1485), "cycle {cycle}: script 336 set 1216:17");
        assert!(
            component_rect(rt, &slot(0, BRONZE_HATCHET)).is_some()
                && component_rect(rt, &slot(1, LOGS)).is_some(),
            "cycle {cycle}: the backpack interface shows the hatchet and the logs"
        );
        assert_eq!(rows, (vec!["2".into()], vec!["2".into()]), "cycle {cycle}");
        row_texts_after.get_or_insert(cycle);
        // Zone state: stump and no canopy until the respawn.
        let game = replay.game();
        let (tree, canopy) = (loc_request(game, TREE_TILE), loc_request(game, CANOPY_TILE));
        if !has(e.respawn) {
            assert_eq!(
                (tree, canopy),
                (Some((STUMP, 10, 3)), Some((-1, 10, 3))),
                "cycle {cycle}: stump, canopy removed"
            );
        }
    }
    // OPLOC1, once, flushed the cycle after the press.
    let mut expected = vec![writers::p1_alt2(0)];
    expected.extend(writers::p2(TREE_TILE[1]));
    expected.extend(writers::p4(TREE));
    expected.extend(writers::p2_alt3(TREE_TILE[0]));
    assert_eq!(
        oplocs,
        [(CLICK_CYCLE + 1, (12u8, expected))],
        "OPLOC1 on the socket"
    );
    // The walk to the reach tile: an orthogonal neighbour of the 1x1 tree,
    // faced with the facing angle.
    let arrival = arrival.context("no swing")?;
    let [x, z, _] = arrival.tile;
    assert!(path.len() >= 2, "the player walked to the tree: {path:?}");
    assert_eq!(path.last(), Some(&arrival.tile), "{path:?}");
    assert_eq!(
        (x - TREE_TILE[0]).abs() + (z - TREE_TILE[1]).abs(),
        1,
        "reach tile {:?}",
        arrival.tile
    );
    assert_eq!(
        arrival.desired_angle & 0x3FFF,
        facing_towards([x, z], [TREE_TILE[0], TREE_TILE[1]]),
        "faces the tree"
    );
    // The chop seq arrives with the swing (one server tick) and plays until
    // the success tick resets it.
    assert_eq!(chop_anim, swing_cycle, "chop seq with the swing message");
    assert_eq!(anim_reset, success_cycle, "chop seq reset on the log tick");
    // The skills rows showed level 1 before the stat and 2 after it.
    assert!(row_texts_before && row_texts_after.is_some());
    // The respawned tree and canopy, and "Chop down" on the tree again.
    let game = replay.game();
    assert_eq!(
        (loc_request(game, TREE_TILE), loc_request(game, CANOPY_TILE)),
        (Some((TREE, 10, 3)), Some((CANOPY, 10, 3))),
        "respawned"
    );
    let mut cycle = 100_000;
    let tree = "<col=ffff>Tree".to_string();
    let menu = menu_on_loc(&mut replay, &mut cycle, TREE)?;
    assert!(
        menu.contains(&("Chop down".into(), tree.clone(), 3))
            && menu.contains(&("Examine".into(), tree, 1002)),
        "respawned tree menu {menu:?}"
    );
    Ok(())
}

/// Between the fell and the respawn of the recorded session: the stump's
/// menu has no "Chop down" and the minimap's loc queue after the loc changes.
///
/// Expected: cache loc 40350 "Tree stump" ops `[null x5, Examine]`, so the
/// scene options add only its Examine (1002) with Face here / Walk here and
/// the left click walks (the tail entry). Every loc replacement ends with a
/// minimap refresh; none of 38760, 40350 and 38789 has a map element (cache
/// `mapelement` -1), so the queue the refresh rebuilds equals the queue of
/// the unchanged scene (over the same map-element locs).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_woodcutting_stump_has_no_chop_down_and_keeps_the_minimap_queue() -> anyhow::Result<()> {
    let (_, trace) = fixture()?;
    let arrivals = arrivals(&trace)?;
    let e = events(&arrivals)?;
    let mut replay = Replay::start(&trace)?;
    let mut cycle = 0;
    // Until the stump and canopy removal are applied, and 25 cycles more.
    let mut settled = None;
    while settled.is_none_or(|at| cycle < at + 25) {
        cycle += 1;
        replay_cycle(&mut replay, &trace, cycle)?;
        if settled.is_none() && replay.processed(&arrivals) > e.canopy_del {
            settled = Some(cycle);
        }
        anyhow::ensure!(
            replay.processed(&arrivals) <= e.respawn,
            "respawned before the check"
        );
    }
    let game = replay.game();
    assert_eq!(
        (loc_request(game, TREE_TILE), loc_request(game, CANOPY_TILE)),
        (Some((STUMP, 10, 3)), Some((-1, 10, 3)))
    );
    let mut cycle = 100_000;
    let stump = "<col=ffff>Tree stump".to_string();
    let entry = |op: &str, target: &str, action| (op.to_string(), target.to_string(), action);
    assert_eq!(
        menu_on_loc(&mut replay, &mut cycle, STUMP)?,
        [
            entry("Cancel", "", 1006),
            entry("Examine", &stump, 1002),
            entry("Face here", "", 60),
            entry("Walk here", "", 23),
        ],
        "the stump's menu"
    );
    let m = &ui(&mut replay).state.minimenu;
    assert_eq!(
        m.active.map(|a| m.entry(a).op.clone()).as_deref(),
        Some("Walk here")
    );

    // ViewerApp::apply_location_changes over this session's requests.
    let (mut app, _keep) = replay.into_app();
    let pack = app.pack.clone();
    app.minimap.install(&pack);
    {
        let session = app.core.session.as_ref().context("session")?;
        let rt = &session.ui;
        app.minimap.logged_in_members = rt.engine.account.logged_in_members;
        let allow = session.game.as_ref().context("game")?.allow_members;
        if let Some(locs) = &app.minimap.locs {
            locs.allow_members.set(allow);
        }
    }
    let unchanged = {
        let game = app.core.session.as_ref().unwrap().game.as_ref().unwrap();
        let p = game.runtime.feed.state.players.players[game.runtime.map.local]
            .as_ref()
            .context("local player")?;
        app.minimap.refresh(
            app.scene.graph.as_ref().context("scene graph")?,
            game.runtime.terrain.as_ref().context("terrain")?,
            0,
            Some([p.x[0], p.z[0]]),
            &Default::default(),
        );
        std::mem::take(&mut app.minimap.queued_locs)
    };
    assert!(!unchanged.is_empty(), "Lumbridge map elements");
    app.apply_location_changes(&crate::floor::SunLighting::environment_default(3, 0.0))?;
    assert_eq!(app.minimap.queued_locs, unchanged, "refreshMinimap queue");
    Ok(())
}
