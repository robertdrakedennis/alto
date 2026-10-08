//! Regression using unchanged revision 910 camera scripts and retained UI hooks.
use crate::{
    session::UiEvent,
    ui_cam2::{pitch_of, yaw_of, Position, Trackable},
    ui_runtime::Runtime,
    ui_vars::Variables,
};
use anyhow::Result;
use rs910_symbols::component::game_window;
use rs910_symbols::{interface, script, varbit, varc, varp};
fn angles(ui: &Runtime) -> (f32, f32) {
    let Some(Position::Entity(p)) = &ui.engine.camera.cam2.position else {
        panic!("entity camera missing")
    };
    (pitch_of(&p.rotation), yaw_of(&p.rotation))
}
fn zoom(vars: &Variables<'_>) -> i32 {
    let native910::vm::Value::Int(v) = vars
        .get(
            native910::vars::VarScope::Client,
            varc::CAMERA_ZOOM.id() as u16,
            false,
        )
        .unwrap()
    else {
        panic!("zoom type")
    };
    v
}
fn tick(ui: &mut Runtime, vars: &mut Variables<'_>) -> Result<()> {
    vars.cycle += 1;
    ui.tick(vars)
}
fn press(ui: &mut Runtime, vars: &mut Variables<'_>, code: i32, cycles: i32) -> Result<()> {
    ui.keyboard.key(code, 0, 1000);
    for _ in 0..cycles {
        tick(ui, vars)?;
    }
    ui.keyboard.key(code, 1, 1000);
    tick(ui, vars)
}
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn retail_camera_controls() -> Result<()> {
    let pack = crate::test_support::require_pack("client.config.js5");
    let defs = crate::entity_runtime::bits_pack::load(&pack)?;
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([800, 600])?;
    let mut state = crate::ui_vars::State::with_client(&pack)?;
    state.stats.as_mut().unwrap().reset_session()?;
    let mut player = crate::entities910::varps::Varps::new(defs.definitions[&0].len());
    let mut now = || 1000;
    let mut vars = Variables {
        cycle: 0,
        definitions: &defs,
        state: &mut state,
        player: Some(&mut player),
        active_player: None,
        active_npc: None,
        now: &mut now,
        probe: None,
        varp_transmit: Default::default(),
        scene: crate::ui_cam2::SceneInput {
            local_player: Some(Trackable {
                kind: 0,
                index: 1,
                level: 0,
                coord: [1640000, 1000, 1640000],
                yaw: 0,
            }),
            local_size: 1,
            ..Default::default()
        },
    };
    ui.packet(
        &mut vars,
        &UiEvent::OpenTop {
            interface_id: interface::GAME_WINDOW.id() as u32,
            keys: [0; 4],
        },
    )?;
    ui.packet(
        &mut vars,
        &UiEvent::OpenSub {
            parent_packed: game_window::WORLD_VIEW_SLOT.0,
            sub_id: interface::WORLD_VIEW.id() as u32,
            kind: 1,
            keys: [0; 4],
        },
    )?;
    ui.packet(
        &mut vars,
        &UiEvent::SetHide {
            packed: game_window::WORLD_VIEW_LAYER.0,
            hidden: false,
            flag: 0,
        },
    )?;
    for (parent, sub) in [
        (game_window::MINIMAP_SLOT, interface::MINIMAP),
        (game_window::COMPASS_SLOT, interface::COMPASS),
        (game_window::CLOCK_SLOT, interface::SYSTEM_CLOCK),
        (game_window::BACKPACK_SLOT, interface::BACKPACK),
    ] {
        ui.packet(
            &mut vars,
            &UiEvent::OpenSub {
                parent_packed: parent.0,
                sub_id: sub.id() as u32,
                kind: 1,
                keys: [0; 4],
            },
        )?;
    }
    vars.set_bit(varbit::INTERFACE_LAYOUT_LOCK.id() as u16, false, 1)?;
    vars.varp_transmit.num = 1;
    vars.varp_transmit.ids[0] = varp::INTERFACE_LAYOUT.id();
    tick(&mut ui, &mut vars)?;
    ui.packet(
        &mut vars,
        &UiEvent::Camera {
            opcode: crate::proto::server::CAM2_ENABLE,
            bytes: vec![1],
        },
    )?;
    ui.packet(
        &mut vars,
        &UiEvent::Camera {
            opcode: crate::proto::server::CAMERA_UPDATE,
            bytes: vec![1],
        },
    )?;
    tick(&mut ui, &mut vars)?;
    assert_eq!(ui.engine.camera.cam2.camera_state, 3);
    assert_eq!(ui.engine.camera.cam2.control_mode, 1);
    let start = angles(&ui);
    assert!((start.0 - 1983. * std::f32::consts::TAU / 16384.).abs() < 0.001);
    assert_eq!(zoom(&vars), 4730);
    // Synchronous onop, first delay and held repeat, release and opposite directions.
    press(&mut ui, &mut vars, 37, 10)?;
    let left = angles(&ui);
    assert!(left.1 > start.1 + 0.4);
    let released = angles(&ui);
    for _ in 0..5 {
        tick(&mut ui, &mut vars)?;
    }
    assert_eq!(angles(&ui), released);
    press(&mut ui, &mut vars, 39, 10)?;
    assert!(angles(&ui).1 < left.1 - 0.4);
    let before = angles(&ui);
    press(&mut ui, &mut vars, 38, 10)?;
    assert!(angles(&ui).0 > before.0 + 0.3);
    let before = angles(&ui);
    press(&mut ui, &mut vars, 40, 10)?;
    assert!(angles(&ui).0 < before.0 - 0.3);
    ui.keyboard.key(16, 0, 1000);
    let before = angles(&ui);
    press(&mut ui, &mut vars, 37, 10)?;
    assert_eq!(angles(&ui), before);
    ui.keyboard.key(16, 1, 1000);
    press(&mut ui, &mut vars, 38, 100)?;
    assert!(angles(&ui).0 <= 3500. * std::f32::consts::TAU / 16384. + 0.001);
    press(&mut ui, &mut vars, 40, 100)?;
    assert!(angles(&ui).0 >= 9. * std::f32::consts::TAU / 16384.);
    let before = zoom(&vars);
    press(&mut ui, &mut vars, 33, 10)?;
    assert!(zoom(&vars) < before);
    let before = zoom(&vars);
    press(&mut ui, &mut vars, 34, 10)?;
    assert!(zoom(&vars) > before);
    // This isolated UI fixture still has the full-screen editor open.
    // Close it through its retail script before testing scene mouse hooks.
    ui.packet(
        &mut vars,
        &UiEvent::RunScript(crate::session::RunClientScript {
            script_id: script::CLOSE_PARENT_WINDOW.id(),
            args: vec![crate::session::ScriptArg::Int(0)],
        }),
    )?;
    ui.paint(vars.cycle, true, [0.; 3])?;
    // Middle-button camera hooks see logical mouse coordinates, once per cycle.
    ui.engine.platform.mouse = [400, 200];
    ui.input.middle_held = true;
    tick(&mut ui, &mut vars)?;
    let before = angles(&ui);
    ui.engine.platform.mouse = [420, 210];
    tick(&mut ui, &mut vars)?;
    assert!(
        angles(&ui).0 > before.0,
        "mouse {:?} -> {:?}; errors {:?}",
        before,
        angles(&ui),
        ui.diagnostics.errors
    );
    assert!(angles(&ui).1 > before.1);
    ui.input.middle_held = false;
    let before = angles(&ui);
    ui.engine.platform.mouse = [440, 220];
    tick(&mut ui, &mut vars)?;
    assert_eq!(angles(&ui), before);
    let before = zoom(&vars);
    ui.input.wheel = 10;
    tick(&mut ui, &mut vars)?;
    assert_eq!(zoom(&vars), before + 490);
    ui.input.wheel = -1000;
    tick(&mut ui, &mut vars)?;
    assert_eq!(zoom(&vars), 1860);
    ui.input.wheel = 1000;
    tick(&mut ui, &mut vars)?;
    assert_eq!(zoom(&vars), 7600);
    // Script writes never trigger camera-update callbacks and reset the zoom.
    tick(&mut ui, &mut vars)?;
    assert_eq!(zoom(&vars), 7600);
    // Console input and focus loss must not leave an active camera key.
    ui.input.console_open = true;
    let before = angles(&ui);
    press(&mut ui, &mut vars, 37, 10)?;
    assert_eq!(angles(&ui), before);
    ui.input.console_open = false;
    ui.keyboard.key(37, 0, 1000);
    tick(&mut ui, &mut vars)?;
    ui.keyboard.focus_lost(1000);
    let before = angles(&ui);
    for _ in 0..5 {
        tick(&mut ui, &mut vars)?;
    }
    assert_eq!(angles(&ui), before);
    ui.packet(
        &mut vars,
        &UiEvent::Camera {
            opcode: crate::proto::server::CAMERA_UPDATE,
            bytes: vec![0],
        },
    )?;
    let before = angles(&ui);
    press(&mut ui, &mut vars, 37, 10)?;
    assert_eq!(angles(&ui), before);
    ui.packet(
        &mut vars,
        &UiEvent::Camera {
            opcode: crate::proto::server::CAM2_ENABLE,
            bytes: vec![0],
        },
    )?;
    assert_eq!(ui.engine.camera.cam2.camera_state, 2);
    assert_eq!(ui.diagnostics.failures, 0, "{:?}", ui.diagnostics.errors);
    Ok(())
}
