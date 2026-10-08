use super::*;

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn player_target_menu_actions_reach_opplayert() -> anyhow::Result<()> {
    let pack = crate::cache::Pack::open(
        rs910_core::test_support::client_dir().join("../../server/data/pack"),
    );
    let mut ui = Runtime::new(pack)?;
    ui.engine.scene.player_routes.insert(7, [321, 322]);
    ui.state.interaction.target = crate::ui_interaction::Target {
        active: true,
        parent: 0x0102_0304,
        child: 0x1234,
        object: 0x5678,
        ..Default::default()
    };
    let mut entry = crate::ui_minimenu::Entry {
        op: "Use".into(),
        target: None,
        cursor: -1,
        action: 15,
        obj_id: -1,
        entity_id: 7,
        tile_x: 0,
        tile_z: 0,
        enabled: true,
        has_arrow: false,
        sub_id: -1,
        force_submenu: false,
        detail: None,
    };
    ui.use_menu_option(&entry, 40, 50, false);
    assert_eq!(
        ui.engine.outgoing,
        vec![41, 0x12, 0x34, 128, 0x78, 0x56, 135, 0, 3, 4, 1, 2]
    );
    assert_eq!(
        ui.engine.menu.cross,
        Cross {
            x: 40,
            y: 50,
            mode: 2,
            cycle: 0
        }
    );
    assert_eq!(ui.engine.minimap.flag, Some([321, 322]));

    ui.engine.outgoing.clear();
    ui.engine.scene.player_routes.insert(2, [12, 13]);
    ui.engine.camera.cam2.scene.local_player = Some(crate::ui_cam2::Trackable {
        kind: crate::ui_cam2::TRACKABLE_PLAYER,
        index: 2,
        level: 0,
        coord: [0, 0, 0],
        yaw: 0,
    });
    entry.action = 16;
    entry.entity_id = 99;
    ui.use_menu_option(&entry, 40, 50, false);
    assert_eq!(ui.engine.outgoing[6], 130);
    assert_eq!(ui.engine.outgoing[7], 0);
    assert_eq!(ui.engine.outgoing[8], 3);
    // The self target sets no minimap flag.
    assert_eq!(ui.engine.minimap.flag, Some([321, 322]));
    Ok(())
}
