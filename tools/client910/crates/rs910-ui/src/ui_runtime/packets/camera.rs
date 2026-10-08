//! Camera packets (`CAM_*`, `CAMERA_UPDATE`, `CAM2_ENABLE`).
//!
//! Applier of the packet router ([`super`]); each arm is the former
//! `packet_event` branch for its variants, moved verbatim in order.
use super::super::Runtime;
use crate::ui_vars::Variables;
use anyhow::Result;

impl Runtime {
    pub(super) fn apply_camera_packet(
        &mut self,
        vars: &mut Variables<'_>,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        match event {
            crate::server_prot::UiEvent::CameraReset => {
                // cameraReset(getDefaultCameraState()).
                let state = self.engine.camera.cam2.default_state;
                self.engine.camera.cam2.camera_reset(state);
                self.engine.scene.server_roof = [-1, -1];
                self.state.life.verify = self.state.life.verify.wrapping_add(1);
                self.state.life.verify_changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::CameraSmoothReset => {
                self.engine.camera.cam2.legacy.loop_cycle = vars.cycle;
                self.engine.camera.cam2.camera_smooth_reset();
                self.engine.scene.server_roof = [-1, -1];
                self.state.life.verify = self.state.life.verify.wrapping_add(1);
                self.state.life.verify_changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::CameraForceAngle { pitch, yaw } => {
                // read CAM_FORCEANGLE: cameraForceAngle(p, y, 0).
                self.engine
                    .camera_force_angle(i32::from(*pitch), i32::from(*yaw), 0);
                self.state.life.verify = self.state.life.verify.wrapping_add(1);
                self.state.life.verify_changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::CameraShake {
                channel,
                jitter,
                wobble_scale,
                cycle,
                wobble_speed,
            } => {
                self.engine.camera.cam2.legacy.shake(
                    usize::from(*channel),
                    crate::ui_cam2::LegacyModifier {
                        enabled: true,
                        jitter: i32::from(*jitter),
                        wobble_scale: i32::from(*wobble_scale),
                        cycle: i32::from(*cycle),
                        wobble_speed: i32::from(*wobble_speed),
                    },
                );
                self.state.life.verify = self.state.life.verify.wrapping_add(1);
                self.state.life.verify_changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::CameraMoveTo {
                x,
                z,
                source_height,
                acceleration,
                speed,
            } => {
                // read CAM_MOVETO: cameraMoveTo(.., true).
                self.engine.camera_move_to(
                    i32::from(*x),
                    i32::from(*z),
                    i32::from(*source_height) << 2,
                    i32::from(*acceleration),
                    i32::from(*speed),
                    true,
                );
                self.state.life.verify = self.state.life.verify.wrapping_add(1);
                self.state.life.verify_changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::CameraLookAt {
                x,
                z,
                height,
                acceleration,
                speed,
            } => {
                // read CAM_LOOKAT.
                self.engine.camera_look_at(
                    i32::from(*x),
                    i32::from(*z),
                    i32::from(*height) << 2,
                    i32::from(*acceleration),
                    i32::from(*speed),
                );
                self.state.life.verify = self.state.life.verify.wrapping_add(1);
                self.state.life.verify_changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::CameraRemoveRoof { packed } => {
                if *packed == -1 {
                    self.engine.scene.server_roof = [-1, -1];
                } else {
                    let base = [vars.scene.base[0] >> 9, vars.scene.base[1] >> 9];
                    let clamp_tile = |value: i32| value.clamp(0, vars.scene.map_width);
                    let x = clamp_tile(((*packed >> 14) & 0x3fff) - base[0]);
                    let z = clamp_tile((*packed & 0x3fff) - base[1]);
                    self.engine.scene.server_roof = [(x << 9) + 256, (z << 9) + 256];
                }
                self.state.life.verify = self.state.life.verify.wrapping_add(1);
                self.state.life.verify_changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::Camera { opcode, bytes } => {
                if *opcode == crate::proto::server::CAM2_ENABLE {
                    anyhow::ensure!(bytes.len() == 1, "CAM2_ENABLE length");
                    self.engine
                        .camera
                        .cam2
                        .camera_reset(if bytes[0] == 1 { 3 } else { 2 });
                } else {
                    self.engine.camera.cam2.sync_scene(&vars.scene);
                    self.engine.camera.cam2.decode(bytes)?;
                    // read CAMERA_UPDATE. Script writes to
                    // cam2.changed are outgoing telemetry, not this transmit stamp.
                    self.state.life.cycles.camera_update = self.state.life.cycles.redraw;
                }
                Ok(())
            }
            other => anyhow::bail!("packet router: {other:?} is not a camera packet"),
        }
    }
}
