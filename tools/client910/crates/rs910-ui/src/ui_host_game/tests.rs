use super::*;

/// The scripted camera path on a real cache spline (cutscene 2,
/// splines 1/2, keyframe 1, speed 936): the progress clock advances by the
/// constant speed each logic update, clamps at 65535 on update 71 with a
/// one-update `splineFinished`, and the eye lands exactly on the next
/// keyframe's control point (t = 1 of applyCameraMoveAlong).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn cutscene_move_along_clock_on_cache_spline() -> anyhow::Result<()> {
    let pack = crate::test_support::require_pack("client.cutscenes.js5");
    let def = crate::cutscene::Definition::load(&pack, 2)?;
    let pos = def.splines[1].rows();
    let target = def.splines[2].rows();
    let mut engine = Engine::default();
    engine.game_host.cutscene_spline = [Some(pos.clone()), Some(target)];
    engine.camera.cam2.legacy.move_along = Some(crate::ui_cam2::LegacyMoveAlong {
        pos_spline: 0,
        pos_keyframe: 1,
        target_spline: 1,
        target_keyframe: 1,
        progress: 0,
        speed_min: 936,
        speed_max: 936,
    });
    let mut legacy = std::mem::take(&mut engine.camera.cam2.legacy);
    for tick in 1..=71 {
        engine.game_host.move_along(&mut legacy)?;
        let progress = legacy.move_along.unwrap().progress;
        if tick < 71 {
            assert_eq!(progress, 936 * tick);
            assert!(!engine.game_host.spline_finished);
        } else {
            assert_eq!(progress, 65535);
            assert!(engine.game_host.spline_finished);
        }
    }
    // t = 1: the Hermite form reduces to the third row (next keyframe).
    assert_eq!(
        [legacy.pose.x, legacy.pose.y, legacy.pose.z],
        [pos[4][0], -pos[4][1], pos[4][2]]
    );
    // cameraRoll interpolates the keyframe rolls by progress (>> 16).
    assert_eq!(
        legacy.pose.roll,
        ((65535 * (pos[4][3] - pos[2][3])) >> 16) + pos[2][3]
    );
    engine.game_host.move_along(&mut legacy)?;
    assert!(!engine.game_host.spline_finished);
    Ok(())
}

/// cameraMoveTo / cameraForceAngle / applyCameraCutscene
///  through the engine owner.
#[test]
fn legacy_move_to_and_force_angle_drive_state_five() {
    let mut engine = Engine::default();
    engine.camera_move_to(2, 3, 100, 5, 50, false);
    assert_eq!(engine.camera.cam2.camera_state, 5);
    assert_eq!(engine.scene.server_roof, [-1, -1]);
    engine.camera_force_angle(100, 200, 0);
    assert_eq!(
        (
            engine.camera.cam2.legacy.pose.pitch,
            engine.camera.cam2.legacy.pose.yaw
        ),
        (800, 1600)
    );
    engine.camera.cam2.legacy.pose.pitch = 0;
    engine.camera.cam2.legacy.pose.yaw = 0;
    // One update of the smoothed move with a flat -200 heightmap.
    engine.camera.cam2.legacy.apply_cutscene(&|_, _| -200);
    let p = engine.camera.cam2.legacy.pose;
    assert_eq!([p.x, p.y, p.z], [69, -20, 94]);
    // No look-at speed: the pitch/yaw statics hold (rotate speed 0).
    assert_eq!([p.pitch, p.yaw, p.roll], [0, 0, 0]);
    // Speed >= 100 snaps to the tile centre above the height offset.
    engine.camera_move_to(2, 3, 100, 100, 100, false);
    engine.camera.cam2.legacy.apply_cutscene(&|_, _| -200);
    let p = engine.camera.cam2.legacy.pose;
    assert_eq!([p.x, p.y, p.z], [1280, -300, 1792]);
}

#[test]
fn cutscene_splines_drive_the_move_along_camera() {
    let mut engine = Engine::default();
    engine.game_host.base = [3200, 3200];
    let mut objs = vec![];
    let run = |engine: &mut Engine, command, ints: &mut Vec<i32>, objs: &mut Vec<String>| {
        super::run(engine, command, ints, objs)
    };
    run(&mut engine, "spline_new", &mut vec![0, 2], &mut objs).unwrap();
    run(&mut engine, "spline_new", &mut vec![1, 2], &mut objs).unwrap();
    assert_eq!(
        run(&mut engine, "spline_length", &mut vec![0], &mut objs).unwrap(),
        Some(Value::Int(2))
    );
    let coord = |x: i32, z: i32| (x << 14) | z;
    for (spline, key, a, b) in [
        (0, 0, coord(3202, 3202), coord(3203, 3202)),
        (0, 1, coord(3204, 3202), coord(3205, 3202)),
        (1, 0, coord(3202, 3210), coord(3203, 3210)),
        (1, 1, coord(3204, 3210), coord(3205, 3210)),
    ] {
        run(
            &mut engine,
            "spline_addpoint",
            &mut vec![spline, key, a, 100, b, 100, 0],
            &mut objs,
        )
        .unwrap();
    }
    assert_eq!(
        engine.game_host.cutscene_spline[0].as_ref().unwrap()[0],
        [3202 << 9, 400, 3202 << 9, 0]
    );
    run(
        &mut engine,
        "cam_movealong",
        &mut vec![0, 0, 65535, 65535, 1, 0],
        &mut objs,
    )
    .unwrap();
    assert_eq!(engine.camera.cam2.camera_state, 6);
    let mut legacy = std::mem::take(&mut engine.camera.cam2.legacy);
    engine.game_host.move_along(&mut legacy).unwrap();
    let view = legacy.move_along_view.unwrap();
    assert_eq!(legacy.move_along.unwrap().progress, 65535);
    assert!(engine.game_host.spline_finished);
    // The eye ends on the third control point, looking north (+z).
    assert_eq!(view.position, [4 << 9, -400, 2 << 9]);
    assert_eq!(view.target[2], 10 << 9);
    assert_eq!(view.yaw, 0);
    engine.game_host.move_along(&mut legacy).unwrap();
    assert!(
        !engine.game_host.spline_finished,
        "splineFinished is one cycle wide"
    );
    // cameraReset clears posSpline/targetSpline.
    engine.camera.cam2.legacy = legacy;
    run(&mut engine, "cam_reset", &mut vec![], &mut objs).unwrap();
    assert_eq!(engine.camera.cam2.legacy.move_along, None);
    assert_eq!(engine.camera.cam2.camera_state, 2);
    assert!(run(
        &mut engine,
        "cam_movealong",
        &mut vec![0, 1, 0, 0, 1, 0],
        &mut objs
    )
    .is_err());
}
