//! Camera command and local-player cases with their entity fixtures.

use super::super::ActiveEntity;

use rs910_core::fault::Fault;

use super::{c, error_fault, rec, Case, Obs, Set, World, C0, IFACE};

/// Legacy camera, cutscene splines and the cam2 commands.
pub(super) fn camera() -> Vec<Case> {
    let spline0 = Set::Spline(0, 2);
    let spline1 = Set::Spline(1, 2);
    vec![
        rec("cam_getangle_xa").set(Set::Orbit(2400, 1000)),
        rec("cam_getangle_ya").set(Set::Orbit(2400, 1000)),
        rec("cam_getfollowheight").set(Set::Follow(300)),
        rec("cam_setfollowheight").i(&[100]).obs(Obs::Follow),
        rec("cam_setfollowheight").i(&[-4]).obs(Obs::Follow),
        rec("cam_inc_x").obs(Obs::OrbitVel),
        rec("cam_dec_x").obs(Obs::OrbitVel),
        rec("cam_inc_y").obs(Obs::OrbitVel),
        rec("cam_dec_y").obs(Obs::OrbitVel),
        rec("cam_removeroof").i(&[-1]).obs(Obs::Roof),
        rec("spline_new").i(&[0, 2]).obs(Obs::Spline(0)),
        rec("spline_new").i(&[2, 2]).obs(Obs::Spline(0)).note("index 2 is ignored"),
        rec("spline_length").set(spline0).i(&[0]),
        rec("spline_length").i(&[1]).note("null spline: missing value"),
        rec("spline_addpoint")
            .set(spline0)
            .i(&[0, 1, (3204 << 14) | 3202, 100, (3205 << 14) | 3202, 90, 7])
            .obs(Obs::Spline(0)),
        rec("cam_movealong")
            .set(spline0)
            .set(spline1)
            .i(&[0, 0, 100, 200, 1, 0])
            .obs(Obs::Camera)
            .obs(Obs::MoveAlong),
        rec("cam_movealong")
            .set(spline0)
            .set(spline1)
            .i(&[0, 1, 100, 200, 1, 0])
            .note("keyframe 1 + 1 >= 2 keyframes: invalid state"),
        rec("cam_movealong").i(&[2, 0, 100, 200, 1, 0]),
        // cameraForceAngle (state != 3): pitch 300 << 3
        // = 2400 (inside clampCamera's 1077..2787), yaw 2050 << 3 = 16400
        // wraps to 16.
        c("cam_forceangle").i(&[300, 2050]).check(|w| {
            let h = &w.engine.game_host;
            (h.orbit_force == Some((2400.0, 16.0)) && (h.orbit_pitch, h.orbit_yaw) == (2400.0, 16.0))
                .then_some(())
                .ok_or(format!("{:?} {} {}", h.orbit_force, h.orbit_pitch, h.orbit_yaw))
        }),
        // cam_followcoord with world base, size
        // 104: tile (110, -50) clamps to (104, 0) → (104 << 9) + 256, 256;
        // cameraState 4, server roof cleared.
        c("cam_followcoord")
            .with(|w| {
                w.engine.game_host.base = [3200, 3200];
                w.engine.game_host.size = [104, 104];
            })
            .i(&[(3310 << 14) | 3150])
            .wo(Obs::Camera, "4,-1,-1")
            .check(|w| {
                let f = w.engine.camera.cam2.legacy.follow_coord;
                (f == Some([(104 << 9) + 256, 256])).then_some(()).ok_or(format!("{f:?}"))
            }),
        // cam_removeroof: - base → roof at tile
        // (10, 11) centres.
        c("cam_removeroof")
            .with(|w| {
                w.engine.game_host.base = [3200, 3200];
                w.engine.game_host.size = [104, 104];
            })
            .i(&[(3210 << 14) | 3211])
            .wo(Obs::Roof, &format!("{},{}", (10 << 9) + 256, (11 << 9) + 256)),
        // cam_moveto → cameraMoveTo(5, 6, 10 << 2, 1, 2)
        // cameraState 5, roof cleared.
        c("cam_moveto")
            .with(|w| w.engine.game_host.base = [3200, 3200])
            .i(&[(3205 << 14) | 3206, 10, 1, 2])
            .wo(Obs::Camera, "5,-1,-1")
            .check(|w| {
                let m = w.engine.camera.cam2.legacy.move_to;
                (m == Some(crate::ui_cam2::LegacyMove {
                    x: 5,
                    z: 6,
                    source_height: 40,
                    acceleration: 1,
                    speed: 2,
                }))
                .then_some(())
                .ok_or(format!("{m:?}"))
            }),
        // cam_lookat → cameraLookAt(5, 6, 40, 1, 2)
        // cameraState 5, roof cleared.
        c("cam_lookat")
            .with(|w| w.engine.game_host.base = [3200, 3200])
            .i(&[(3205 << 14) | 3206, 10, 1, 2])
            .wo(Obs::Camera, "5,-1,-1")
            .check(|w| {
                let l = w.engine.camera.cam2.legacy.look_at;
                (l == Some(crate::ui_cam2::LegacyLook {
                    x: 5,
                    z: 6,
                    height: 40,
                    acceleration: 1,
                    speed: 2,
                }))
                .then_some(())
                .ok_or(format!("{l:?}"))
            }),
        // cam_reset resets to the default camera state (2 without a graphics
        // defaults override).
        c("cam_reset")
            .set(Set::CamState(5))
            .wo(Obs::Camera, "2,-1,-1"),
        // cam_smoothreset → cameraSmoothReset: state 1.
        c("cam_smoothreset")
            .set(Set::CamState(5))
            .wo(Obs::Camera, "1,-1,-1"),
        // cam2_setpositionpointcollision.
        c("cam2_setpositionpointcollision")
            .with(|w| cam2_mode(w, "cam2_setpositionmode", 0))
            .i(&[1])
            .check(|w| {
                matches!(&w.engine.camera.cam2.position, Some(crate::ui_cam2::Position::Point(p)) if p.collision)
                    .then_some(())
                    .ok_or("collision not set".into())
            }),
        c("cam2_setpositionpointcollision")
            .with(|w| cam2_mode(w, "cam2_setpositionmode", 2))
            .i(&[1])
            .throws(Fault::InvalidState),
        // cam2_setpositionspline_spline (spline position mode).
        c("cam2_setpositionspline_spline")
            .with(|w| cam2_mode(w, "cam2_setpositionmode", 2))
            .check(|w| {
                w.engine
                    .camera.cam2
                    .position
                    .as_ref()
                    .is_some_and(|p| {
                        // The appended spline is absent: reading its point is a
                        // missing-value fault.
                        p.initialised()
                            && p.point().is_err_and(|e| {
                                error_fault(&format!("trap: {e}")) == Some(Fault::MissingValue)
                            })
                    })
                    .then_some(())
                    .ok_or("spline position not initialised with the null spline".into())
            }),
        c("cam2_setpositionspline_spline")
            .with(|w| cam2_mode(w, "cam2_setpositionmode", 0))
            .throws(Fault::InvalidState),
        // cam2_setlookatspline_spline: POINT (0) is not a spline lookat:
        // invalid state.
        c("cam2_setlookatspline_spline")
            .with(|w| cam2_mode(w, "cam2_setlookatmode", 0))
            .throws(Fault::InvalidState),
        // Mode 5 passes the spline-lookat check but is not a spline lookat:
        // the downcast fails.
        c("cam2_setlookatspline_spline")
            .with(|w| cam2_mode(w, "cam2_setlookatmode", 5))
            .throws(Fault::WrongValueType),
        // ORIENTATION (3) is not a spline lookat either, so the clamp setter
        // on a spline lookat (mode 2) throws.
        c("cam2_setlookatorientation_maxdistanceclamping")
            .with(|w| cam2_mode(w, "cam2_setlookatmode", 2))
            .i(&[1, 2, -1, 4, 5, 6])
            .s(&[&crate::ui_cam2::encode_coord_fine(0, [1000, -200, 3000])])
            .throws(Fault::InvalidState),
        // cam2_setlookatorientation_maxdistanceclamping.
        c("cam2_setlookatorientation_maxdistanceclamping")
            .with(|w| cam2_mode(w, "cam2_setlookatmode", 3))
            .i(&[1, 2, -1, 4, 5, 6])
            .s(&[&crate::ui_cam2::encode_coord_fine(0, [1000, -200, 3000])])
            .check(|w| match &w.engine.camera.cam2.lookat {
                Some(crate::ui_cam2::Focus::Orientation(o)) => (o.clamps
                    == [488.0, 2024.0, -2360.0, 3272.0, -1.0, 5048.0])
                    .then_some(())
                    .ok_or(format!("{:?}", o.clamps)),
                other => Err(format!("{other:?}")),
            }),
    ]
}

pub(super) fn cam2_mode(w: &mut World, command: &str, mode: i32) {
    let mut ints = vec![mode];
    w.engine
        .camera
        .cam2
        .dispatch_active_with_objects(command, &mut ints, &mut vec![], None)
        .unwrap()
        .unwrap();
}

/// The local player and its appearance model.
pub(super) fn local_player() -> Vec<Case> {
    fn model(w: &mut World) {
        w.engine.game_host.local_angle = Some(0x1234);
        w.engine.game_host.local_model = Some(crate::entities910::appearance::Model {
            bas: -1,
            kits: vec![0; 12],
            custom: vec![None; 12],
            colours: [0; 10],
            textures: [0; 10],
            female: false,
            npc: -1,
            hash: 0,
        });
    }
    fn m(w: &World) -> crate::entities910::appearance::Model {
        w.engine.game_host.local_model.clone().unwrap()
    }
    vec![
        // The local player is absent before login: missing value.
        rec("gender"),
        rec("setgender").i(&[1]),
        rec("basecolour").i(&[3, 7]),
        rec("basematerial").i(&[2, 9]),
        rec("baseidkit").i(&[9, 42]),
        rec("setobj").i(&[5, 1234]),
        rec("get_selfyangle"),
        rec("facing_fine"),
        rec("coord"),
        // setIDKRecolourSlot.
        c("basecolour").with(model).i(&[3, 7]).check(|w| {
            let m = m(w);
            (m.colours[3] == 7 && w.engine.game_host.model_dirty)
                .then_some(())
                .ok_or(format!("{:?}", m.colours))
        }),
        // setIDKRematerialSlot.
        c("basematerial").with(model).i(&[2, 9]).check(|w| {
            let m = m(w);
            (m.textures[2] == 9)
                .then_some(())
                .ok_or(format!("{:?}", m.textures))
        }),
        // Kit 9 in slot 2 goes to wear slot 4: kit ids[4] = 42 | MIN_VALUE.
        c("baseidkit").with(model).i(&[9, 42]).check(|w| {
            let m = m(w);
            (m.kits[4] == 42 | i32::MIN)
                .then_some(())
                .ok_or(format!("{:?}", m.kits))
        }),
        // setGender.
        c("setgender")
            .with(model)
            .i(&[1])
            .check(|w| m(w).female.then_some(()).ok_or("not female".into())),
        // setObject: kitIds[5] = 1234 | 0x40000000.
        c("setobj").with(model).i(&[5, 1234]).check(|w| {
            let m = m(w);
            (m.kits[5] == 1234 | 0x4000_0000)
                .then_some(())
                .ok_or(format!("{:?}", m.kits))
        }),
        c("gender")
            .with(|w| {
                model(w);
                w.engine.game_host.local_model.as_mut().unwrap().female = true;
            })
            .wi(&[1]),
        // The facing angle >> 3 / the fine angle itself.
        c("get_selfyangle").with(model).wi(&[0x1234 >> 3]),
        c("facing_fine").with(model).wi(&[0x1234]),
        // pack of the local player at tile, level 1.
        c("coord")
            .with(|w| {
                w.engine.camera.cam2.scene.local_player = Some(crate::ui_cam2::Trackable {
                    kind: 0,
                    index: 1,
                    level: 1,
                    coord: [3222 * 512 + 256, 0, 3218 * 512 + 256],
                    yaw: 0,
                })
            })
            .wi(&[(1 << 28) | (3222 << 14) | 3218]),
        // opplayer: OPPLAYER2 p2(index) p1_alt1(0).
        c("opplayer")
            .with(|w| {
                w.props.game.players = vec![
                    (3, Some("Local".into()), [1, 1]),
                    (9, Some("Eve".into()), [20, 21]),
                ];
                w.props.game.local_player = Some(3);
            })
            .i(&[2])
            .s(&["eve"])
            .wo(
                Obs::Out,
                &format!("{:02x}0009{:02x}", crate::proto::client::OPPLAYER2, 128),
            ),
        // The local player is skipped → "Unable to find " + name.
        c("opplayer")
            .with(|w| {
                w.props.game.players = vec![(3, Some("Local".into()), [1, 1])];
                w.props.game.local_player = Some(3);
            })
            .i(&[1])
            .s(&["local"])
            .wo(Obs::Out, "")
            .check(|w| {
                (w.props.game.system_messages == [(4, "Unable to find local".to_owned())])
                    .then_some(())
                    .ok_or(format!("{:?}", w.props.game.system_messages))
            }),
        // opplayert with target mask 0x10 on the local
        // player: OPPLAYERT, minimap flag at its first waypoint, target
        // mode cleared.
        c("opplayert")
            .with(|w| {
                Set::Iface(IFACE, 2).apply(w, false);
                w.props.game.players = vec![(3, Some("Local".into()), [1, 1])];
                w.props.game.local_player = Some(3);
                let t = &mut w.props.interaction.target;
                t.active = true;
                t.mask = 0x10;
                t.parent = C0;
                t.child = 4;
                t.object = rs910_symbols::obj::COINS.id();
            })
            .s(&["Local"])
            .check(|w| {
                let out = &w.props.interaction.outgoing;
                // p2(child 4) p1_alt1(0) p2_alt1 p2_alt3(index 3) p4_alt2(parent).
                let want = [
                    crate::proto::client::OPPLAYERT,
                    0,
                    4,
                    128,
                    rs910_symbols::obj::COINS.id() as u16 as u8,
                    (rs910_symbols::obj::COINS.id() >> 8) as u8,
                    3 + 128,
                    0,
                    (C0 >> 8) as u8,
                    C0 as u8,
                    (C0 >> 24) as u8,
                    (C0 >> 16) as u8,
                ];
                (out[..] == want[..]
                    && w.props.game.minimap_flag == Some([1, 1])
                    && !w.props.interaction.target.active)
                    .then_some(())
                    .ok_or(format!("{out:02x?} {:?}", w.props.game.minimap_flag))
            }),
        // targetModeActive false → return before any lookup.
        c("opplayert").s(&["Local"]).wo(Obs::Out, ""),
        // getActiveMiniMenuEntry() is null → entity type 0 → 0.
        rec("npc_find_active_minimenu_entry"),
        rec("player_find_active_minimenu_entry"),
        // An active NPC (type 4) / player (type 7) entry installs the
        // entity as activeEntity and pushes 1.
        c("npc_find_active_minimenu_entry")
            .with(|w| w.engine.game_host.minimenu_entity = Some((4, npc_entity())))
            .wi(&[1])
            .check(|w| {
                matches!(
                    w.engine.scene.active_entity,
                    Some(ActiveEntity::Npc { index: 8, .. })
                )
                .then_some(())
                .ok_or("active entity not installed".into())
            }),
        c("player_find_active_minimenu_entry")
            .with(|w| w.engine.game_host.minimenu_entity = Some((4, npc_entity())))
            .wi(&[0])
            .note("an NPC entry is not a player entry"),
        // getNameWithExtras(true): title with the
        // <name> slot replaced.
        c("get_displayname_withextras")
            .with(|w| {
                w.engine.scene.active_entity =
                    Some(player_entity(Some("<col=ff0000>Sir</col> <name>")))
            })
            .ws(&["<col=ff0000>Sir</col> Zezima"]),
        c("get_displayname_withextras")
            .with(|w| w.engine.scene.active_entity = Some(player_entity(None)))
            .ws(&["Zezima"]),
        c("get_displayname_withextras")
            .with(|w| w.engine.scene.active_entity = Some(npc_entity()))
            .throws(Fault::WrongValueType),
        // opcount reads the executed-instruction counter
        // (incremented before each fetch): the command is the 4th
        // instruction after the three sentinel pushes.
        c("opcount").wi(&[4]),
        c("opcount").i(&[1, 2]).wi(&[1, 2, 6]),
    ]
}

pub(super) fn npc_entity() -> ActiveEntity {
    ActiveEntity::Npc {
        index: 8,
        type_id: 3,
        name: "Man".into(),
        chat: None,
        stats: [0; 6],
        stat_max: [0; 6],
        vislevel: 0,
        active: true,
        overlay_height: 0,
        target: -1,
        position: [0.0; 3],
        screen_bounds: None,
    }
}

pub(super) fn player_entity(title: Option<&str>) -> ActiveEntity {
    ActiveEntity::Player {
        index: 1,
        name: "Zezima".into(),
        title: title.map(str::to_owned),
        chat: None,
        overlay_height: 0,
        target: -1,
        position: [0.0; 3],
        screen_bounds: None,
    }
}
