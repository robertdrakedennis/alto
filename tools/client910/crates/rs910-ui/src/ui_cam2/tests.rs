use super::*;

fn player(coord: [i32; 3], yaw: i32) -> Trackable {
    Trackable {
        kind: TRACKABLE_PLAYER,
        index: 7,
        level: 0,
        coord,
        yaw,
    }
}
/// The free camera: the arrow key 98 pushes the point target 25 units along
/// the (identity) orientation and the infinite-acceleration point owner
/// reaches it.
#[test]
fn free_camera_moves_with_arrow_keys() {
    let scene = Scene::default();
    let mut free = FreeCamera::create(0, [1000, -500, 2000], [10, 10], &scene);
    free.handle_orbit_input(Some([512, 334]), [10, 10], false, &|k| k == 98, &scene)
        .unwrap();
    let eye = free.camera.eye().unwrap();
    assert_eq!(
        [eye.x as i32, eye.y as i32, eye.z as i32],
        [1000, -500, 2025]
    );
    // setFieldOfView(atan(w / 2 / 1000) * 2, ..).
    assert!((free.camera.fov[0] - ((256.0_f64 / 1000.0).atan() * 2.0) as f32).abs() < 1e-6);
    assert!(free.camera.frame().is_some());
}

/// Smooth reset and its transition with the orbit default: the cubic ease from
/// the saved pose,
/// yaw through the wrapped delta, and the default state after 100 cycles.
#[test]
fn legacy_smooth_reset_transition_eases_to_the_orbit() {
    let mut c = Cam2::new(false);
    c.legacy.loop_cycle = 0;
    c.legacy.pose = LegacyPose {
        x: 0,
        y: 0,
        z: 0,
        pitch: 1000,
        yaw: 0,
        roll: 0,
    };
    c.legacy.zoom = 500;
    c.camera_smooth_reset();
    assert_eq!(c.camera_state, 1);
    c.legacy.loop_cycle = 50;
    let orbit = LegacyPose {
        x: 1000,
        y: -800,
        z: 2000,
        pitch: 2000,
        yaw: 4000,
        roll: 0,
    };
    assert!(c.update_transition(orbit, 800, 600));
    // eased = 1 - 50^3 / 1e6 = 0.875.
    let p = c.legacy.pose;
    assert_eq!(
        [p.x, p.y, p.z, p.pitch, p.yaw],
        [875, -700, 1750, 1875, 3500]
    );
    assert_eq!(c.legacy.zoom, 587);
    c.legacy.loop_cycle = 100;
    assert!(!c.update_transition(orbit, 800, 600));
    assert_eq!(c.camera_state, 2);
}

#[test]
fn sync_scene_resolves_live_npc_trackable_coordinates() {
    let map = crate::protocol910::Context {
        local: 0,
        base_x: 100,
        base_z: 200,
        width: 104,
        height: 104,
        bridges: vec![],
    };
    let players = crate::protocol910::Players::default();
    let mut npcs = crate::protocol910::npc::Npcs::default();
    let mut npc = crate::entities910::Npc::new([32, 3, 16, 0]);
    npc.path.tele(7, 11);
    npc.path.level = 2;
    npc.path.motion.y = 17.0;
    npc.path.angle = 1234;
    npcs.entities.insert(42, npc);

    let mut input = SceneInput::new_with_objects(&map, &players, None, None, 0);
    input.npcs = Some(&npcs);
    let mut cam = Cam2::new(false);
    cam.sync_scene(&input);

    assert_eq!(
        cam.scene.trackable(TrackableRef {
            kind: TRACKABLE_NPC,
            index: 42
        }),
        Some(Trackable {
            kind: TRACKABLE_NPC,
            index: 42,
            level: 2,
            coord: [55_040, -17, 108_288],
            yaw: 1234,
        })
    );
}
fn run(c: &mut Cam2, command: &str, ints: &[i32]) -> VmResult<Option<Value>> {
    let mut ints = ints.to_vec();
    c.dispatch(command, &mut ints).unwrap()
}
fn run_pushing(c: &mut Cam2, command: &str, ints: &[i32]) -> Vec<i32> {
    let mut ints = ints.to_vec();
    c.dispatch(command, &mut ints).unwrap().unwrap();
    ints
}

fn spline_packet(start: f32, control1: f32, control2: f32, end: f32) -> Vec<u8> {
    let mut bytes = vec![1];
    for value in [
        start,
        0.0,
        0.0,
        control1,
        0.0,
        0.0,
        0.0,
        end,
        0.0,
        0.0,
        end * 2.0 - control2,
        0.0,
        0.0,
        0.0,
    ] {
        bytes.extend_from_slice(&value.to_bits().to_be_bytes());
    }
    bytes
}

#[test]
fn coupled_lookat_spline_uses_the_live_position_parameter() {
    let mut c = Cam2::new(true);
    // Set the coupled look-at spline (mode 5) beside the linear eye spline (mode 4).
    c.decode(&[24, 5, 4]).unwrap();
    let mut lookat = spline_packet(0.0, 30.0, 130.0, 100.0);
    let mut lookat_packet = vec![32];
    lookat_packet.append(&mut lookat);
    c.decode(&lookat_packet).unwrap();
    let mut position = vec![1];
    position.extend_from_slice(&spline_packet(1_000.0, 1_030.0, 1_130.0, 1_100.0));
    position.extend_from_slice(&0.0_f32.to_bits().to_be_bytes());
    position.extend_from_slice(&0.5_f32.to_bits().to_be_bytes());
    position.extend_from_slice(&0.5_f32.to_bits().to_be_bytes());
    let mut position_packet = vec![64];
    position_packet.append(&mut position);
    c.decode(&position_packet).unwrap();

    assert!(c.ready());
    c.update(0.5).unwrap();
    let Some(Position::Spline(position)) = &c.position else {
        panic!()
    };
    assert_eq!(position.position, 0.5, "{position:?}");
    let frame = c.frame().expect("coupled spline camera is ready");
    assert!((frame.eye[0] - 1072.5).abs() < 0.01, "{:?}", frame.eye);
    assert!((frame.lookat[0] - 72.5).abs() < 0.01, "{:?}", frame.lookat);
}

/// The command sequence the retail scripts issue online (traced with
/// `CLIENT910_UI_CAM2_TRACE`), then the frame the scene draw would use.
#[test]
fn online_script_sequence_frames_the_player() {
    let mut c = Cam2::new(true);
    c.scene.base = [1622016, 1622016];
    c.scene.local_player = Some(Trackable {
        kind: TRACKABLE_PLAYER,
        index: 1,
        level: 0,
        coord: [1649920, 944, 1649920],
        yaw: 0,
    });
    for (cmd, args) in [
        ("cam2_setfieldofviewscreen", vec![1592, 1160, 820]),
        ("cam2_setpositionmode", vec![1]),
        ("cam2_setlookatmode", vec![1]),
        ("cam2_setlinearmovementmode", vec![2]),
        ("cam2_setspringproperties", vec![113, 113, 113, 1697]),
        ("cam2_setpositionangularinterpolation", vec![1000]),
        ("cam2_setcollisionmode", vec![1, 1]),
        ("cam2_setdepthplanes", vec![50, 14847]),
        ("cam2_setlookatentity_player", vec![0, 300, 0, 0]),
        (
            "cam2_setpositionentity_player",
            vec![0, 0, 4730, 1983, 8139, 0, 0],
        ),
    ] {
        run(&mut c, cmd, &args).unwrap();
    }
    for _ in 0..3 {
        c.update(0.025).unwrap();
    }
    let mut f = c.frame().unwrap();
    // Eye 4730 units behind (south of) and above the player, y down.
    assert!(
        f.eye[2] < 1649920.0 - 3000.0 && f.eye[1] < -4000.0,
        "{:?}",
        f.eye
    );
    assert_eq!(f.lookat, [1649920.0, -1244.0, 1649920.0]);
    assert!(
        f.pitch > 0.7 && f.pitch < 0.72 && f.yaw.abs() < 0.03,
        "pitch {} yaw {}",
        f.pitch,
        f.yaw
    );
    assert!(
        (f.fov[0] - 1.5410956).abs() < 1e-5 && (f.fov[1] - 1.2312398).abs() < 1e-5,
        "{:?}",
        f.fov
    );
    // As app.rs installs it: anchored on the integer look-at point.
    f.rebase([1649920, -1244, 1649920]);
    let cam = crate::camera::SceneCamera {
        cam2: Some(f.clone()),
        viewport: (1592, 1160),
        ..crate::camera::SceneCamera::new([1649920, -1244, 1649920])
    };
    assert_eq!((cam.pitch_int(), cam.yaw_int()), (1857, 52));
    // The matrix works in world units: fine units over 512, y up.
    let vp = cam.view_proj();
    let project = |p: [f32; 3]| {
        let v = vp * glam::Vec4::new(p[0] / 512.0, -p[1] / 512.0, p[2] / 512.0, 1.0);
        (v.x / v.w, v.y / v.w, v.z / v.w)
    };
    // The player is at the screen centre (just below it: the lookat is 300 above).
    let (px, py, pz) = project([1649920.0, -944.0, 1649920.0]);
    assert!(
        px.abs() < 1e-3 && py < 0.0 && py > -0.1 && (0.0..=1.0).contains(&pz),
        "{px} {py} {pz}"
    );
    // Ten tiles north is higher on screen and deeper; ten tiles east is right.
    let (_, ny, nz) = project([1649920.0, -944.0, 1649920.0 + 5120.0]);
    assert!(ny > py && nz > pz, "{ny} {nz}");
    let (ex, _, _) = project([1649920.0 + 5120.0, -944.0, 1649920.0]);
    assert!(ex > 1.0, "{ex}");
}

#[test]
fn orientation_owner_drives_camera_frame_and_rotation_commands() {
    let mut c = Cam2::new(true);
    c.position = Some(Position::Point(PointPosition::default()));
    if let Some(Position::Point(point)) = c.position.as_mut() {
        point.set(0, [1_000, 100, 2_000]);
    }
    run(&mut c, "cam2_setlookatmode", &[MODE_ORIENTATION]).unwrap();
    run(&mut c, "cam2_setlookatorientation_vector", &[0, 0, 1]).unwrap();
    let frame = c.frame().expect("orientation camera is ready");
    assert_eq!(frame.eye, [1_000.0, -100.0, 2_000.0]);
    assert_eq!(frame.lookat, [1_000.0, -100.0, 3_000.0]);
    run(&mut c, "cam2_setlookatorientation_xrotation", &[4096]).unwrap();
    c.update(0.02).unwrap();
    let rotated = c.frame().expect("rotated orientation camera is ready");
    assert!(
        rotated.lookat[1] > 899.0,
        "orientation rotation did not affect lookat: {:?}",
        rotated.lookat
    );
}

#[test]
fn guards_follow_camera_rules() {
    let mut c = Cam2::new(false);
    assert_eq!(c.camera_state, 2);
    let mut ints = vec![2];
    assert!(c
        .dispatch("cam2_setlinearmovementmode", &mut ints)
        .unwrap()
        .is_ok());
    // Acceleration setters are refused in the spring mode.
    let mut ints = vec![-1, 500];
    assert!(c
        .dispatch("cam2_setlookatacceleration", &mut ints)
        .unwrap()
        .is_err());
    let mut ints = vec![1, 2, 3, 1500];
    assert!(c
        .dispatch("cam2_setspringproperties", &mut ints)
        .unwrap()
        .is_ok());
    assert_eq!(
        (c.lookat_spring, c.position_spring_damping),
        ([1.0, 2.0, 3.0], 1.5)
    );
    let mut ints = vec![1];
    c.dispatch("cam2_setlinearmovementmode", &mut ints)
        .unwrap()
        .unwrap();
    let mut ints = vec![-1, 500];
    c.dispatch("cam2_setpositionacceleration", &mut ints)
        .unwrap()
        .unwrap();
    assert_eq!(c.position_acceleration, [f32::INFINITY; 3]);
    assert_eq!(c.position_angular_interpolation, 0.5);
    let mut ints = vec![0, 0];
    c.dispatch("cam2_setdepthplanes", &mut ints)
        .unwrap()
        .unwrap();
    assert_eq!(c.depth_planes, [50.0, 10000.0]);
    let mut ints = vec![7];
    assert!(c
        .dispatch("cam2_setpositionmode", &mut ints)
        .unwrap()
        .is_ok());
    assert_eq!(c.position_mode, None);
    let mut ints = vec![3, 100, 2000];
    let id = c
        .dispatch("cam2_addeffect_shake", &mut ints)
        .unwrap()
        .unwrap();
    assert_eq!(id, Some(Value::Int(1)));
    let mut ints = vec![1];
    assert!(c
        .dispatch("cam2_updateeffect_ztilt", &mut vec![1, 5])
        .unwrap()
        .is_err());
    c.dispatch("cam2_removeeffect", &mut ints).unwrap().unwrap();
    assert!(c.effects.is_empty());
}

#[test]
fn quaternion_layout_and_rotation_order() {
    // Quaternion.setToIdentity: scalar in z.
    assert_eq!(
        Quat::IDENTITY,
        Quat {
            w: 0.0,
            x: 0.0,
            y: 0.0,
            z: 1.0
        }
    );
    // A yaw of pi/2 about +y turns +z into +x under Vector3.rotate
    // (opposite(q) * v * q with the original client's multiply operand order).
    let q = Quat::rotation(0.0, 1.0, 0.0, std::f32::consts::FRAC_PI_2);
    let mut v = Vec3::new(0.0, 0.0, 1.0);
    v.rotate(&q);
    assert!(
        (v.x - 1.0).abs() < 1e-6 && v.y.abs() < 1e-6 && v.z.abs() < 1e-6,
        "{v:?}"
    );
    // The yaw read back is normalised to [0, 2pi).
    let yaw = yaw_of(&q);
    assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5, "{yaw}");
    assert!(pitch_of(&q).abs() < 1e-6);
    // Rotating by a quaternion and by its opposite cancel.
    let mut back = v;
    back.rotate(&Quat::opposite_of(&q));
    assert!((back.z - 1.0).abs() < 1e-6, "{back:?}");
}

#[test]
fn entity_targets_follow_the_local_player() {
    let mut c = Cam2::new(true);
    c.scene.local_player = Some(player([3200 * 512 + 256, 100, 3200 * 512 + 256], 0));
    run(&mut c, "cam2_setlookatmode", &[MODE_ENTITY]).unwrap();
    run(&mut c, "cam2_setpositionmode", &[MODE_ENTITY]).unwrap();
    assert!(!c.ready());
    // Wrong mode: an invalid-state fault.
    assert!(run(&mut c, "cam2_setlookatpoint_point", &[]).is_err());
    run(&mut c, "cam2_setlookatentity_player", &[0, 200, 0, 0]).unwrap();
    // pitch 1024 units (22.5 deg) looking from 1200 units behind.
    run(
        &mut c,
        "cam2_setpositionentity_player",
        &[0, 0, -1200, 1024, 0, 0, 200],
    )
    .unwrap();
    assert!(c.changed);
    // The angle offsets read back what the script wrote,
    // less the truncating `(int)` cast of the float round trip.
    let angles = run_pushing(&mut c, "cam2_getpositionentity_angleoffsets", &[]);
    assert_eq!(angles, vec![1023, 0]);
    // First update: NaN current snaps onto the target in the camera-axis law.
    c.update(0.02).unwrap();
    assert!(c.ready());
    let Some(Focus::Entity(l)) = &c.lookat else {
        panic!()
    };
    assert_eq!(l.current, l.target);
    assert_eq!(
        l.target,
        Vec3::new(3200.0 * 512.0 + 256.0, 100.0, 3200.0 * 512.0 + 256.0)
    );
    let Some(Position::Entity(p)) = &c.position else {
        panic!()
    };
    assert_eq!(p.current, p.target);
    // The eye is the entity plus the offset rotated by the smoothed rotation,
    // which the eye update blends 5% per cycle (positionAngularInterpolation) from the
    // identity toward the scripted rotation: one cycle moves the eye by
    // 0.05 * 1200 * sin(22.5 deg) along the pitch.
    let eye = c.eye().unwrap();
    let look = c.lookat_point().unwrap();
    assert!((look.y - 300.0).abs() < 1e-3, "{look:?}");
    assert!(eye.z < look.z, "eye {eye:?} look {look:?}");
    let expected = 0.05 * 1200.0 * (22.5_f32).to_radians().sin();
    assert!(
        ((p.current.y - eye.y).abs() - expected).abs() < 1.0,
        "eye {eye:?} current {:?}",
        p.current
    );
    // The angle/distance getters see the raw current points.
    let d = run(&mut c, "cam2_getpositionentity_lookatdistance", &[]).unwrap();
    assert_eq!(d, Some(Value::Int(0)));
    let view = c.view().unwrap();
    assert_eq!(view.level, 0);
    // pitch = atan2(-(look.y - eye.y), horizontal): the eye is below
    // the look point after one cycle, so the camera looks up.
    assert!(
        view.pitch < 0.0 && view.coord == [eye.x as i32, eye.y as i32, eye.z as i32],
        "{view:?}"
    );
    // Moving the player: infinite acceleration snaps every cycle.
    c.scene.local_player = Some(player(
        [3200 * 512 + 256 + 512, 100, 3200 * 512 + 256],
        4096,
    ));
    c.update(0.02).unwrap();
    let Some(Focus::Entity(l)) = &c.lookat else {
        panic!()
    };
    assert_eq!(l.current.x, 3200.0 * 512.0 + 256.0 + 512.0);
    // Finite acceleration in the default mode moves current toward target
    // (1e5 units/s^2: one 20 ms cycle blends to 1600 units/s, 32 units moved;
    // a small acceleration would move less than one float ulp at 1.6e6).
    c.position_acceleration = [100000.0; 3];
    c.lookat_acceleration = [100000.0; 3];
    c.scene.local_player = Some(player(
        [3200 * 512 + 256 + 1024, 100, 3200 * 512 + 256],
        4096,
    ));
    c.update(0.02).unwrap();
    let Some(Focus::Entity(l)) = &c.lookat else {
        panic!()
    };
    assert!(
        l.current.x > 3200.0 * 512.0 + 256.0 + 512.0
            && l.current.x < 3200.0 * 512.0 + 256.0 + 1024.0,
        "{:?}",
        l.current
    );
}

#[test]
fn active_npc_entity_commands_bind_camera_trackable() {
    let mut c = Cam2::new(true);
    c.scene.npcs.insert(
        42,
        Trackable {
            kind: TRACKABLE_NPC,
            index: 42,
            level: 1,
            coord: [3200 * 512 + 256, -100, 3201 * 512 + 256],
            yaw: 2048,
        },
    );
    run(&mut c, "cam2_setlookatmode", &[MODE_ENTITY]).unwrap();
    run(&mut c, "cam2_setpositionmode", &[MODE_ENTITY]).unwrap();
    let mut look = vec![0, 200, 0, 0];
    c.dispatch_active("cam2_setlookatentity_npc", &mut look, Some(42))
        .unwrap()
        .unwrap();
    let mut position = vec![0, 0, -1200, 1024, 0, 0, 200];
    c.dispatch_active("cam2_setpositionentity_npc", &mut position, Some(42))
        .unwrap()
        .unwrap();
    assert_eq!(
        match &c.lookat {
            Some(Focus::Entity(owner)) => owner.trackable,
            _ => None,
        },
        Some(TrackableRef {
            kind: TRACKABLE_NPC,
            index: 42
        })
    );
    assert_eq!(
        match &c.position {
            Some(Position::Entity(owner)) => owner.trackable,
            _ => None,
        },
        Some(TrackableRef {
            kind: TRACKABLE_NPC,
            index: 42
        })
    );
}

#[test]
fn coordfine_object_lane_reaches_point_camera_owners() {
    let mut c = Cam2::new(true);
    run(&mut c, "cam2_setlookatmode", &[MODE_POINT]).unwrap();
    run(&mut c, "cam2_setpositionmode", &[MODE_POINT]).unwrap();

    let mut ints = Vec::new();
    let mut objects = vec![encode_coord_fine(2, [12_345, -678, 90_123])];
    c.dispatch_active_with_objects("cam2_setlookatpoint_point", &mut ints, &mut objects, None)
        .unwrap()
        .unwrap();
    let mut objects = vec![encode_coord_fine(2, [45_678, -321, 54_321])];
    c.dispatch_active_with_objects("cam2_setpositionpoint_point", &mut ints, &mut objects, None)
        .unwrap()
        .unwrap();

    assert_eq!(c.eye(), Some(Vec3::new(45_678.0, -321.0, 54_321.0)));
    let mut objects = Vec::new();
    c.dispatch_active_with_objects("cam2_getpositionpoint_point", &mut ints, &mut objects, None)
        .unwrap()
        .unwrap();
    assert_eq!(
        decode_coord_fine(&objects[0]),
        Some((2, [45_678, -321, 54_321]))
    );
}

#[test]
fn position_collision_pitches_the_offset_over_terrain() {
    // Flat terrain at height 0: a camera level with the player sits less
    // than the 1024-unit clearance above the floor.
    let t = Terrain::new(8, 8).unwrap();
    let mut c = Cam2::new(true);
    c.scene.base = [0, 0];
    c.scene.heightmap = Some(Heightmap::from_terrain(&t, 1));
    c.scene.local_player = Some(player([4 * 512 + 256, 0, 4 * 512 + 256], 0));
    run(&mut c, "cam2_setlookatmode", &[MODE_ENTITY]).unwrap();
    run(&mut c, "cam2_setpositionmode", &[MODE_ENTITY]).unwrap();
    run(&mut c, "cam2_setlookatentity_player", &[0, 0, 0, 0]).unwrap();
    // Camera 1000 units behind (toward z=0), level with the player.
    run(
        &mut c,
        "cam2_setpositionentity_player",
        &[0, 0, -1000, 0, 0, 0, 200],
    )
    .unwrap();
    let before = match &c.position {
        Some(Position::Entity(p)) => p.rotation,
        _ => panic!(),
    };
    // The eye update collides before moving, so the first cycle only initialises
    // `current` (collision returns while the eye is uninitialised).
    c.update(0.02).unwrap();
    let first = match &c.position {
        Some(Position::Entity(p)) => p.rotation,
        _ => panic!(),
    };
    assert_eq!(first, before);
    c.update(0.02).unwrap();
    // Clearance > 0, so the bisection pitches the offset upward.
    let after = match &c.position {
        Some(Position::Entity(p)) => p.rotation,
        _ => panic!(),
    };
    assert_ne!(before, after);
    let mut without = Cam2::new(true);
    without.scene = c.scene.clone();
    without.collision = [false, false];
    run(&mut without, "cam2_setlookatmode", &[MODE_ENTITY]).unwrap();
    run(&mut without, "cam2_setpositionmode", &[MODE_ENTITY]).unwrap();
    run(&mut without, "cam2_setlookatentity_player", &[0, 0, 0, 0]).unwrap();
    run(
        &mut without,
        "cam2_setpositionentity_player",
        &[0, 0, -1000, 0, 0, 0, 200],
    )
    .unwrap();
    without.update(0.02).unwrap();
    without.update(0.02).unwrap();
    let unpitched = match &without.position {
        Some(Position::Entity(p)) => p.rotation,
        _ => panic!(),
    };
    assert_eq!(unpitched, before);
}

#[test]
fn step_chunks_elapsed_time_like_update_game() {
    let mut c = Cam2::new(true);
    c.scene.local_player = Some(player([100, 0, 100], 0));
    run(&mut c, "cam2_setlookatmode", &[MODE_ENTITY]).unwrap();
    run(&mut c, "cam2_setpositionmode", &[MODE_ENTITY]).unwrap();
    run(&mut c, "cam2_setlookatentity_player", &[0, 0, 0, 0]).unwrap();
    run(
        &mut c,
        "cam2_setpositionentity_player",
        &[0, 0, -500, 0, 0, 0, 200],
    )
    .unwrap();
    let mut clock = 1000_i64;
    let mut now = || {
        clock += 30;
        clock
    };
    // First step seeds the last update time and runs no chunk.
    c.step(&mut now).unwrap();
    assert!(c.last_camera_update > 0);
    c.step(&mut now).unwrap();
    assert!(c.ready());
    c.camera_reset(2);
    assert_eq!(c.last_camera_update, 0);
    c.camera_smooth_reset();
    assert_eq!(c.camera_state, 1);
    assert!(c.changed);
}

#[test]
fn position_spline_decodes_control_points_and_advances_the_camera() {
    let mut bytes = vec![1, 1];
    let mut push_float = |value: f32| bytes.extend(value.to_bits().to_be_bytes());
    for value in [
        0.0, 0.0, 0.0, 33.3333, 0.0, 0.0, 0.0, 100.0, 0.0, 0.0, 66.6667, 0.0, 0.0, 100.0, 0.0,
    ] {
        push_float(value);
    }
    let mut reader = crate::ui_bytes::Cursor::new(&bytes);
    let mut spline = SplineTrack::new(CameraSplineKind::Accelerated);
    spline.decode(&mut reader).unwrap();
    assert_eq!(reader.remaining(), 0);
    assert_eq!(spline.point().unwrap(), Vec3::ZERO);

    spline.update(0.1, [100.0; 3], [f32::INFINITY; 3]).unwrap();
    let x = spline.point().unwrap().x;
    assert!(x > 0.0 && x < 100.0);
}
