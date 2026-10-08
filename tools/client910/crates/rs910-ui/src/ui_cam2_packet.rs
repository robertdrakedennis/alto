//! Decoder of the camera update packet: modes, settings, effects and the
//! point/entity/spline target readers.
use super::*;
use crate::ui_bytes::Cursor;
use anyhow::{bail, ensure, Result};

fn float(r: &mut Cursor<'_>) -> Result<f32> {
    Ok(f32::from_bits(r.g4s()? as u32))
}
fn vector(r: &mut Cursor<'_>) -> Result<Vec3> {
    Ok(Vec3::new(float(r)?, float(r)?, float(r)?))
}
fn array(r: &mut Cursor<'_>) -> Result<[f32; 3]> {
    let v = vector(r)?;
    Ok([v.x, v.y, v.z])
}
fn trackable(r: &mut Cursor<'_>, scene: &Scene) -> Result<Option<TrackableRef>> {
    let reference = TrackableRef {
        kind: r.g1()? as i32,
        index: r.g2()? as i32,
    };
    Ok(scene.trackable(reference).map(|_| reference))
}

impl Cam2 {
    pub fn decode(&mut self, bytes: &[u8]) -> Result<()> {
        let mut r = Cursor::new(bytes);
        let flags = r.g1()?;
        self.control_mode = i32::from(flags & 1);
        if flags & 8 != 0 {
            let mode = i32::from(r.g1()?);
            let mode = (0..=6).contains(&mode).then_some(mode);
            if self.lookat_mode != mode {
                self.lookat_mode = mode;
                if let Some(mode) = mode {
                    self.lookat = Some(Focus::for_mode(mode));
                }
            }
        }
        if flags & 16 != 0 {
            let mode = i32::from(r.g1()?);
            let mode = (0..=4).contains(&mode).then_some(mode);
            if self.position_mode != mode {
                self.position_mode = mode;
                if let Some(mode) = mode {
                    self.position = Some(Position::for_mode(mode));
                }
            }
        }
        if flags & 128 != 0 {
            let settings = r.g2()?;
            if settings & 1 != 0 {
                self.lookat_max_speed = array(&mut r)?;
            }
            if settings & 2 != 0 {
                self.position_max_speed = array(&mut r)?;
            }
            if settings & 4 != 0 {
                self.lookat_acceleration = array(&mut r)?;
            }
            if settings & 8 != 0 {
                self.position_acceleration = array(&mut r)?;
            }
            if settings & 16 != 0 {
                self.depth_planes = [float(&mut r)?, float(&mut r)?];
            }
            if settings & 32 != 0 {
                self.fov = [float(&mut r)?, float(&mut r)?];
            }
            if settings & 64 != 0 {
                self.projection_mode = i32::from(r.g1()?);
            }
            if settings & 128 != 0 {
                self.projection_scale = r.g3()? as i32;
                r.g1()?;
            }
            if settings & 256 != 0 {
                let v = r.g1()?;
                self.collision = [v & 1 != 0, v & 2 != 0];
            }
            if settings & 512 != 0 {
                for _ in 0..r.g1()? {
                    let action = r.g1()?;
                    let id = i32::from(r.g1()?);
                    if action == 0 {
                        self.effects.retain(|(key, _)| *key != id);
                        continue;
                    }
                    let kind = r.g1()?;
                    let old = self.effects.iter().position(|(key, _)| *key == id);
                    // Existing effects decode their own type, even when the
                    // incoming type differs; Shake.decode preserves its phase.
                    let kind = old.map_or(kind, |i| match self.effects[i].1 {
                        Effect::Shake { .. } => 0,
                        Effect::Tilt(_) => 1,
                    });
                    let effect = match kind {
                        0 => Effect::Shake {
                            mode: i32::from(r.g1()?),
                            magnitude: float(&mut r)?,
                            duration: float(&mut r)?,
                            phase: old.map_or(0., |i| match self.effects[i].1 {
                                Effect::Shake { phase, .. } => phase,
                                _ => 0.,
                            }),
                        },
                        1 => Effect::Tilt(float(&mut r)?),
                        _ => bail!("unknown camera effect {kind}"),
                    };
                    if let Some(i) = old {
                        self.effects[i].1 = effect;
                    } else {
                        self.effects.push((id, effect));
                    }
                }
            }
            if settings & 1024 != 0 {
                self.trail = (i32::from(r.g2()?), float(&mut r)?);
            }
            if settings & 2048 != 0 {
                self.linear_movement_mode = i32::from(r.g1()?);
            }
            if settings & 4096 != 0 {
                self.lookat_spring = array(&mut r)?;
                self.position_spring = array(&mut r)?;
                self.lookat_spring_damping = float(&mut r)?;
                self.position_spring_damping = float(&mut r)?;
            }
            if settings & 8192 != 0 {
                float(&mut r)?;
            }
            if settings & 16384 != 0 {
                self.position_angular_interpolation = float(&mut r)?;
            }
        }
        if flags & 32 != 0 {
            match self.lookat.as_mut() {
                Some(Focus::Point(l)) => l.target = vector(&mut r)?,
                Some(Focus::Entity(l)) => {
                    l.trackable = trackable(&mut r, &self.scene)?;
                    l.offset = vector(&mut r)?;
                    l.rotate_with_entity = r.g1()? == 1;
                }
                Some(Focus::Orientation(l)) => {
                    let q = Quat {
                        w: float(&mut r)?,
                        x: float(&mut r)?,
                        y: float(&mut r)?,
                        z: float(&mut r)?,
                    };
                    l.set_orientation(q);
                }
                Some(Focus::Spline(l)) => l.decode(&mut r)?,
                Some(Focus::CoupledSpline(l)) => l.decode(&mut r)?,
                None => {}
            }
        }
        if flags & 64 != 0 {
            match self.position.as_mut() {
                Some(Position::Point(p)) => p.target = vector(&mut r)?,
                Some(Position::Entity(p)) => {
                    let reference = trackable(&mut r, &self.scene)?;
                    p.offset = vector(&mut r)?;
                    p.rotation = Quat {
                        w: float(&mut r)?,
                        x: float(&mut r)?,
                        y: float(&mut r)?,
                        z: float(&mut r)?,
                    };
                    p.follow_yaw = r.g1()? == 1;
                    p.min_distance = i32::from(r.g2()?);
                    p.trackable = reference;
                }
                Some(Position::Spline(p)) => p.decode(&mut r)?,
                None => {}
            }
        }
        ensure!(
            r.remaining() == 0,
            "CAMERA_UPDATE consumed {}, expected {}",
            r.pos(),
            bytes.len()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet_control_changes_preserve_targets_and_do_not_mark_script_telemetry() {
        let mut cam = Cam2::new(false);
        cam.changed = false;
        cam.decode(&[25, 0, 0]).unwrap();
        let Some(Position::Point(p)) = &mut cam.position else {
            panic!()
        };
        p.set(2, [100, 200, 300]);
        cam.decode(&[25, 0, 0]).unwrap();
        assert_eq!(cam.eye(), Some(Vec3::new(100., 200., 300.)));
        cam.decode(&[0]).unwrap();
        assert_eq!(cam.control_mode, 0);
        assert!(!cam.changed);
        for bytes in [&[][..], &[129], &[129, 0], &[1, 0]] {
            assert!(cam.decode(bytes).is_err());
        }
    }
    #[test]
    fn camera_packets_match_recording() -> Result<()> {
        let input = rs910_core::test_support::frozen::bytes("camera-packets/input.bin");
        let mut r = Cursor::new(&input);
        let mut out = Vec::new();
        let mut cam = Cam2::new(false);
        fn int(out: &mut Vec<u8>, value: i32) {
            out.extend(value.to_be_bytes());
        }
        fn floats(out: &mut Vec<u8>, values: &[f32]) {
            for v in values {
                out.extend(v.to_bits().to_be_bytes());
            }
        }
        fn vec(out: &mut Vec<u8>, v: Vec3) {
            floats(out, &[v.x, v.y, v.z]);
        }
        while r.remaining() > 0 {
            let len = r.g4s()?;
            let mut bytes = Vec::new();
            for _ in 0..len {
                bytes.push(r.g1()?);
            }
            cam.decode(&bytes)?;
            for value in [
                cam.control_mode,
                cam.lookat_mode.unwrap(),
                cam.position_mode.unwrap(),
                cam.linear_movement_mode,
                cam.projection_mode,
                cam.projection_scale,
                cam.collision[0] as i32,
                cam.collision[1] as i32,
                cam.trail.0,
            ] {
                int(&mut out, value);
            }
            floats(&mut out, &[cam.position_angular_interpolation]);
            for values in [
                cam.lookat_max_speed,
                cam.position_max_speed,
                cam.lookat_acceleration,
                cam.position_acceleration,
            ] {
                floats(&mut out, &values);
            }
            floats(&mut out, &cam.depth_planes);
            floats(&mut out, &cam.fov);
            floats(&mut out, &cam.lookat_spring);
            floats(&mut out, &cam.position_spring);
            floats(
                &mut out,
                &[
                    cam.lookat_spring_damping,
                    cam.position_spring_damping,
                    cam.trail.1,
                ],
            );
            let Some(Focus::Point(l)) = &cam.lookat else {
                panic!()
            };
            vec(&mut out, l.target);
            let Some(Position::Point(p)) = &cam.position else {
                panic!()
            };
            vec(&mut out, p.target);
            let mut effects = cam.effects.clone();
            effects.sort_by_key(|e| e.0);
            int(&mut out, effects.len() as i32);
            for (id, effect) in effects {
                int(&mut out, id);
                match effect {
                    Effect::Shake {
                        mode,
                        magnitude,
                        duration,
                        phase,
                    } => {
                        int(&mut out, 0);
                        int(&mut out, mode);
                        floats(&mut out, &[magnitude, duration, phase]);
                    }
                    Effect::Tilt(v) => {
                        int(&mut out, 1);
                        floats(&mut out, &[v]);
                    }
                }
            }
        }
        rs910_core::test_support::frozen::assert_stream("camera-packets/decoded", &out);
        Ok(())
    }
}
