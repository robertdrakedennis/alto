//! `ViewerApp::view` (code-quality programme Phase 4.4).
use super::*;

/// The viewer camera (`OrbitCamera` and the client camera it derives) and
/// whether it follows the server player.
pub(super) struct ViewCamera {
    pub(super) camera: OrbitCamera,
    pub(super) follow: bool,
    pub(super) last_redraw: Option<i64>,
    /// The scripted or free camera is not ready, so the scene draws black.
    pub(super) camera_unready: bool,
}

impl ViewerApp {
    /// Following updates once per redraw from absolute sampled
    /// milliseconds. The game owns its raw orbit state across map transactions.
    /// The pose is selected by `cameraState`: the
    /// orbit (2), follow-coordinate orbit (4), the smooth-reset transition
    /// (1), the explicit legacy pose of cutscene/script cameras (5, 6) or
    /// cam2 (3, `update_cam2_frame`).
    pub(super) fn update_follow_camera(&mut self, millis: i64) -> anyhow::Result<()> {
        if !self.view.follow {
            return Ok(());
        }
        let (legacy, camera_state, viewport) = self
            .core
            .session
            .ui()
            .map(|ui| {
                (
                    Some(ui.engine.camera.cam2.legacy.clone()),
                    ui.engine.camera.cam2.camera_state,
                    ui.state.viewport,
                )
            })
            .unwrap_or((None, 2, None));
        // The camera zoom and viewport width come from the viewport.
        self.view.camera.zoom = viewport.map(|(_, zoom)| zoom);
        self.view.camera.legacy = None;
        let viewport_width =
            viewport.map_or(self.view.camera.viewport.0 as i32, |(rect, _)| rect[2]);
        let Some(game) = self.core.session.game_mut() else {
            return Ok(());
        };
        if game.game.runtime.map_request.is_some() || !game.game.runtime.feed.state.initialized {
            return Ok(());
        }
        let Some(terrain) = game.game.runtime.terrain.as_ref() else {
            return Ok(());
        };
        let base = [
            game.game.runtime.map.base_x * 512,
            game.game.runtime.map.base_z * 512,
        ];
        let max_tile = [game.game.runtime.map.width, game.game.runtime.map.height];
        self.view.camera.map_size_x = game.game.runtime.map.width;
        // cameraState 5 (cutscene camera) and 6 (camera move-along)
        // draw the retained legacy camera pose, with the modifiers and the
        // scene clamp applied to the drawn copy.
        if matches!(camera_state, 5 | 6) {
            if let Some(legacy) = legacy.as_ref() {
                let drawn = legacy.drawn(max_tile, &mut crate::ui_cam2::random_unit);
                if crate::game_debug_flags::flags().cutscene_trace && self.core.cycle % 20 == 0 {
                    log::info!(
                        "[cutscene] camera state {camera_state} drawn {drawn:?} zoom {:?}",
                        self.view.camera.zoom
                    );
                }
                self.view.set_legacy_frame(base, drawn);
            }
            self.update_cam2_frame();
            return Ok(());
        }
        let Some(player) =
            game.game.runtime.feed.state.players.players[game.game.runtime.map.local].as_ref()
        else {
            self.update_cam2_frame();
            return Ok(());
        };
        let world = crate::camera_follow::World {
            terrain,
            offsets: self
                .core
                .environment
                .env
                .as_ref()
                .and_then(|e| e.camera_height.as_ref()),
            level: game.game.runtime.feed.state.players.current_level,
        };
        // `cameraState == 4` (`cam_followcoord`): the orbit camera circles
        // the follow coordinate.
        let follow_coord = legacy
            .as_ref()
            .and_then(|c| c.follow_coord)
            .filter(|_| camera_state == 4);
        let view = if let Some(coord) = follow_coord {
            game.game
                .camera
                .frame_coord(millis, coord, &world)
                .map_err(|e| anyhow::anyhow!("follow-coord redraw: {e:?}"))?;
            game.game
                .camera
                .view_coord(coord, &world, None)
                .map_err(|e| anyhow::anyhow!("follow-coord view: {e:?}"))?
        } else {
            let point = [player.fine_x, player.fine_z];
            game.game
                .camera
                .frame(millis, point, &world)
                .map_err(|e| anyhow::anyhow!("follow redraw: {e:?}"))?;
            game.game
                .camera
                .view(point, &world, None)
                .map_err(|e| anyhow::anyhow!("follow view: {e:?}"))?
        };
        self.view.camera.target = glam::Vec3::new(
            (base[0] + view.target[0]) as f32 / 512.,
            -(view.target[1] as f32) / 512.,
            (base[1] + view.target[2]) as f32 / 512.,
        );
        self.view.camera.pitch = view.pitch as f32;
        self.view.camera.yaw = view.yaw as f32;
        // An enabled pitch modifier raises
        // the orbit pitch to its wobble scale + 128.
        if let Some(m) = legacy.as_ref().map(|l| l.modifiers[4]) {
            if m.enabled && m.wobble_scale + 128 > self.view.camera.pitch as i32 {
                self.view.camera.pitch = (m.wobble_scale + 128) as f32;
            }
        }
        // The orbit camera writes the legacy camera position, pitch and yaw.
        let eye = self
            .view
            .camera
            .scene_camera(self.view.camera.viewport)
            .eye();
        let orbit = crate::ui_cam2::LegacyPose {
            x: eye[0] - base[0],
            y: eye[1],
            z: eye[2] - base[1],
            pitch: self
                .view
                .camera
                .scene_camera(self.view.camera.viewport)
                .pitch_int(),
            yaw: self
                .view
                .camera
                .scene_camera(self.view.camera.viewport)
                .yaw_int(),
            roll: 0,
        };
        let zoom = self.view.camera.zoom.unwrap_or(0);
        let mut drawn_pose = None;
        if let Some(ui) = self.core.session.ui_mut() {
            let cam2 = &mut ui.engine.camera.cam2;
            if camera_state == 1 {
                // The camera transition update; when it completes the
                // stale statics are drawn for this frame.
                if !cam2.update_transition(orbit, viewport_width, zoom) {
                    ui.engine.scene.server_roof = [-1, -1];
                }
                drawn_pose = Some(
                    cam2.legacy
                        .drawn(max_tile, &mut crate::ui_cam2::random_unit),
                );
                self.view.camera.zoom = Some(cam2.legacy.zoom);
            } else if cam2.camera_state != 3 {
                cam2.legacy.pose = orbit;
                cam2.legacy.zoom = zoom;
                // Modifier jitter/wobble and the scene clamp
                // only replace the orbit frame when they change the pose.
                let drawn = cam2
                    .legacy
                    .drawn(max_tile, &mut crate::ui_cam2::random_unit);
                if drawn != orbit {
                    drawn_pose = Some(drawn);
                }
            }
        }
        if let Some(drawn) = drawn_pose {
            self.view.set_legacy_frame(base, drawn);
        }
        self.update_cam2_frame();
        Ok(())
    }
    /// The scene is filled black (and nothing
    /// is pushed or drawn) while the local player is outside the built map
    /// with `sceneState == 3`, or while a cutscene has no active camera.
    pub(super) fn drawscene_black(&self) -> bool {
        let Some(game) = self.core.session.game() else {
            return false;
        };
        if game.cutscene.scene_state != 3 {
            return !game.cutscene.camera_active;
        }
        let map = &game.runtime.map;
        game.runtime.feed.state.players.players[map.local]
            .as_ref()
            .is_some_and(|p| {
                let (x, z) = (p.fine_x as i32, p.fine_z as i32);
                x < 0 || x >= map.width * 512 || z < 0 || z >= map.height * 512
            })
    }
    /// With `cameraState == 3` the world is viewed through the cam2 camera
    /// once it is ready, and the viewport stays black until then. Any other
    /// state keeps the
    /// orbit camera above.
    pub(super) fn update_cam2_frame(&mut self) {
        let cam2 = match self.core.session.ui() {
            // The free camera takes precedence.
            Some(ui) if ui.engine.camera.free_camera.is_some() => ui
                .engine
                .camera
                .free_camera
                .as_ref()
                .map(|f| f.camera.frame()),
            Some(ui) if ui.engine.camera.cam2.camera_state == 3 => {
                Some(ui.engine.camera.cam2.frame())
            }
            _ => None,
        };
        let cutscene_black = self.drawscene_black();
        self.view.camera_unready = matches!(cam2, Some(None));
        let blackout = self.view.camera_unready || cutscene_black;
        if matches!(cam2, Some(Some(_))) {
            // A cam2 or free-camera frame replaces the legacy pose.
            self.view.camera.legacy = None;
        }
        self.view.install_cam2_frame(cam2.flatten());
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_scene_blackout(blackout);
        }
    }
    pub(super) fn update_camera(&mut self, dt: f32) {
        if self.view.follow {
            return;
        }
        let speed = self.view.camera.distance_tiles() * 1.1 * dt;
        let mut forward = 0.0;
        let mut strafe = 0.0;
        if self.input.pressed.contains(&KeyCode::KeyW) {
            forward += 1.0;
        }
        if self.input.pressed.contains(&KeyCode::KeyS) {
            forward -= 1.0;
        }
        if self.input.pressed.contains(&KeyCode::KeyD) {
            strafe += 1.0;
        }
        if self.input.pressed.contains(&KeyCode::KeyA) {
            strafe -= 1.0;
        }
        if forward != 0.0 || strafe != 0.0 {
            self.view.camera.pan(forward * speed, strafe * speed);
        }
        if self.input.pressed.contains(&KeyCode::KeyE) {
            self.view.camera.nudge_vertical(speed * 0.5);
        }
        if self.input.pressed.contains(&KeyCode::KeyQ) {
            self.view.camera.nudge_vertical(-speed * 0.5);
        }
    }
}

impl ViewCamera {
    /// Install the scripted or free camera's frame as the frame camera. The
    /// renderer's frame origin is `OrbitCamera.target`: anchor it on the integer
    /// look-at point (exact in f32 after /512) the way the frame is anchored on
    /// the scene base (`- base`).
    pub(super) fn install_cam2_frame(&mut self, frame: Option<crate::camera::Cam2Frame>) {
        self.camera.cam2 = frame.map(|mut frame| {
            let anchor = [
                frame.lookat[0] as i32,
                frame.lookat[1] as i32,
                frame.lookat[2] as i32,
            ];
            frame.rebase(anchor);
            self.camera.target = glam::Vec3::new(
                anchor[0] as f32 / 512.,
                -(anchor[1] as f32) / 512.,
                anchor[2] as f32 / 512.,
            );
            frame
        });
    }
    /// Install the drawn legacy pose (scene-local) as the frame camera: the
    /// frame origin sits eight tiles along the view axis
    /// (`orbitCamera`'s eye offset for the same angles).
    pub(super) fn set_legacy_frame(&mut self, base: [i32; 2], p: crate::ui_cam2::LegacyPose) {
        let eye = [base[0] + p.x, p.y, base[1] + p.z];
        let orbit = crate::camera::Orbit {
            target: [0, 0, 0],
            pitch: p.pitch,
            yaw: p.yaw,
            distance: 4096,
        };
        let offset = crate::camera::orbit_camera(orbit, 334);
        let target = [eye[0] - offset[0], eye[1] - offset[1], eye[2] - offset[2]];
        self.camera.legacy = Some(crate::camera::LegacyFrame {
            eye: offset,
            pitch: p.pitch,
            yaw: p.yaw,
            roll: p.roll,
        });
        self.camera.target = glam::Vec3::new(
            target[0] as f32 / 512.0,
            -(target[1] as f32) / 512.0,
            target[2] as f32 / 512.0,
        );
        self.camera.pitch = p.pitch as f32;
        self.camera.yaw = p.yaw as f32;
    }
}
