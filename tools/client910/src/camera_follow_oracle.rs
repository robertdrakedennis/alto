//! Recorded follow-camera clamp, logic, view and rebase replay.
use crate::{
    camera_follow::{Follow, World},
    protocol910::terrain::Terrain,
};
use std::io::Write;
fn words(s: &Follow) -> Vec<i32> {
    vec![
        s.x,
        s.z,
        s.pitch.to_bits() as i32,
        s.yaw.to_bits() as i32,
        s.pitch_velocity.to_bits() as i32,
        s.yaw_velocity.to_bits() as i32,
        s.pitch_touched as i32,
        s.yaw_touched as i32,
        s.changed as i32,
        s.pitch_clamp,
        s.height,
        s.offset_x,
        s.offset_z,
        s.offset_yaw,
    ]
}
fn put(w: &mut impl Write, values: &[i32]) -> anyhow::Result<()> {
    for v in values {
        w.write_all(&v.to_be_bytes())?;
    }
    Ok(())
}
#[test]
fn player_follow() -> anyhow::Result<()> {
    let scratch = rs910_core::test_support::frozen::Scratch::new("camera-follow");
    let out = scratch.dir().to_path_buf();
    std::fs::create_dir_all(&out)?;
    let mut input = std::fs::File::create(out.join("input.bin"))?;
    let mut result = std::fs::File::create(out.join("rust.bin"))?;
    put(&mut input, &[24])?;
    for scenario in 0..24i32 {
        let width = [72, 104, 120, 136, 168, 256][scenario as usize % 6];
        let height = [104, 72, 136, 120, 256, 168][scenario as usize % 6];
        let style = scenario % 4;
        let mut terrain = Terrain::new(width, height).unwrap();
        let offsets: Option<[Option<Vec<i8>>; 4]> = if style == 0 {
            None
        } else {
            Some(std::array::from_fn(|l| {
                if style == 1 && l % 2 == 0 {
                    None
                } else {
                    Some(vec![0; (width + 1) * (height + 1)])
                }
            }))
        };
        let mut offsets = offsets;
        for l in 0..4 {
            for x in 0..=width {
                for z in 0..=height {
                    let h = if style == 0 || style == 2 {
                        -960 * l as i32
                    } else {
                        -960 * l as i32 + x as i32 * 17 - z as i32 * 13 + ((x * z) % 23) as i32 * 43
                    };
                    let p = terrain.point(l, x, z);
                    terrain.heights[p] = h;
                    if x < width && z < height {
                        let t = terrain.tile(l, x, z);
                        terrain.tiles[t].flags = if l == 1 && (x + z) % 3 == 0 { 2 } else { 0 };
                    }
                    if let Some(grid) = offsets.as_mut().and_then(|o| o[l].as_mut()) {
                        grid[x * (height + 1) + z] = if style == 2 {
                            255u8 as i8
                        } else {
                            ((x * 3 + z * 7 + l * 13) % 256) as i8
                        };
                    }
                }
            }
        }
        let mut s = Follow {
            pitch: [-130., 1088.75, 2787.5, 4000.][scenario as usize % 4],
            yaw: [-33000.5, 0.25, 16384.75, 33000.][scenario as usize % 4],
            x: 10000,
            z: 12000,
            pitch_clamp: if scenario % 2 == 0 { 0 } else { 999999 },
            height: 235 + scenario * 19,
            offset_x: scenario * 3 - 35,
            offset_z: 31 - scenario * 2,
            offset_yaw: scenario * 17 - 101,
            ..Default::default()
        };
        put(
            &mut input,
            &[scenario, width as i32, height as i32, style, 256],
        )?;
        put(&mut input, &words(&s))?;
        for frame in 0..256i32 {
            let dt = [0, 1, 6, 8, 16, 20, 33, 120, 321, 1000][frame as usize % 10];
            let pos = match frame % 32 {
                0 => [
                    s.x as f32 + 2000.75 - s.offset_x as f32,
                    s.z as f32 + 1.5 - s.offset_z as f32,
                ],
                1 => [
                    s.x as f32 - 2000.25 - s.offset_x as f32,
                    s.z as f32 - 1.25 - s.offset_z as f32,
                ],
                2 => [
                    s.x as f32 + 2001. - s.offset_x as f32,
                    s.z as f32 + 2001. - s.offset_z as f32,
                ],
                3 => [512.25, 511.75],
                4 => [-1.25, 512.5],
                5 => [width as f32 * 512. - 1., height as f32 * 512. - 1.],
                _ => [
                    width as f32 * 256. + ((frame % 9) * 31) as f32 + 0.75,
                    height as f32 * 256. - ((frame % 7) * 27) as f32 + 0.25,
                ],
            };
            let level = (scenario + frame / 17) % 4;
            let logic = frame % 4;
            let actions = if frame % 7 < 4 {
                1 << ((frame / 7) % 4)
            } else {
                15
            };
            let (dx, dz) = if frame % 97 == 0 { (16, -24) } else { (0, 0) };
            let modifier = if frame % 11 == 0 {
                Some(1024 + frame * 2)
            } else {
                None
            };
            put(
                &mut input,
                &[
                    dt,
                    pos[0].to_bits() as i32,
                    pos[1].to_bits() as i32,
                    level,
                    logic,
                    actions,
                    dx,
                    dz,
                    modifier.unwrap_or(i32::MIN),
                ],
            )?;
            s.rebase(dx, dz);
            for _ in 0..logic {
                s.logic();
                for action in 0..4 {
                    if actions & (1 << action) != 0 {
                        s.input(action < 2, action % 2 == 0);
                        s.input(action < 2, action % 2 == 0);
                    }
                }
            }
            let w = World {
                terrain: &terrain,
                offsets: offsets.as_ref(),
                level,
            };
            s.frame(dt as i64, pos, &w)
                .map_err(|e| anyhow::anyhow!("follow: {e:?}"))?;
            let view = s.view(pos, &w, modifier).unwrap();
            let mut record = words(&s);
            record.extend(view.target);
            record.extend([view.pitch, view.yaw]);
            record.extend(crate::camera::orbit_camera(
                crate::camera::Orbit {
                    target: view.target,
                    pitch: view.pitch,
                    yaw: view.yaw,
                    distance: crate::camera::orbit_distance(view.pitch),
                },
                334 + frame % 201,
            ));
            put(&mut result, &record)?;
        }
    }
    println!("Rust follow: 24 scenarios, 6144 redraws");
    scratch.finish("camera-follow", &[("rust.bin", "recording")]);
    Ok(())
}
