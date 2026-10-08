//! Fresh server login packets drive the native toolbar and retained pointer owners.
//! The existing recording supplies only the real window/capability envelope.
use super::scenario_tests::{component_rect, ui};
use super::session_replay::{client_frames, Replay, Trace};
use crate::client_core::input_event::{InputEvent, LEFT_BUTTON, RIGHT_BUTTON};
use anyhow::{Context, Result};
use rs910_config::ui_configs::Scalar;
use rs910_symbols::{component, enums, interface, inv, obj, param, script, structs, varbit};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const RECORDING: &str = "fixtures/session-replay/legacy-interface/session.rtr";
const LOGIN_FIXTURE: &str = "fixtures/recorded/toolbar-input/login.json";
const FIXTURE_FORMAT: u8 = 1;
const FIRST_CYCLE: i32 = 1;
const LOGIN_LAYOUT_CYCLES: i32 = 90;
const POINTER_SETTLE_CYCLES: i32 = 30;
const WIELD_REPLY_CYCLES: i32 = 30;
const STARTUP_CYCLE: i32 = -1;
const NO_CHILD: i32 = -1;
const ENABLED: i32 = 1;
const MODERN_SKIN: i32 = 0;
const FIRST_SLOT: i32 = 0;
const WIELD_OPERATION: i32 = 2;
const SINGLE_ITEM: i32 = 1; // not a content id: worn stack quantity
const WINDOW_SELECTION_BIAS: i32 = 1;
const CENTRE_DIVISOR: i32 = 2;
const GROUP_BITS: u32 = 16;
const MAX_ANCESTORS: usize = 64;
const RECT_X: usize = 0;
const RECT_Y: usize = 1;
const RECT_WIDTH: usize = 2;
const RECT_HEIGHT: usize = 3;
const ZERO: i32 = 0;
const HOOK_DESTINATION_ARGUMENT: usize = 1;

#[derive(Deserialize)]
struct Packet {
    opcode: u8,
    payload: Vec<u8>,
}
#[derive(Deserialize)]
struct Packets {
    frames: Vec<Packet>,
    wire: Vec<u8>,
}
#[derive(Deserialize)]
struct Wield {
    slot: i32,
    item: i32,
    operation: i32,
    label: String,
    input: Vec<u8>,
    response: Packets,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginFixture {
    format: u8,
    profile: BTreeMap<String, Value>,
    server_varcs: Vec<u8>,
    initial: Packets,
    wield: Wield,
}

fn replace_record(trace: &mut Trace, tag: [u8; 4], bytes: Vec<u8>) {
    trace.records.retain(|record| record.tag != tag);
    trace.records.push(crate::session_record::Record {
        cycle: STARTUP_CYCLE,
        tag,
        bytes,
    });
}

fn fresh_trace(root: &std::path::Path, fixture: &LoginFixture) -> Result<Trace> {
    let mut trace = Trace::load(&root.join(RECORDING))?;
    // Keep only the real device envelope and clock. No old gameplay, input,
    // saved client preferences or server UI traffic enters this fresh login.
    trace.records.retain(|record| {
        record.tag == *b"NOWM"
            || (record.cycle == STARTUP_CYCLE
                && [
                    *b"HEAD", *b"TOOL", *b"CANV", *b"GLTF", *b"MAPI", *b"INIT", *b"IOUT", *b"SVRC",
                    *b"PREF", *b"VARC",
                ]
                .contains(&record.tag))
    });
    trace.head.retain(|(key, _)| key != "server_command");
    for (key, value) in &fixture.profile {
        let text = if key == "server_token" {
            // HEAD stores the same unsigned token bits in its signed wire view.
            (value
                .as_str()
                .context("actual TCP server token")?
                .parse::<u64>()? as i64)
                .to_string()
        } else {
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        };
        let entry = trace
            .head
            .iter_mut()
            .find(|(name, _)| name == key)
            .with_context(|| format!("recorded login profile {key}"))?;
        entry.1 = text;
    }
    trace
        .head
        .iter_mut()
        .find(|(key, _)| key == "entity_state")
        .context("native entity startup mode")?
        .1 = true.to_string();
    replace_record(&mut trace, *b"INIT", fixture.initial.wire.clone());
    // Strict entity startup returns at REBUILD_NORMAL without a timed reply.
    replace_record(&mut trace, *b"IOUT", Vec::new());
    replace_record(&mut trace, *b"SVRC", fixture.server_varcs.clone());
    replace_record(&mut trace, *b"PREF", Vec::new());
    replace_record(&mut trace, *b"VARC", Vec::new());
    Ok(trace)
}

fn bit(replay: &mut Replay, identity: i32) -> Result<i32> {
    let game = replay
        .core
        .session
        .as_mut()
        .context("live session")?
        .game
        .as_mut()
        .context("live game")?;
    crate::client_game::with_game(game, |variables| variables.get_bit(identity as u16, false))
}

fn geometry(replay: &mut Replay, parent: i32, child: i32) -> Result<Value> {
    let runtime = ui(replay);
    let mut current = runtime
        .store
        .get(parent, child)?
        .context("mounted native component")?;
    let rect = {
        let selected = current.borrow();
        component_rect(runtime, &|component| std::ptr::eq(component, &*selected))
            .context("component in ordinary visible layer walk")?
    };
    anyhow::ensure!(
        rect[RECT_WIDTH] > ZERO && rect[RECT_HEIGHT] > ZERO,
        "positive native component geometry"
    );
    let point = [
        rect[RECT_X] + rect[RECT_WIDTH] / CENTRE_DIVISOR,
        rect[RECT_Y] + rect[RECT_HEIGHT] / CENTRE_DIVISOR,
    ];
    let mut local = [point[RECT_X] - rect[RECT_X], point[RECT_Y] - rect[RECT_Y]];
    let mut ancestors = Vec::new();
    let mut seen = BTreeSet::new();
    let mut reachable = false;
    for _ in ZERO as usize..MAX_ANCESTORS {
        anyhow::ensure!(
            seen.insert(std::rc::Rc::as_ptr(&current) as usize),
            "native ancestry cycle"
        );
        let attached = rs910_ui::ui_hooks::attached(&mut runtime.store, &current)?;
        let node = current.borrow();
        let hidden = node.runtime_entry_hidden().unwrap_or(node.f.hide);
        let contains = local[RECT_X] >= ZERO
            && local[RECT_Y] >= ZERO
            && local[RECT_X] < node.f.width
            && local[RECT_Y] < node.f.height;
        ancestors.push(serde_json::json!({"component":node.f.parentlayer,"child":node.f.id,
            "attached":attached,"hidden":hidden,"contains":contains,"position":[node.f.x,node.f.y],
            "size":[node.f.width,node.f.height],"scroll":[node.f.scrollx,node.f.scrolly],"pointLocal":local}));
        if !attached || hidden || !contains {
            break;
        }
        let position = [node.f.x, node.f.y];
        let runtime_parent = node.runtime_parent();
        let layer = node.f.layer;
        let group = (node.f.parentlayer as u32 >> GROUP_BITS) as i32;
        drop(node);
        let next = if let Some(parent) = runtime_parent {
            Some(parent)
        } else if layer != NO_CHILD {
            runtime.store.get(layer, NO_CHILD)?
        } else if group == runtime.state.life.top {
            reachable = true;
            break;
        } else if let Some(&(host, _)) = runtime
            .state
            .layout
            .subs
            .iter()
            .find(|&&(_, sub)| sub == group)
        {
            runtime.store.get(host, NO_CHILD)?
        } else {
            None
        };
        let Some(next) = next else {
            break;
        };
        {
            let ancestor = next.borrow();
            local = [
                position[RECT_X] + local[RECT_X] - ancestor.f.scrollx,
                position[RECT_Y] + local[RECT_Y] - ancestor.f.scrolly,
            ];
        }
        current = next;
    }
    let result = serde_json::json!({"component":parent,"child":child,"rect":rect,"point":point,
        "reachable":reachable,"ancestors":ancestors});
    println!("TOOLBAR_NATIVE_BOUND {result}");
    anyhow::ensure!(
        reachable,
        "native pointer centre is attached and clipped through every ancestor: {result}"
    );
    Ok(result)
}

fn point(bound: &Value) -> Result<[i32; 2]> {
    let values = bound["point"].as_array().context("native pointer centre")?;
    Ok([
        i32::try_from(values[RECT_X].as_i64().context("pointer x")?)?,
        i32::try_from(values[RECT_Y].as_i64().context("pointer y")?)?,
    ])
}

fn backpack_button(replay: &mut Replay) -> Result<(i32, i32)> {
    let runtime = ui(replay);
    let destination = runtime
        .state
        .configs
        .enumeration(enums::NATIVE_INTERFACE_WINDOWS.id())
        .entries()
        .into_iter()
        .find_map(|(key, value)| match value {
            Scalar::Int(identity) if *identity == structs::BACKPACK_WINDOW.id() => Some(key),
            _ => None,
        })
        .context("cache Backpack window destination")?;
    let parent = runtime
        .store
        .get(component::window_buttons::ACTIONS.packed(), NO_CHILD)?
        .context("normal login-mounted Buttons actions")?;
    let children = parent
        .borrow()
        .children
        .clone()
        .context("native CS2-built window buttons")?;
    let mut matching = Vec::new();
    for child in children.borrow().iter().flatten() {
        let child = child.borrow();
        if child.param_int(param::WINDOW_TAB_DESTINATION.id(), NO_CHILD)? != destination {
            continue;
        }
        let hook = child.hooks.get("onop").context("cache Backpack onop")?;
        assert!(
            matches!(hook.first(),Some(crate::ui_components::Arg::Int(identity))
            if *identity == script::WINDOW_BUTTON_SELECT.id())
        );
        assert!(
            matches!(hook.get(HOOK_DESTINATION_ARGUMENT),Some(crate::ui_components::Arg::Int(key))
            if *key == destination)
        );
        matching.push(child.f.id);
    }
    assert_eq!(
        matching.len(),
        ENABLED as usize,
        "one native Backpack destination button"
    );
    Ok((destination, matching[ZERO as usize]))
}

fn advance(
    replay: &mut Replay,
    cycle: &mut i32,
    count: i32,
    inbound: &[u8],
) -> Result<Vec<(u8, Vec<u8>)>> {
    let mut frames = Vec::new();
    for offset in ZERO..count {
        *cycle += ENABLED;
        let bytes = if offset == ZERO { inbound } else { &[] };
        frames.extend(client_frames(
            &replay.step_live(*cycle, bytes, &mut |_, _| {})?.written,
        )?);
    }
    Ok(frames)
}

fn click(
    replay: &mut Replay,
    cycle: &mut i32,
    point: [i32; 2],
    button: i32,
) -> Result<Vec<(u8, Vec<u8>)>> {
    let time = crate::logic_clock::monotonic_millis();
    InputEvent::Move {
        position: point,
        time,
    }
    .apply(replay.session())?;
    InputEvent::Button {
        action: button,
        pressed: true,
        time,
    }
    .apply(replay.session())?;
    let mut frames = advance(replay, cycle, ENABLED, &[])?;
    InputEvent::Button {
        action: button,
        pressed: false,
        time: crate::logic_clock::monotonic_millis(),
    }
    .apply(replay.session())?;
    frames.extend(advance(replay, cycle, ENABLED, &[])?);
    Ok(frames)
}

fn buttons(frames: &[(u8, Vec<u8>)]) -> Vec<(u8, Vec<u8>)> {
    use crate::proto::client;
    let opcodes = [
        client::IF_BUTTON1,
        client::IF_BUTTON2,
        client::IF_BUTTON3,
        client::IF_BUTTON4,
        client::IF_BUTTON5,
        client::IF_BUTTON6,
        client::IF_BUTTON7,
        client::IF_BUTTON8,
        client::IF_BUTTON9,
        client::IF_BUTTON10,
    ];
    frames
        .iter()
        .filter(|(opcode, _)| opcodes.contains(opcode))
        .cloned()
        .collect()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn fresh_login_toolbar_pointer_opens_backpack_and_wields_native_item() -> Result<()> {
    let root = rs910_core::test_support::client_dir();
    let fixture: LoginFixture = serde_json::from_slice(&std::fs::read(root.join(LOGIN_FIXTURE))?)?;
    assert_eq!(fixture.format, FIXTURE_FORMAT);
    assert_eq!(fixture.wield.item, obj::BRONZE_DAGGER.id());
    assert_eq!(fixture.wield.slot, FIRST_SLOT);
    assert_eq!(fixture.wield.operation, WIELD_OPERATION);
    let mounts = fixture
        .initial
        .frames
        .iter()
        .filter(|frame| frame.opcode == crate::proto::server::IF_OPENSUB)
        .map(|frame| rs910_protocol::server_prot::parse_if_opensub(&frame.payload))
        .collect::<Result<Vec<_>>>()?;
    assert!(
        mounts.iter().any(|(host, group, _)| *host
            == component::game_window::WINDOW_BUTTONS_SLOT.packed() as u32
            && *group == interface::WINDOW_BUTTONS.id() as u32),
        "actual fresh TCP login owns the toolbar mount"
    );
    let trace = fresh_trace(&root, &fixture)?;
    let mut replay = Replay::start(&trace)?;
    let mut cycle = FIRST_CYCLE - ENABLED;
    advance(&mut replay, &mut cycle, LOGIN_LAYOUT_CYCLES, &[])?;
    assert_eq!(
        bit(&mut replay, varbit::LEGACY_COMBAT_ACTIVE.id())?,
        ENABLED
    );
    assert_eq!(
        bit(&mut replay, varbit::LEGACY_INTERFACE_MODE.id())?,
        ENABLED
    );
    assert_eq!(
        bit(&mut replay, varbit::INTERFACE_SKIN.id())?,
        MODERN_SKIN,
        "fresh account skin remains its normal default"
    );
    #[cfg(unix)]
    {
        let session = replay.session();
        let loaded_before = session.ui.store.interfaces.len();
        let mounted = crate::live_control::observed_children_for_test(
            session,
            component::game_window::WINDOW_BUTTONS_SLOT.packed(),
            NO_CHILD,
        )?;
        assert!(!mounted.is_empty(), "loaded toolbar roots are observable");
        assert!(mounted.iter().all(|row| row["relation"] == "mounted"));
        let mut static_children = Vec::new();
        for root in &mounted {
            let parent =
                i32::try_from(root["target"]["parent"].as_i64().context("root identity")?)?;
            let child = i32::try_from(
                root["target"]["child"]
                    .as_i64()
                    .context("root child identity")?,
            )?;
            static_children.extend(crate::live_control::observed_children_for_test(
                session, parent, child,
            )?);
        }
        assert!(
            static_children
                .iter()
                .any(|row| row["relation"] == "static"),
            "native static toolbar layers remain discoverable"
        );
        assert_eq!(
            session.ui.store.interfaces.len(),
            loaded_before,
            "read-only scans never load another interface"
        );
    }
    let (destination, child) = backpack_button(&mut replay)?;
    #[cfg(unix)]
    {
        let rows = crate::live_control::observed_children_parameters_for_test(
            replay.session(),
            component::window_buttons::ACTIONS.packed(),
            NO_CHILD,
            &[param::WINDOW_TAB_DESTINATION.id()],
        )?;
        let destination_rows: Vec<_> = rows
            .iter()
            .filter(|row| {
                row["value"]["parameters"]
                    .as_array()
                    .is_some_and(|parameters| {
                        parameters.iter().any(|parameter| {
                            parameter["id"] == param::WINDOW_TAB_DESTINATION.id()
                                && parameter["present"] == true
                                && parameter["value"]["kind"] == "int"
                                && parameter["value"]["value"] == destination
                        })
                    })
            })
            .collect();
        assert_eq!(
            destination_rows.len(),
            ENABLED as usize,
            "one observed actual runtime parameter selects the cache Backpack destination"
        );
        assert_eq!(destination_rows[ZERO as usize]["target"]["child"], child);
        let observed = rows
            .iter()
            .find(|row| row["target"]["child"].as_i64() == Some(i64::from(child)))
            .context("native Backpack runtime button is discoverable")?;
        assert_eq!(observed["relation"], "runtime");
        assert_eq!(observed["value"]["rooted_visible"], true);
    }
    let button = geometry(
        &mut replay,
        component::window_buttons::ACTIONS.packed(),
        child,
    )?;
    let mut local_frames = click(&mut replay, &mut cycle, point(&button)?, LEFT_BUTTON)?;
    local_frames.extend(advance(
        &mut replay,
        &mut cycle,
        POINTER_SETTLE_CYCLES,
        &[],
    )?);
    assert!(
        buttons(&local_frames).is_empty(),
        "client-local Backpack selection emits no IF_BUTTON: {local_frames:?}"
    );
    assert_eq!(
        bit(&mut replay, varbit::LEGACY_SELECTED_WINDOW.id())?,
        destination + WINDOW_SELECTION_BIAS
    );
    #[cfg(unix)]
    assert_eq!(
        crate::live_control::observed_client_bit_for_test(
            replay.session(),
            varbit::LEGACY_SELECTED_WINDOW.id()
        ),
        Some(destination + WINDOW_SELECTION_BIAS),
        "controller observes native CLIENT-domain toolbar selection"
    );
    let slot = geometry(
        &mut replay,
        component::backpack::SLOTS.packed(),
        fixture.wield.slot,
    )?;
    {
        let grid = ui(&mut replay)
            .store
            .get(component::backpack::SLOTS.packed(), fixture.wield.slot)?
            .context("native visible item slot")?;
        assert_eq!(grid.borrow().f.invobject, obj::BRONZE_DAGGER.id());
    }
    // The item menu is built by the retained pointer path, including its
    // native Wield label and operation index; no component operation is injected.
    let mut wield_frames = click(&mut replay, &mut cycle, point(&slot)?, RIGHT_BUTTON)?;
    let menu_point = {
        let menu = &ui(&mut replay).state.minimenu;
        anyhow::ensure!(menu.open && !menu.grouped, "native item popup is open");
        let row = menu
            .popup_rows(false)
            .into_iter()
            .find(|row| {
                let entry = menu.entry(row.entry);
                entry.op == fixture.wield.label
                    && entry.obj_id == fixture.wield.item
                    && entry.entity_id == i64::from(fixture.wield.operation)
            })
            .context("native item Wield menu entry")?;
        let point = [
            menu.popup.bounds[RECT_X] + menu.popup.bounds[RECT_WIDTH] / CENTRE_DIVISOR,
            row.baseline - menu.popup.ascent / CENTRE_DIVISOR,
        ];
        anyhow::ensure!(
            point[RECT_X] >= menu.popup.bounds[RECT_X]
                && point[RECT_X] < menu.popup.bounds[RECT_X] + menu.popup.bounds[RECT_WIDTH]
                && point[RECT_Y] >= menu.popup.bounds[RECT_Y]
                && point[RECT_Y] < menu.popup.bounds[RECT_Y] + menu.popup.bounds[RECT_HEIGHT],
            "native Wield row pointer lies inside the actual popup"
        );
        point
    };
    wield_frames.extend(click(&mut replay, &mut cycle, menu_point, LEFT_BUTTON)?);
    wield_frames.extend(advance(
        &mut replay,
        &mut cycle,
        POINTER_SETTLE_CYCLES,
        &[],
    )?);
    let expected = client_frames(&fixture.wield.input)?;
    assert_eq!(
        buttons(&wield_frames),
        expected,
        "ordinary native Wield request matches fresh TCP input"
    );
    advance(
        &mut replay,
        &mut cycle,
        WIELD_REPLY_CYCLES,
        &fixture.wield.response.wire,
    )?;
    let inventory = ui(&mut replay)
        .engine
        .inv_cache
        .inventory(inv::WORN_EQUIPMENT.id(), false)
        .context("ordinary native worn inventory response")?;
    let worn = inventory
        .obj_ids
        .iter()
        .position(|identity| *identity == obj::BRONZE_DAGGER.id())
        .context("normal server-accepted Wield reached client worn inventory")?;
    assert_eq!(inventory.counts[worn], SINGLE_ITEM);
    println!(
        "TOOLBAR_NATIVE_PREFLIGHT {}",
        serde_json::json!({"destination":destination,"child":child,
        "selectedWindow":destination+WINDOW_SELECTION_BIAS,"localButtonPackets":0,
        "backpack":slot,"wieldRequests":expected,"freshLogin":true,"renderedScreenshot":false})
    );
    Ok(())
}
