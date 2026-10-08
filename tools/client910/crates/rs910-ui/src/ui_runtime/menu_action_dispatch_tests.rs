use super::*;
use crate::proto::client as cp;

const COINS: i64 = rs910_symbols::obj::COINS.id() as i64;

fn entry(action: i32, entity_id: i64, tile: [i32; 2]) -> crate::ui_minimenu::Entry {
    crate::ui_minimenu::Entry {
        op: "Op".into(),
        target: Some(String::new()),
        cursor: -1,
        action,
        obj_id: -1,
        entity_id,
        tile_x: tile[0],
        tile_z: tile[1],
        enabled: true,
        has_arrow: false,
        sub_id: 0,
        force_submenu: false,
        detail: None,
    }
}

fn runtime() -> anyhow::Result<Runtime> {
    let pack = crate::cache::Pack::open(
        rs910_core::test_support::client_dir().join("../../server/data/pack"),
    );
    let mut ui = Runtime::new(pack)?;
    ui.engine.camera.cam2.scene.base = [3200 << 9, 3264 << 9];
    ui.engine.camera.cam2.scene.local_player = Some(crate::ui_cam2::Trackable {
        kind: crate::ui_cam2::TRACKABLE_PLAYER,
        index: 2,
        level: 0,
        coord: [3210 << 9, 0, 3270 << 9],
        yaw: 0,
    });
    ui.engine.scene.player_routes.insert(2, [10, 6]);
    ui.engine.scene.player_routes.insert(7, [11, 12]);
    ui.engine.scene.npc_routes.insert(33, [21, 22]);
    ui.state.interaction.target = crate::ui_interaction::Target {
        active: true,
        parent: 0x0102_0304,
        child: 0x1234,
        object: 0x5678,
        mask: 0x7f,
        ..Default::default()
    };
    Ok(ui)
}

/// Every packet-emitting `useMenuOption` branch,
/// including the `>= 2000` right-click offset: the emitted
/// opcode, the registry's fixed payload size, the cross mode and the
/// minimap flag owner.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn every_world_action_emits_its_packet() -> anyhow::Result<()> {
    let loc = crate::ui_scene_options::encode_loc_id(
        rs910_symbols::loc::CLASSIC_TREE.id(),
        4,
        5,
        10,
        1,
        false,
        false,
    );
    let opplayer = [117, 62, 76, 102, 49, 4, 54, 48, 6, 91];
    /// (menu action, entity, opcode, cross, minimap flag).
    type Row = (i32, i64, u8, i32, Option<[i32; 2]>);
    let mut table: Vec<Row> = Vec::new();
    for (i, &opcode) in opplayer.iter().enumerate() {
        table.push((44 + i as i32, 7, opcode, 2, Some([11, 12])));
    }
    for (action, opcode) in [
        (3, cp::OPLOC1),
        (4, cp::OPLOC2),
        (5, cp::OPLOC3),
        (6, cp::OPLOC4),
        (1001, cp::OPLOC5),
        (1002, cp::OPLOC6),
    ] {
        table.push((action, loc, opcode, 2, Some([4, 5])));
    }
    for (action, opcode) in [
        (9, cp::OPNPC1),
        (10, cp::OPNPC2),
        (11, cp::OPNPC3),
        (12, cp::OPNPC4),
        (13, cp::OPNPC5),
        (1003, cp::OPNPC6),
        (2010, cp::OPNPC2),
        (3003, cp::OPNPC6),
    ] {
        table.push((action, 33, opcode, 2, Some([21, 22])));
    }
    for (action, opcode) in [
        (18, cp::OPOBJ1),
        (19, cp::OPOBJ2),
        (20, cp::OPOBJ3),
        (21, cp::OPOBJ4),
        (22, cp::OPOBJ5),
        (1004, cp::OPOBJ6),
    ] {
        table.push((action, COINS, opcode, 2, Some([4, 5])));
    }
    table.push((2, loc, cp::OPLOCT, 2, Some([4, 5])));
    table.push((8, 33, cp::OPNPCT, 2, Some([21, 22])));
    table.push((17, COINS, cp::OPOBJT, 2, Some([4, 5])));
    table.push((15, 7, cp::OPPLAYERT, 2, Some([11, 12])));
    table.push((16, 0, cp::OPPLAYERT, 2, None));
    table.push((59, 0, cp::APCOORDT, 1, Some([4, 5])));
    table.push((60, 0, cp::FACE_SQUARE, 1, None));
    let mut ui = runtime()?;
    let target = ui.state.interaction.target.clone();
    for (action, entity, opcode, cross, flag) in table {
        ui.engine.outgoing.clear();
        ui.engine.menu.cross = Cross::default();
        ui.engine.minimap.flag = None;
        ui.state.interaction.target = target.clone();
        ui.state.interaction.actions.clear();
        ui.use_menu_option(&entry(action, entity, [4, 5]), 40, 50, false);
        let size = cp::size(opcode).expect("registered client prot");
        assert_eq!(ui.engine.outgoing.first(), Some(&opcode), "action {action}");
        assert_eq!(
            ui.engine.outgoing.len() as i32,
            size + 1,
            "action {action} framing"
        );
        assert_eq!(ui.engine.menu.cross.mode, cross, "action {action} cross");
        assert_eq!(ui.engine.minimap.flag, flag, "action {action} flag");
        assert!(
            ui.engine.menu.gaps.is_empty(),
            "action {action}: {:?}",
            ui.engine.menu.gaps
        );
        //  clears target mode after every non-select action.
        assert!(matches!(
            ui.state.interaction.actions.back(),
            Some(crate::ui_interaction::Action::ClearTarget)
        ));
    }
    Ok(())
}

/// OPNPC/OPNPCT only send for an NPC still in `npcs`
/// and interface/world-map actions reach
/// their original consumers instead of a packet writer.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn non_packet_actions_reach_their_consumers() -> anyhow::Result<()> {
    let mut ui = runtime()?;
    for action in [9, 1003, 8] {
        ui.use_menu_option(&entry(action, 99, [0, 0]), 1, 2, false);
    }
    assert!(ui.engine.outgoing.is_empty());
    use crate::ui_interaction::Action;
    let mut ui = runtime()?;
    ui.state.interaction.actions.clear();
    let mut op = entry(57, 3, [7, 0x0102_0003]);
    ui.use_menu_option(&op, 0, 0, false);
    op.action = 1007;
    ui.use_menu_option(&op, 0, 0, false);
    ui.use_menu_option(&entry(58, 0, [7, 0x0102_0003]), 0, 0, false);
    ui.use_menu_option(&entry(30, 0, [7, 0x0102_0003]), 0, 0, false);
    ui.use_menu_option(&entry(1010, 44, [5, 0]), 0, 0, false);
    // 1008..=1012 queue the world-map element trigger `action - 998`.
    ui.use_menu_option(&entry(1012, 321, [17, 0]), 40, 50, false);
    let kinds: Vec<&'static str> = ui
        .state
        .interaction
        .actions
        .iter()
        .map(|a| match a {
            Action::Op { .. } => "op",
            Action::Target { .. } => "target",
            Action::Pause { .. } => "pause",
            Action::MapElementTrigger {
                trigger: 12,
                element: 44,
                category: 5,
                ..
            } => "map",
            Action::MapElementTrigger {
                trigger: 14,
                element: 321,
                category: 17,
                mouse: None,
            } => "map14",
            Action::ClearTarget => "clear",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "op", "clear", "op", "clear", "target", "clear", "pause", "clear", "map", "clear",
            "map14", "clear"
        ]
    );
    // 25 selects a target and returns before the tail.
    ui.state.interaction.actions.clear();
    ui.use_menu_option(&entry(25, 0, [7, 0x0102_0003]), 0, 0, false);
    assert!(matches!(
        ui.state.interaction.actions.iter().collect::<Vec<_>>()[..],
        [Action::Select {
            parent: 0x0102_0003,
            child: 7
        }]
    ));
    // Cancel and the disabled other-level player row (-1) have
    // no original branch: only the tail runs and nothing is reported.
    ui.state.interaction.actions.clear();
    for action in [1006, -1] {
        ui.use_menu_option(&entry(action, 0, [0, 0]), 0, 0, false);
    }
    assert!(ui.engine.outgoing.is_empty());
    assert!(ui.engine.menu.gaps.is_empty());
    Ok(())
}

/// OPLOC6 (loc Examine, ->) carries the loc id and
/// absolute tile exactly like OPLOC1.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn loc_examine_writes_oploc6_payload() -> anyhow::Result<()> {
    let mut ui = runtime()?;
    let loc = crate::ui_scene_options::encode_loc_id(0x0102_0304, 4, 5, 10, 1, false, false);
    ui.use_menu_option(&entry(1002, loc, [4, 5]), 0, 0, false);
    let z = 3264 + 5;
    let x = 3200 + 4;
    assert_eq!(
        ui.engine.outgoing,
        vec![
            cp::OPLOC6,
            0,
            (z >> 8) as u8,
            z as u8,
            1,
            2,
            3,
            4,
            (x as u8).wrapping_add(128),
            (x >> 8) as u8
        ]
    );
    Ok(())
}
