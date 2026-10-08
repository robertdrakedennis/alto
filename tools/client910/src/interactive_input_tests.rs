//! Recorded native input against the ordinary session, camera and scene owners.
use super::scenario_tests::{hit_point, install_pick_frame, ui, writers};
use super::session_replay::{client_frames, CycleOutput, Replay, Trace, FIXTURE};
use crate::client_core::input_event::{InputEvent, LEFT_BUTTON, RECORD_TAG};
use crate::client_core::ReplayIo;
use crate::proto::client as cp;
use crate::session_record::Record;
use anyhow::{Context, Result};
use rs910_symbols::loc;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::net::Shutdown;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RECORDED_WORLD_READY: i32 = 130;
const FIRST_LOGIC_CYCLE: i32 = 1;
const NEXT_CYCLE: i32 = 1;
const ZOOM_OUT_EVENTS: i32 = 15;
const FIRST_ZOOM_EVENT: i32 = 0;
const WHEEL_ZOOM_OUT: i32 = 1;
const CAMERA_PAN_PULSES: i32 = 3;
const FIRST_CAMERA_PULSE: i32 = 0;
const CAMERA_HELD_CYCLES: i32 = 2;
const CAMERA_RELEASE_CYCLES: i32 = 10;
const AWT_LEFT: i32 = 37;
const NATIVE_LEFT: i32 = 96;
const NO_CONTROL_MODIFIER: i32 = 0;
const CANVAS_CENTRE_DIVISOR: i32 = 2;
const FIRST_NATIVE_OPERATION: usize = 0;
const DEFAULT_OPERATION_CURSOR: usize = 1;
const NO_CURSOR: i32 = -1;
const LOC_DEFINITION_SHIFT: u32 = 32; // not a content id: packed scene-key bit width
const POINTER_COORDINATE_SHIFT: u32 = 16;
const POINTER_POSITION_BYTES: usize = 4;
const POINTER_CLICK_BYTES: usize = 6;
const KEYBOARD_EVENT_BYTES: usize = 4;
const KEYBOARD_CODE_OFFSET: usize = 0;
const KEYBOARD_TIME_OFFSET: usize = 1;
const FIRST_KEYBOARD_EVENT: usize = 1;
const EMPTY_REMAINDER: usize = 0;
const FIRST_CLICK: usize = 0;
const REQUIRED_POINTER_CLICKS: usize = 1;
const ZERO_BYTE: u8 = 0;
const EVENT_AFTER_FRAME_MILLIS: i64 = 1;
const LOGIC_INTERVAL_MILLIS: i64 = 20;
const NO_TRANSLATION: [f32; 3] = [0.0; 3];
#[cfg(unix)]
const CONTROLLER_REPLY_TIMEOUT_SECONDS: u64 = 1;
#[cfg(unix)]
const MAX_CONTROLLER_REPLY_BYTES: u64 = 512 * 1024;
#[cfg(unix)]
const EMPTY_CONTROLLER_REPLY: usize = 0;
#[cfg(unix)]
const SNAPSHOT_REQUEST_ID: u64 = 1;
#[cfg(unix)]
const LOC_SCAN_REQUEST_ID: u64 = 2; // not a content id: control request sequence
#[cfg(unix)]
const LOC_ACTION_REQUEST_ID: u64 = 3; // not a content id: control request sequence
#[cfg(unix)]
const REQUIRED_SCAN_ENTRIES: usize = 1;
#[cfg(unix)]
const FIRST_SCAN_ENTRY: usize = 0;
#[cfg(unix)]
const SOCKET_TEST_ROOT: &str = "/tmp";
#[cfg(unix)]
const LOC_OPERATION: &str = "Climb-up";

#[cfg(unix)]
fn controller_send(peer: &mut UnixStream, id: u64, request: serde_json::Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(&serde_json::json!({"id":id,"request":request}))?;
    bytes.push(b'\n');
    peer.write_all(&bytes)?;
    Ok(())
}

#[cfg(unix)]
fn controller_reply(peer: &mut BufReader<UnixStream>, id: u64) -> Result<serde_json::Value> {
    let mut bytes = Vec::new();
    let count = peer
        .by_ref()
        .take(MAX_CONTROLLER_REPLY_BYTES)
        .read_until(b'\n', &mut bytes)?;
    anyhow::ensure!(
        count > EMPTY_CONTROLLER_REPLY && bytes.last() == Some(&b'\n'),
        "controller reply did not finish within its byte bound"
    );
    let response: serde_json::Value = serde_json::from_slice(&bytes)?;
    assert_eq!(response["id"], id);
    Ok(response)
}

#[cfg(unix)]
fn observed_integer(value: &serde_json::Value, field: &str) -> Result<i32> {
    Ok(i32::try_from(
        value[field]
            .as_i64()
            .with_context(|| format!("observed {field}"))?,
    )?)
}

fn append_input(trace: &mut Trace, cycle: i32, input: InputEvent) -> Result<()> {
    trace.records.push(Record {
        cycle,
        tag: *RECORD_TAG,
        bytes: input.encode()?,
    });
    Ok(())
}

fn frame_time(clock: i64, cycle: i32) -> i64 {
    clock + i64::from(cycle - RECORDED_WORLD_READY) * LOGIC_INTERVAL_MILLIS
}

fn event_time(clock: i64, cycle: i32) -> i64 {
    frame_time(clock, cycle) + EVENT_AFTER_FRAME_MILLIS
}

fn append_clock(trace: &mut Trace, clock: i64, cycles: std::ops::RangeInclusive<i32>) {
    for cycle in cycles {
        trace.records.push(Record {
            cycle,
            tag: *b"NOWM",
            bytes: frame_time(clock, cycle).to_le_bytes().to_vec(),
        });
    }
}

/// The fixture prefix is a live, fully installed world. Its extension keeps
/// the peer open and server silent, as the existing native-input scenarios do.
/// Refresh the immutable record list only at a drained transport boundary.
fn install_extension(replay: &mut Replay, trace: &Trace) {
    assert_eq!(replay.io.unread(), EMPTY_REMAINDER, "unread fixture bytes");
    replay.io = ReplayIo::new(trace.records.clone());
}

fn recorded_pick_cycle(replay: &mut Replay, trace: &Trace, cycle: i32) -> Result<CycleOutput> {
    let mut scene = replay
        .assets
        .scene_graph
        .take()
        .context("installed scene graph")?;
    let output = replay.cycle_with_picks(trace, cycle, &mut |_, ui| {
        let mouse = ui.input.click.unwrap_or(ui.engine.platform.mouse);
        if let Some(frame) = ui.engine.scene.player_picks.as_mut() {
            frame.refresh_locs(&mut scene, mouse);
        }
    });
    replay.assets.scene_graph = Some(scene);
    output
}

fn location_packets(frames: &[(u8, Vec<u8>)]) -> Vec<(u8, Vec<u8>)> {
    frames
        .iter()
        .filter(|(opcode, _)| {
            [
                cp::OPLOC1,
                cp::OPLOC2,
                cp::OPLOC3,
                cp::OPLOC4,
                cp::OPLOC5,
                cp::OPLOC6,
                cp::OPLOCT,
            ]
            .contains(opcode)
        })
        .cloned()
        .collect()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn recorded_input_waits_for_the_next_cycle_then_pans_and_clicks_the_native_scene() -> Result<()> {
    let root = rs910_core::test_support::client_dir();
    let mut trace = Trace::load(&root.join(FIXTURE))?;
    // Retain the recorded login, persistence, map, camera and native interfaces;
    // subsequent world traffic belongs to the old scenario, not this input.
    trace
        .records
        .retain(|record| record.cycle <= RECORDED_WORLD_READY);
    let mut replay = Replay::start(&trace)?;
    for cycle in FIRST_LOGIC_CYCLE..=RECORDED_WORLD_READY {
        replay.cycle(&trace, cycle)?;
    }
    ui(&mut replay).paint(RECORDED_WORLD_READY, true, NO_TRANSLATION)?;
    let (viewport, _) = ui(&mut replay)
        .state
        .viewport
        .context("native scene viewport")?;
    let [x, y, width, height] = viewport;
    let viewport_centre = [
        x + width / CANVAS_CENTRE_DIVISOR,
        y + height / CANVAS_CENTRE_DIVISOR,
    ];
    let clock = crate::logic_clock::monotonic_millis();
    let first_event = RECORDED_WORLD_READY + NEXT_CYCLE;
    append_input(
        &mut trace,
        first_event,
        InputEvent::Move {
            position: viewport_centre,
            time: event_time(clock, first_event),
        },
    )?;
    for offset in FIRST_ZOOM_EVENT..ZOOM_OUT_EVENTS {
        append_input(
            &mut trace,
            first_event + offset,
            InputEvent::Wheel {
                delta: WHEEL_ZOOM_OUT,
            },
        )?;
    }
    let first_press = first_event + ZOOM_OUT_EVENTS;
    let pulse_cycles = CAMERA_HELD_CYCLES + CAMERA_RELEASE_CYCLES;
    for pulse in FIRST_CAMERA_PULSE..CAMERA_PAN_PULSES {
        let press = first_press + pulse * pulse_cycles;
        let release = press + CAMERA_HELD_CYCLES;
        for (cycle, pressed) in [(press, true), (release, false)] {
            append_input(
                &mut trace,
                cycle,
                InputEvent::Key {
                    code: AWT_LEFT,
                    pressed,
                    text: None,
                    time: event_time(clock, cycle),
                },
            )?;
        }
    }
    let camera_done = first_press + CAMERA_PAN_PULSES * pulse_cycles;
    append_clock(&mut trace, clock, first_event..=camera_done);
    install_extension(&mut replay, &trace);
    let mut frames = Vec::new();
    let mut press_boundary_yaw = None;
    for cycle in first_event..=camera_done {
        let output = recorded_pick_cycle(&mut replay, &trace, cycle)?;
        frames.extend(client_frames(&output.written)?);
        if cycle == first_event {
            assert_eq!(
                ui(&mut replay).engine.platform.pending_mouse,
                viewport_centre
            );
            assert_eq!(
                ui(&mut replay).input.wheel,
                WHEEL_ZOOM_OUT,
                "UIEV is accepted after this cycle, not consumed within it"
            );
        }
        if cycle == first_press {
            assert!(!ui(&mut replay).keyboard.held(NATIVE_LEFT));
            press_boundary_yaw = Some(ui(&mut replay).engine.camera.cam2.yaw());
        }
        if cycle == first_press + NEXT_CYCLE {
            assert!(ui(&mut replay).keyboard.held(NATIVE_LEFT));
        }
        if cycle == first_press + CAMERA_HELD_CYCLES {
            assert!(
                ui(&mut replay).keyboard.held(NATIVE_LEFT),
                "the recorded release is queued after the current update"
            );
            assert_ne!(
                Some(ui(&mut replay).engine.camera.cam2.yaw()),
                press_boundary_yaw,
                "the retained native camera consumes the recorded key"
            );
        }
        if cycle == first_press + CAMERA_HELD_CYCLES + NEXT_CYCLE {
            assert!(!ui(&mut replay).keyboard.held(NATIVE_LEFT));
        }
    }
    let keyboard: Vec<_> = frames
        .iter()
        .filter(|(opcode, _)| *opcode == cp::EVENT_KEYBOARD)
        .flat_map(|(_, payload)| {
            assert_eq!(payload.len() % KEYBOARD_EVENT_BYTES, EMPTY_REMAINDER);
            payload
                .chunks_exact(KEYBOARD_EVENT_BYTES)
                .map(<[u8]>::to_vec)
        })
        .collect();
    assert_eq!(
        keyboard
            .iter()
            .map(|event| event[KEYBOARD_CODE_OFFSET])
            .collect::<Vec<_>>(),
        vec![NATIVE_LEFT as u8; CAMERA_PAN_PULSES as usize],
        "ordinary telemetry reports native presses, not AWT codes or releases"
    );
    let repeat_delay = (i64::from(pulse_cycles) * LOGIC_INTERVAL_MILLIS) as u32;
    for event in keyboard.iter().skip(FIRST_KEYBOARD_EVENT) {
        let mut time = [ZERO_BYTE; KEYBOARD_EVENT_BYTES];
        time[KEYBOARD_TIME_OFFSET..].copy_from_slice(&event[KEYBOARD_TIME_OFFSET..]);
        assert_eq!(
            u32::from_be_bytes(time),
            repeat_delay,
            "recorded event timestamps reach ordinary keyboard telemetry"
        );
    }
    assert!(frames
        .iter()
        .any(|(opcode, _)| *opcode == cp::EVENT_CAMERA_POSITION));
    assert!(location_packets(&frames).is_empty());

    ui(&mut replay).paint(camera_done, true, NO_TRANSLATION)?;
    ui(&mut replay).paint(camera_done, true, NO_TRANSLATION)?;
    install_pick_frame(&mut replay)?;
    let ladder = loc::LUMBRIDGE_CASTLE_LADDER.id();
    let pointer = hit_point(&mut replay, ladder)?;
    let hover_boundary = camera_done + NEXT_CYCLE;
    let hover_consumed = hover_boundary + NEXT_CYCLE;
    let click_boundary = hover_consumed + NEXT_CYCLE;
    let click_consumed = click_boundary + NEXT_CYCLE;
    let pointer_done = click_consumed + CAMERA_HELD_CYCLES;
    append_input(
        &mut trace,
        hover_boundary,
        InputEvent::Move {
            position: pointer,
            time: event_time(clock, hover_boundary),
        },
    )?;
    for (cycle, pressed) in [(click_boundary, true), (click_consumed, false)] {
        append_input(
            &mut trace,
            cycle,
            InputEvent::Button {
                action: LEFT_BUTTON,
                pressed,
                time: event_time(clock, cycle),
            },
        )?;
    }
    append_clock(&mut trace, clock, hover_boundary..=pointer_done);
    install_extension(&mut replay, &trace);
    let output = recorded_pick_cycle(&mut replay, &trace, hover_boundary)?;
    assert_eq!(ui(&mut replay).engine.platform.pending_mouse, pointer);
    assert_eq!(
        ui(&mut replay).engine.platform.mouse,
        viewport_centre,
        "the recorded move has not crossed the next mouse-flip boundary"
    );
    assert!(location_packets(&client_frames(&output.written)?).is_empty());
    recorded_pick_cycle(&mut replay, &trace, hover_consumed)?;
    assert_eq!(ui(&mut replay).engine.platform.mouse, pointer);
    let native_menu = &ui(&mut replay).state.minimenu;
    assert_eq!(
        native_menu
            .entries
            .iter()
            .map(|entry| native_menu.entry(*entry).op.as_str())
            .collect::<Vec<_>>(),
        ["Cancel", "Examine", "Face here", "Walk here", "Climb-up"]
    );
    let climb = native_menu
        .entry(native_menu.active.context("active native loc option")?)
        .clone();
    assert_eq!(climb.op, "Climb-up");
    assert_eq!((climb.entity_id >> LOC_DEFINITION_SHIFT) as i32, ladder);
    let ladder_type = ui(&mut replay)
        .engine
        .configs
        .locs
        .as_deref()
        .and_then(|locs| locs.get(ladder as u32))
        .cloned()
        .context("native ladder type")?;
    let cursor = match ladder_type.cursor[FIRST_NATIVE_OPERATION] {
        NO_CURSOR => ui(&mut replay).engine.menu.default_cursors[DEFAULT_OPERATION_CURSOR],
        cursor => cursor,
    };
    assert_eq!(climb.cursor, cursor);
    assert_eq!(ui(&mut replay).cursor_id(), cursor);
    let mut click_frames = Vec::new();
    for cycle in click_boundary..=pointer_done {
        let output = recorded_pick_cycle(&mut replay, &trace, cycle)?;
        let written = client_frames(&output.written)?;
        if cycle == click_boundary {
            assert!(
                location_packets(&written).is_empty(),
                "click is still queued"
            );
            assert!(ui(&mut replay).input.left_held);
            assert_eq!(ui(&mut replay).input.click, Some(pointer));
        }
        click_frames.extend(written);
    }
    assert!(!ui(&mut replay).input.left_held);
    assert!(ui(&mut replay).input.click.is_none());
    let map = &replay.game().runtime.map;
    let mut expected = vec![writers::p1_alt2(NO_CONTROL_MODIFIER)];
    expected.extend(writers::p2(map.base_z + climb.tile_z));
    expected.extend(writers::p4(ladder));
    expected.extend(writers::p2_alt3(map.base_x + climb.tile_x));
    assert_eq!(
        location_packets(&click_frames),
        [(cp::OPLOC1, expected)],
        "real retained click emits the native location operation onto the ordinary socket"
    );
    let click: Vec<_> = click_frames
        .iter()
        .filter(|(opcode, _)| *opcode == cp::EVENT_MOUSE_CLICK)
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(click.len(), REQUIRED_POINTER_CLICKS);
    assert_eq!(click[FIRST_CLICK].len(), POINTER_CLICK_BYTES);
    let [pointer_x, pointer_y] = pointer;
    assert_eq!(
        &click[FIRST_CLICK][..POINTER_POSITION_BYTES],
        &(pointer_x | pointer_y << POINTER_COORDINATE_SHIFT).to_be_bytes(),
        "native mouse telemetry carries the actual retained pick position"
    );
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
#[cfg(unix)]
fn controller_half_close_preserves_a_native_observed_loc_action_and_its_packet() -> Result<()> {
    use crate::live_control::{ActionOutcome, LiveControl};

    let root = rs910_core::test_support::client_dir();
    let mut trace = Trace::load(&root.join(FIXTURE))?;
    trace
        .records
        .retain(|record| record.cycle <= RECORDED_WORLD_READY);
    let mut replay = Replay::start(&trace)?;
    for cycle in FIRST_LOGIC_CYCLE..=RECORDED_WORLD_READY {
        replay.cycle(&trace, cycle)?;
    }
    // This private short socket name avoids platform sockaddr path limits.
    // The production binder refuses existing paths and unlinks only its inode.
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let socket_path = std::path::Path::new(SOCKET_TEST_ROOT)
        .join(format!("alto-input-{}-{unique}.sock", std::process::id()));
    let mut controller = LiveControl::bind_for_test(socket_path.clone())?;
    let mut peer = UnixStream::connect(&socket_path)?;
    let timeout = Some(Duration::from_secs(CONTROLLER_REPLY_TIMEOUT_SECONDS));
    peer.set_read_timeout(timeout)?;
    peer.set_write_timeout(timeout)?;
    let mut replies = BufReader::new(peer.try_clone()?);
    let clock = crate::logic_clock::monotonic_millis();
    let outgoing = ui(&mut replay).engine.outgoing.clone();

    controller_send(
        &mut peer,
        SNAPSHOT_REQUEST_ID,
        serde_json::json!({"command":"snapshot"}),
    )?;
    assert!(controller
        .poll(replay.session(), RECORDED_WORLD_READY, true, clock)?
        .is_none());
    let snapshot = controller_reply(&mut replies, SNAPSHOT_REQUEST_ID)?;
    assert_eq!(snapshot["status"], "observed");
    assert_eq!(snapshot["data"]["ready"], true);
    let map = snapshot["data"]["map"].clone();
    let level = observed_integer(&map, "level")?;
    let ladder = loc::LUMBRIDGE_CASTLE_LADDER.id();
    controller_send(
        &mut peer,
        LOC_SCAN_REQUEST_ID,
        serde_json::json!({
            "command":"scan_locs", "query":{
                "definition":ladder,"level":level,"limit":REQUIRED_SCAN_ENTRIES
            }
        }),
    )?;
    assert!(controller
        .poll(replay.session(), RECORDED_WORLD_READY, true, clock)?
        .is_none());
    let scan = controller_reply(&mut replies, LOC_SCAN_REQUEST_ID)?;
    assert_eq!(scan["status"], "observed");
    assert_eq!(scan["data"]["map"], map);
    let entries = scan["data"]["scan"]["entries"]
        .as_array()
        .context("native loc observations")?;
    assert_eq!(entries.len(), REQUIRED_SCAN_ENTRIES);
    let observed = &entries[FIRST_SCAN_ENTRY];
    assert_eq!(observed["definition"], ladder);
    assert!(observed["ops"]
        .as_array()
        .context("native loc operations")?
        .iter()
        .any(|operation| operation == LOC_OPERATION));
    let x = observed_integer(observed, "x")?;
    let z = observed_integer(observed, "z")?;
    assert_eq!(
        ui(&mut replay).engine.outgoing,
        outgoing,
        "read-only observations do not produce gameplay packets"
    );

    controller_send(
        &mut peer,
        LOC_ACTION_REQUEST_ID,
        serde_json::json!({
            "command":"action", "map":map, "observed_cycle":RECORDED_WORLD_READY,
            "action":{
                "kind":"loc", "definition":observed["definition"],
                "x":x,"z":z,"level":observed["level"],
                "shape":observed["shape"],"angle":observed["angle"],"operation":LOC_OPERATION
            }
        }),
    )?;
    peer.shutdown(Shutdown::Write)?;
    let pending = controller
        .poll(replay.session(), RECORDED_WORLD_READY, true, clock)?
        .context("complete request must survive the peer write half-close")?;
    assert_eq!(pending.id, LOC_ACTION_REQUEST_ID);
    assert_eq!(pending.provenance, "native-loc-menu");
    let choice = match &pending.event {
        InputEvent::Menu { choice, .. } => choice,
        other => anyhow::bail!("controller produced a non-native loc input: {other:?}"),
    };
    assert!(choice.enabled);
    assert_eq!(choice.op, LOC_OPERATION);
    assert_eq!((choice.entity_id >> LOC_DEFINITION_SHIFT) as i32, ladder);
    assert_eq!(choice.tile_x + replay.game().runtime.map.base_x, x);
    assert_eq!(choice.tile_z + replay.game().runtime.map.base_z, z);
    assert_eq!(
        ui(&mut replay).engine.outgoing,
        outgoing,
        "resolution returns canonical input; it does not write the gameplay socket"
    );

    // The same retained input owner used by live acceptance records and applies
    // this choice at the current event boundary. The next ordinary frame owns
    // sending its generated packet; no test route or packet builder is invoked.
    append_input(&mut trace, RECORDED_WORLD_READY, pending.event.clone())?;
    pending.event.apply(replay.session())?;
    controller.complete(&pending, RECORDED_WORLD_READY, ActionOutcome::Applied)?;
    let accepted = controller_reply(&mut replies, LOC_ACTION_REQUEST_ID)?;
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(
        accepted["data"]["meaning"],
        "input application only; observe later gameplay state"
    );
    let next = RECORDED_WORLD_READY + NEXT_CYCLE;
    append_clock(&mut trace, clock, next..=next);
    install_extension(&mut replay, &trace);
    let output = replay.cycle(&trace, next)?;
    let mut expected = vec![writers::p1_alt2(NO_CONTROL_MODIFIER)];
    expected.extend(writers::p2(z));
    expected.extend(writers::p4(ladder));
    expected.extend(writers::p2_alt3(x));
    assert_eq!(
        location_packets(&client_frames(&output.written)?),
        [(cp::OPLOC1, expected)],
        "actual observed cache operation passes through retained input to the normal socket"
    );
    assert!(controller
        .poll(replay.session(), next, true, frame_time(clock, next))?
        .is_none());
    drop(replies);
    drop(peer);
    drop(controller);
    assert!(
        !socket_path.exists(),
        "only the bound owned socket was removed"
    );
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
#[cfg(unix)]
fn controller_observes_recorded_ground_stack_and_takes_it_through_the_native_menu() -> Result<()> {
    use crate::live_control::{ActionOutcome, LiveControl};
    use rs910_symbols::obj;

    const GROUND_FIXTURE: &str = "fixtures/session-replay/npc-combat/session.rtr";
    const OBJECT_SCAN_ID: u64 = 1;
    const STALE_REVISION_ID: u64 = 2;
    const STALE_COUNT_ID: u64 = 3;
    const STALE_INDEX_ID: u64 = 4;
    const OBJECT_ACTION_ID: u64 = 5;
    const GROUND_ITEM_ID_OFFSET: usize = 0;
    const GROUND_X_OFFSET: usize = 2;
    const GROUND_Z_OFFSET: usize = 4;
    const GROUND_FIELD_BYTES: usize = 2;
    const GROUND_OPERATION_BYTES: usize = 7;
    const NEXT_GROUND_REVISION: u64 = 1;
    const NEXT_GROUND_COUNT: i32 = 1;
    const ABSENT_GROUND_INDEX: usize = usize::MAX;
    const GROUND_OPERATION: &str = "Take";
    const EXACT_GROUND_TILE_RADIUS: usize = 0;

    let root = rs910_core::test_support::client_dir();
    let mut trace = Trace::load(&root.join(GROUND_FIXTURE))?;
    let item = obj::RAW_CHICKEN.id();
    // Select the exact existing recorded native Take. Its coordinates are
    // fixture data, not a new hardcoded test tile or synthetic ground packet.
    let mut recorded_take = None;
    for cycle in FIRST_LOGIC_CYCLE..=trace.last_cycle() {
        for (opcode, bytes) in client_frames(&trace.bytes(b"OUT ", cycle))? {
            if opcode != cp::OPOBJ3 {
                continue;
            }
            anyhow::ensure!(
                bytes.len() == GROUND_OPERATION_BYTES,
                "recorded OPOBJ3 length"
            );
            let id = i32::from(u16::from_le_bytes(
                bytes[GROUND_ITEM_ID_OFFSET..GROUND_ITEM_ID_OFFSET + GROUND_FIELD_BYTES]
                    .try_into()?,
            ));
            if id == item {
                recorded_take = Some((cycle, bytes));
                break;
            }
        }
        if recorded_take.is_some() {
            break;
        }
    }
    let (take_cycle, expected_packet) = recorded_take.context("recorded Raw chicken Take")?;
    let x = i32::from(u16::from_le_bytes(
        expected_packet[GROUND_X_OFFSET..GROUND_X_OFFSET + GROUND_FIELD_BYTES].try_into()?,
    ));
    let z = i32::from(u16::from_be_bytes(
        expected_packet[GROUND_Z_OFFSET..GROUND_Z_OFFSET + GROUND_FIELD_BYTES].try_into()?,
    ));
    let ready_cycle = take_cycle
        .checked_sub(NEXT_CYCLE)
        .context("Take before fixture start")?;
    trace.records.retain(|record| record.cycle <= ready_cycle);
    let mut replay = Replay::start(&trace)?;
    for cycle in FIRST_LOGIC_CYCLE..=ready_cycle {
        replay.cycle(&trace, cycle)?;
    }

    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let socket_path = std::path::Path::new(SOCKET_TEST_ROOT)
        .join(format!("alto-ground-{}-{unique}.sock", std::process::id()));
    let mut controller = LiveControl::bind_for_test(socket_path.clone())?;
    let mut peer = UnixStream::connect(&socket_path)?;
    let timeout = Some(Duration::from_secs(CONTROLLER_REPLY_TIMEOUT_SECONDS));
    peer.set_read_timeout(timeout)?;
    peer.set_write_timeout(timeout)?;
    let mut replies = BufReader::new(peer.try_clone()?);
    let clock = crate::logic_clock::monotonic_millis();
    let outgoing = ui(&mut replay).engine.outgoing.clone();
    controller_send(
        &mut peer,
        OBJECT_SCAN_ID,
        serde_json::json!({
            "command":"scan_objects", "query":{"definition":item,"x":x,"z":z,
                "radius":EXACT_GROUND_TILE_RADIUS,"limit":REQUIRED_SCAN_ENTRIES}
        }),
    )?;
    assert!(controller
        .poll(replay.session(), ready_cycle, true, clock)?
        .is_none());
    let scan = controller_reply(&mut replies, OBJECT_SCAN_ID)?;
    assert_eq!(scan["status"], "observed");
    let entries = scan["data"]["scan"]["entries"]
        .as_array()
        .context("observed ground rows")?;
    assert_eq!(entries.len(), REQUIRED_SCAN_ENTRIES);
    let row = &entries[FIRST_SCAN_ENTRY];
    assert_eq!(row["definition"], item);
    assert_eq!(observed_integer(row, "x")?, x);
    assert_eq!(observed_integer(row, "z")?, z);
    let revision = row["object_revision"]
        .as_u64()
        .context("observed object revision")?;
    assert_eq!(scan["data"]["object_revision"], revision);
    let count = observed_integer(row, "count")?;
    let map = scan["data"]["map"].clone();
    let action = serde_json::json!({
        "kind":"object","definition":item,"x":x,"z":z,"level":row["level"],
        "stack_index":row["stack_index"],"count":count,"object_revision":revision,
        "operation":GROUND_OPERATION
    });
    assert_eq!(
        ui(&mut replay).engine.outgoing,
        outgoing,
        "stack observation is read-only"
    );

    for (id, field, value) in [
        (
            STALE_REVISION_ID,
            "object_revision",
            serde_json::json!(revision
                .checked_add(NEXT_GROUND_REVISION)
                .context("fixture revision overflow")?),
        ),
        (
            STALE_COUNT_ID,
            "count",
            serde_json::json!(count
                .checked_add(NEXT_GROUND_COUNT)
                .context("fixture count overflow")?),
        ),
        (
            STALE_INDEX_ID,
            "stack_index",
            serde_json::json!(ABSENT_GROUND_INDEX),
        ),
    ] {
        let mut stale = action.clone();
        stale
            .as_object_mut()
            .context("object action")?
            .insert(field.into(), value);
        controller_send(
            &mut peer,
            id,
            serde_json::json!({"command":"action","map":map,
            "observed_cycle":ready_cycle,"action":stale}),
        )?;
        assert!(controller
            .poll(replay.session(), ready_cycle, true, clock)?
            .is_none());
        assert_eq!(controller_reply(&mut replies, id)?["status"], "refused");
        assert_eq!(
            ui(&mut replay).engine.outgoing,
            outgoing,
            "stale ground actions cannot write gameplay bytes"
        );
    }

    controller_send(
        &mut peer,
        OBJECT_ACTION_ID,
        serde_json::json!({"command":"action",
        "map":map,"observed_cycle":ready_cycle,"action":action}),
    )?;
    let pending = controller
        .poll(replay.session(), ready_cycle, true, clock)?
        .context("native ground input")?;
    assert_eq!(pending.provenance, "native-ground-menu");
    let choice = match &pending.event {
        InputEvent::Menu { choice, .. } => choice,
        other => anyhow::bail!("ground action produced non-menu input: {other:?}"),
    };
    assert_eq!(choice.op, GROUND_OPERATION);
    assert_eq!(choice.entity_id, i64::from(item));
    assert_eq!(
        choice.sub_id,
        row["stack_index"]
            .as_i64()
            .context("observed stack index")?
    );
    assert_eq!(choice.tile_x + replay.game().runtime.map.base_x, x);
    assert_eq!(choice.tile_z + replay.game().runtime.map.base_z, z);
    assert_eq!(
        ui(&mut replay).engine.outgoing,
        outgoing,
        "resolution returns retained input only"
    );
    append_input(&mut trace, ready_cycle, pending.event.clone())?;
    pending.event.apply(replay.session())?;
    controller.complete(&pending, ready_cycle, ActionOutcome::Applied)?;
    assert_eq!(
        controller_reply(&mut replies, OBJECT_ACTION_ID)?["status"],
        "accepted"
    );

    trace.records.push(Record {
        cycle: take_cycle,
        tag: *b"NOWM",
        bytes: clock
            .checked_add(LOGIC_INTERVAL_MILLIS)
            .context("fixture clock overflow")?
            .to_le_bytes()
            .to_vec(),
    });
    install_extension(&mut replay, &trace);
    let output = replay.cycle(&trace, take_cycle)?;
    let takes: Vec<_> = client_frames(&output.written)?
        .into_iter()
        .filter(|(opcode, _)| *opcode == cp::OPOBJ3)
        .collect();
    assert_eq!(
        takes,
        [(cp::OPOBJ3, expected_packet)],
        "retained ground menu emits the exact native Take recorded during the ordinary fight"
    );
    drop(replies);
    drop(peer);
    drop(controller);
    assert!(
        !socket_path.exists(),
        "only the owned controller socket is removed"
    );
    Ok(())
}
