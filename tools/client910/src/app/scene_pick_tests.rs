//! The scene ground pick ("Walk here") under every camera mode.
//!
//! The pick unprojects the mouse through the matrices of the last scene draw,
//! whichever camera drew it. These tests publish the drawn view exactly the way
//! the redraw does (the frame camera of each mode, `Frame::new`,
//! `DrawnView::of_frame`) and read the scene options a real UI tick builds at
//! the mouse. Flat ground keeps the expected tiles independent of the terrain.
use super::*;
use crate::{
    client_game::ClientGame,
    player_picking::Frame,
    protocol910::{live::Feed, terrain::Terrain},
    ui_cam2::{
        Cam2, FreeCamera, LegacyPose, Position, Scene, SceneInput, Trackable, MODE_POINT,
        TRACKABLE_PLAYER,
    },
    ui_runtime::Runtime,
    ui_scene_options::{DrawnView, SceneOption},
    ui_vars::Variables,
};

/// The scene base in fine units.
const BASE: [i32; 2] = [3200 << 9, 3264 << 9];
/// The scene rectangle: origin and size.
const VIEWPORT: [i32; 4] = [4, 4, 512, 334];

/// A client with a flat 104 x 104 scene and the local player in its middle.
struct Rig {
    ui: Runtime,
    game: ClientGame,
    terrain: Terrain,
    millis: i64,
}

impl Rig {
    fn new() -> anyhow::Result<Self> {
        let pack = crate::test_support::require_pack("client.config.js5");
        let mut ui = Runtime::new(pack.clone())?;
        ui.resize([1280, 720])?;
        ui.target.quiet = true;
        let game = ClientGame::login(&pack, 1, Feed::default(), 910, true)?;
        Ok(Self {
            ui,
            game,
            terrain: Terrain::new(104, 104).map_err(|e| anyhow::anyhow!("{e:?}"))?,
            millis: 10_000,
        })
    }

    /// One UI tick with the mouse at `mouse`; the scene options it built.
    fn options_at(&mut self, mouse: [i32; 2]) -> anyhow::Result<Vec<SceneOption>> {
        self.millis += 20;
        self.game.game.cycle += 1;
        self.ui.engine.platform.mouse = mouse;
        let millis = self.millis;
        let mut now = || millis;
        let player = Trackable {
            kind: TRACKABLE_PLAYER,
            index: 1,
            level: 0,
            coord: [BASE[0] + (52 << 9) + 256, 0, BASE[1] + (50 << 9) + 256],
            yaw: 0,
        };
        self.ui.tick(&mut Variables {
            cycle: self.game.game.cycle,
            definitions: &self.game.game.inputs.bits,
            state: &mut self.game.ui_variables,
            player: self.game.game.runtime.feed.state.varps.as_mut(),
            active_player: None,
            active_npc: None,
            now: &mut now,
            probe: None,
            varp_transmit: crate::ui_loop::Counter {
                num: self.game.game.runtime.varp_transmit_num,
                ids: self.game.game.runtime.varp_transmitted,
            },
            scene: SceneInput {
                map_width: 104,
                base: BASE,
                local_player: Some(player),
                local_size: 1,
                terrain: Some(&self.terrain),
                terrain_generation: 1,
                ..Default::default()
            },
        })?;
        Ok(self.ui.input.scene_options.clone())
    }

    /// The scene draw of `camera`: the viewport the UI published, the drawn
    /// view the next ticks pick through, and the frame they were built from.
    fn draw(&mut self, mut camera: OrbitCamera, viewport: [i32; 4]) -> Frame {
        let zoom = crate::camera::camera_zoom(viewport[2], viewport[3]);
        camera.zoom = Some(zoom);
        camera.scene_viewport = Some(viewport);
        self.ui.state.viewport = Some((viewport, zoom));
        let scene = camera.scene_camera((viewport[2] as u32, viewport[3] as u32));
        let frame = Frame::new(&scene, BASE, 1, viewport);
        self.ui.engine.scene.drawn_view = Some(DrawnView::of_frame(&frame));
        frame
    }

    /// The tile of the "Walk here" option at `mouse`, if one is offered.
    fn walk_tile(&mut self, mouse: [i32; 2]) -> anyhow::Result<Option<[i32; 2]>> {
        Ok(self
            .options_at(mouse)?
            .iter()
            .find(|option| option.action == 23)
            .map(|option| option.tile))
    }
}

/// The orbit camera around a scene-local target (`y` up is negative), as the
/// follow owner places it for camera states 1, 2 and 4.
fn orbit(target: [i32; 3], pitch: i32, yaw: i32) -> OrbitCamera {
    let mut camera = OrbitCamera::new(glam::Vec3::new(
        (BASE[0] + target[0]) as f32 / 512.0,
        -(target[1] as f32) / 512.0,
        (BASE[1] + target[2]) as f32 / 512.0,
    ));
    camera.pitch = pitch as f32;
    camera.yaw = yaw as f32;
    camera
}

/// The explicit camera pose of camera states 1, 5 and 6, installed the way the
/// redraw installs the drawn pose.
fn pose(pose: LegacyPose) -> OrbitCamera {
    let mut view = ViewCamera {
        camera: OrbitCamera::new(glam::Vec3::ZERO),
        follow: true,
        last_redraw: None,
        camera_unready: false,
    };
    view.set_legacy_frame(BASE, pose);
    view.camera
}

/// The scripted camera (state 3) looking from `eye` at `target`, in absolute
/// coordinates (`y` up), as the redraw installs its frame.
fn scripted(eye: [i32; 3], target: [i32; 3]) -> OrbitCamera {
    let mut cam2 = Cam2::new(true);
    cam2.position_mode = Some(MODE_POINT);
    cam2.position = Some(Position::Point(Default::default()));
    cam2.lookat_mode = Some(MODE_POINT);
    cam2.lookat = Some(crate::ui_cam2::Focus::Point(Default::default()));
    if let Some(Position::Point(point)) = cam2.position.as_mut() {
        point.set(0, eye);
    }
    if let Some(crate::ui_cam2::Focus::Point(focus)) = cam2.lookat.as_mut() {
        focus.set(target);
    }
    assert!(cam2.ready(), "scripted camera is ready");
    install(cam2.frame())
}

/// The free camera (the developer orbit) at `eye`
/// (`y` up), pitched by a middle-button drag of `drag` pixels the way the real
/// orbit input does.
fn free(eye: [i32; 3], drag: [i32; 2]) -> anyhow::Result<OrbitCamera> {
    let mut free = FreeCamera::create(0, eye, [0, 0], &Scene::default());
    free.handle_orbit_input(
        Some([VIEWPORT[2], VIEWPORT[3]]),
        drag,
        true,
        &|_| false,
        &Scene::default(),
    )
    .map_err(|reason| anyhow::anyhow!("free camera: {reason}"))?;
    Ok(install(free.camera.frame()))
}

/// A scripted or free camera frame as the frame camera.
fn install(frame: Option<crate::camera::Cam2Frame>) -> OrbitCamera {
    let mut view = ViewCamera {
        camera: OrbitCamera::new(glam::Vec3::ZERO),
        follow: true,
        last_redraw: None,
        camera_unready: false,
    };
    view.install_cam2_frame(frame);
    view.camera
}

/// The pixel the centre of scene-local tile `(x, z)` (flat ground at height 0)
/// is drawn at, if it is in front of the camera and inside the viewport.
fn ground_pixel(frame: &Frame, tile: [i32; 2]) -> Option<[i32; 2]> {
    let clip = crate::ui_scene_options::transform(
        &frame.vp,
        (tile[0] * 512 + 256) as f32,
        0.0,
        (tile[1] * 512 + 256) as f32,
    );
    if !clip[3].is_finite() || clip[3] <= 0.0 {
        return None;
    }
    let x = frame.screen[0] + frame.screen[2] * clip[0] / clip[3];
    let y = frame.screen[1] + frame.screen[3] * clip[1] / clip[3];
    let [left, top, width, height] = VIEWPORT;
    let inside = x >= (left + 8) as f32
        && y >= (top + 8) as f32
        && x < (left + width - 8) as f32
        && y < (top + height - 8) as f32;
    inside.then_some([x as i32, y as i32])
}

/// Every camera mode builds the same "Walk here" menu entry under the mouse:
/// for ground tiles around the player the pixel they are drawn at picks that
/// tile, and sky picks nothing.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn every_camera_mode_picks_the_ground() -> anyhow::Result<()> {
    let eye_abs = |x: i32, y: i32, z: i32| [BASE[0] + x, y, BASE[1] + z];
    let ground_target = [52 * 512 + 256, 50 * 512 + 256];
    // (name, camera state, frame camera).
    let modes: Vec<(&str, i32, OrbitCamera)> = vec![
        // States 2 and 4, and the orbit of state 1's transition target.
        (
            "orbit",
            2,
            orbit([52 * 512 + 256, -150, 50 * 512 + 256], 1500, 3000),
        ),
        (
            "orbit around a coordinate",
            4,
            orbit([40 * 512, -150, 50 * 512], 1900, 9000),
        ),
        // States 5 and 6 and the transition: an explicit pose, with roll.
        (
            "pose",
            5,
            pose(LegacyPose {
                x: 48 * 512,
                y: -2200,
                z: 40 * 512,
                pitch: 1400,
                yaw: 2000,
                roll: 0,
            }),
        ),
        (
            "move-along pose with roll",
            6,
            pose(LegacyPose {
                x: 56 * 512 + 77,
                y: -1800,
                z: 60 * 512 + 13,
                pitch: 1250,
                yaw: 9000,
                roll: 600,
            }),
        ),
        // State 3.
        (
            "scripted",
            3,
            scripted(
                eye_abs(52 * 512, 2500, 44 * 512),
                eye_abs(ground_target[0], 0, ground_target[1]),
            ),
        ),
        // The free camera, pitched down by a middle-button drag.
        (
            "free",
            3,
            free(eye_abs(52 * 512, 2500, 44 * 512), [0, 150])?,
        ),
    ];
    let mut rig = Rig::new()?;
    for (name, state, camera) in modes {
        rig.ui.engine.camera.cam2.camera_state = state;
        let frame = rig.draw(camera, VIEWPORT);
        let mut checked = 0;
        for tx in 44..61 {
            for tz in 42..60 {
                let Some(pixel) = ground_pixel(&frame, [tx, tz]) else {
                    continue;
                };
                assert_eq!(
                    rig.walk_tile(pixel)?,
                    Some([tx, tz]),
                    "{name}: tile ({tx}, {tz}) drawn at {pixel:?}"
                );
                checked += 1;
            }
        }
        assert!(checked >= 6, "{name}: only {checked} ground tiles in view");
    }
    Ok(())
}

/// Sky above the horizon picks no ground, and nothing picks before the first
/// scene draw (the pick has no view yet) unless the scripted camera can
/// supply one.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn sky_and_an_undrawn_scene_offer_no_walk_here() -> anyhow::Result<()> {
    let mut rig = Rig::new()?;
    let camera = pose(LegacyPose {
        x: 50 * 512,
        y: -400,
        z: 50 * 512,
        pitch: 100,
        yaw: 4000,
        roll: 0,
    });
    let mouse = [260, 60];
    rig.ui.state.viewport = Some((VIEWPORT, 256));
    // Camera state 5 before any draw.
    rig.ui.engine.camera.cam2.camera_state = 5;
    assert_eq!(rig.walk_tile(mouse)?, None, "before the first draw");
    let frame = rig.draw(camera, VIEWPORT);
    assert_eq!(rig.walk_tile(mouse)?, None, "sky at {mouse:?}");
    let below = [260, 300];
    assert!(frame.vp.iter().all(|v| v.is_finite()));
    assert!(
        rig.walk_tile(below)?.is_some(),
        "ground at {below:?} under the same camera"
    );
    Ok(())
}

/// The ground pick of the original client for the matrices of the follow
/// orbit and of an explicit pose with roll, over flat ground: the tile under
/// each of 25 pixels per case.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn ground_pick_matches_the_original_client() -> anyhow::Result<()> {
    let tall = [0, 0, 1024, 700];
    let cases: Vec<(String, [i32; 4], OrbitCamera)> = vec![
        orbit_case(1088, 0),
        orbit_case(1500, 3000),
        orbit_case(2000, 12000),
        orbit_case(1300, 8192),
        (
            "orbit-tall 1500 3000".into(),
            tall,
            orbit([52 * 512 + 100, -150, 50 * 512 + 300], 1500, 3000),
        ),
        pose_case([48 * 512, -2200, 40 * 512, 1400, 2000, 0]),
        pose_case([56 * 512 + 77, -1800, 60 * 512 + 13, 1250, 9000, 600]),
        pose_case([50 * 512, -3000, 50 * 512, 1700, 0, 12000]),
        pose_case([50 * 512, -400, 50 * 512, 15500, 4000, 0]),
        pose_case([50 * 512, -400, 50 * 512, 100, 4000, 0]),
        pose_case([30 * 512 + 5, -900, 70 * 512 + 400, 1100, 14000, 16000]),
    ];
    let mut rig = Rig::new()?;
    let mut lines = String::new();
    for (name, viewport, camera) in cases {
        rig.draw(camera, viewport);
        let zoom = crate::camera::camera_zoom(viewport[2], viewport[3]);
        lines.push_str(&format!(
            "case {name} viewport {} {} {} {} zoom {zoom}\n",
            viewport[0], viewport[1], viewport[2], viewport[3]
        ));
        for y in [60, 120, 180, 240, 320] {
            for x in [40, 140, 260, 380, 500] {
                match rig.walk_tile([x, y])? {
                    Some(tile) => lines.push_str(&format!("{x} {y} -> {} {}\n", tile[0], tile[1])),
                    None => lines.push_str(&format!("{x} {y} -> none\n")),
                }
            }
        }
    }
    rs910_core::test_support::frozen::assert_stream("scene-pick/recording", lines.as_bytes());
    Ok(())
}

fn orbit_case(pitch: i32, yaw: i32) -> (String, [i32; 4], OrbitCamera) {
    (
        format!("orbit {pitch} {yaw}"),
        VIEWPORT,
        orbit([52 * 512 + 100, -150, 50 * 512 + 300], pitch, yaw),
    )
}

fn pose_case(p: [i32; 6]) -> (String, [i32; 4], OrbitCamera) {
    (
        format!("pose {} {} {} {} {} {}", p[0], p[1], p[2], p[3], p[4], p[5]),
        VIEWPORT,
        pose(LegacyPose {
            x: p[0],
            y: p[1],
            z: p[2],
            pitch: p[3],
            yaw: p[4],
            roll: p[5],
        }),
    )
}
