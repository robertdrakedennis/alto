//! E3 replay fixtures. The generated rows are explicit inputs to the roof
//! routines; masks/bounds are independently produced on each side.
use crate::{
    draw_trace::Trace,
    roof::{RoofInput, RoofState, RoofWorld},
};
use std::path::Path;

/// id,action,mode,cycle,level,cameraState,cameraXYZ,pitch,playerXZ,serverXZ,
/// baseXZ,cam2LookXZ bits,cam2EyeXZ bits, four action arguments.
type Row = [i32; 24];

fn rows(result: &crate::rebuild::Rebuild) -> Vec<Row> {
    let n = result.map_size;
    let mut roof = None;
    let mut open = None;
    for x in 2..n - 2 {
        for z in 2..n - 2 {
            if result.flags.get(0, x, z) & 4 != 0 {
                roof.get_or_insert((x as i32, z as i32));
            } else {
                open.get_or_insert((x as i32, z as i32));
            }
        }
    }
    let (rx, rz) = roof.expect("roof oracle needs an under-roof tile");
    let (ox, oz) = open.expect("roof oracle needs an open tile");
    let mut a = [0; 24];
    a[2] = 0;
    a[3] = 120;
    a[5] = 2;
    a[6] = ((rx + 5).min(n as i32 - 1) << 9) + 256;
    a[7] = -3000;
    a[8] = (rz << 9) + 256;
    a[9] = 1600;
    a[10] = (rx << 9) + 256;
    a[11] = (rz << 9) + 256;
    a[12] = -1;
    a[13] = -1;
    a[14] = result.base_x;
    a[15] = result.base_z;
    a[16] = ((result.base_x * 512 + a[10]) as f32 + 0.5).to_bits() as i32;
    a[17] = ((result.base_z * 512 + a[11]) as f32 + 0.5).to_bits() as i32;
    a[18] = ((result.base_x * 512 + a[6]) as f32 + 0.25).to_bits() as i32;
    a[19] = ((result.base_z * 512 + a[8]) as f32 + 0.25).to_bits() as i32;
    let mut out = Vec::new();
    let mut push = |action: i32, a: Row| {
        let mut a = a;
        a[0] = out.len() as i32;
        a[1] = action;
        out.push(a);
    };
    for mode in [0, 1, 3, 2] {
        a[2] = mode;
        a[4] = 0;
        a[5] = 2;
        a[9] = 1600;
        a[10] = (rx << 9) + 256;
        a[11] = (rz << 9) + 256;
        push(0, a);
        for state in [2, 3, 1] {
            a[5] = state;
            for cycle in [121, 127, 128, 129, 254, 255, 256, 257, 260] {
                a[3] = cycle;
                push(1, a);
            }
        }
        // cam2 invalid lookat, out-of-window lookat/eye, zero-distance ray.
        a[5] = 3;
        for word in [
            f32::NAN.to_bits() as i32,
            ((result.base_x * 512 - 513) as f32).to_bits() as i32,
        ] {
            let mut b = a;
            b[16] = word;
            push(1, b);
        }
        let mut b = a;
        b[16] = ((result.base_x * 512 + (ox << 9) + 256) as f32).to_bits() as i32;
        b[17] = ((result.base_z * 512 + (oz << 9) + 256) as f32).to_bits() as i32;
        b[18] = ((result.base_x * 512 - 1024) as f32).to_bits() as i32;
        push(1, b);
        b[18] = b[16];
        b[19] = b[17];
        push(1, b);
        // Camera-only height branch below/at the strict 3200 boundary.
        a[5] = 1;
        a[6] = (rx << 9) + 256;
        a[8] = (rz << 9) + 256;
        let plane = usize::from(result.flags.is_link_below(a[6] >> 9, a[8] >> 9));
        let h = result.scene.normal[plane]
            .as_ref()
            .unwrap()
            .heights
            .get_fine_height(a[6], a[8]);
        for dy in [3199, 3200] {
            a[7] = h - dy;
            push(1, a);
        }
        a[5] = 4;
        a[12] = (rx << 9) + 256;
        a[13] = (rz << 9) + 256;
        push(1, a);
        a[12] = -1;
        a[13] = -1;
        a[4] = 3;
        push(1, a); // Mode 2: retain old boxes after clearing column.
        for level in [1, 2, 3] {
            a[4] = level;
            push(0, a);
            push(1, a);
        }
    }
    // Controlled ray geometry, no dependency on a map's roof layout.
    a[4] = 0;
    a[3] = 256;
    a[12] = -1;
    a[13] = -1;
    a[20] = 0;
    a[21] = 0;
    push(4, a);
    a[20] = 0;
    a[21] = 50;
    a[22] = 50;
    a[23] = 4;
    push(3, a);
    for mode in [2, 3] {
        a[2] = mode;
        a[3] = 256;
        push(0, a);
        a[5] = 2;
        for (dx, dz) in [
            (10, 10),
            (10, -10),
            (-10, 10),
            (-10, -10),
            (10, 5),
            (-10, 5),
            (5, 10),
            (5, -10),
        ] {
            a[6] = ((50 - dx) << 9) + 256;
            a[8] = ((50 - dz) << 9) + 256;
            a[10] = ((50 + dx) << 9) + 256;
            a[11] = ((50 + dz) << 9) + 256;
            for pitch in [2559, 2560] {
                a[9] = pitch;
                a[3] += 1;
                push(1, a);
            }
        }
        // Roof-of-world camera flag, with cameraY on either side of height.
        a[20] = 3;
        a[21] = 40;
        a[22] = 40;
        a[23] = 2;
        push(3, a);
        a[6] = (40 << 9) + 256;
        a[8] = (40 << 9) + 256;
        a[10] = (60 << 9) + 256;
        a[11] = (60 << 9) + 256;
        let h = result.scene.normal[3]
            .as_ref()
            .unwrap()
            .heights
            .get_fine_height(a[6], a[8]);
        for delta in [-1, 0] {
            a[7] = h + delta;
            a[3] += 1;
            push(1, a);
        }
        a[23] = 0;
        push(3, a);
    }
    // Queue cursor rollover on a complete 104x104 roof; 512-region scan cap.
    a[4] = 0;
    a[20] = 4;
    a[21] = 0;
    push(4, a);
    a[2] = 2;
    a[3] = 512;
    push(0, a);
    a[20] = 0;
    a[21] = 52;
    a[22] = 52;
    a[23] = 0;
    push(2, a);
    push(2, a);
    a[20] = 0;
    a[21] = 1;
    push(4, a);
    a[2] = 1;
    push(0, a);
    // Each upper plane excluded; zero-filled boxes are expanded nonetheless.
    a[20] = 8;
    a[21] = 0;
    push(4, a);
    a[20] = 0;
    a[21] = 50;
    a[22] = 50;
    a[23] = 4;
    push(3, a);
    for plane in 1..4 {
        a[20] = plane;
        a[23] = 8;
        push(3, a);
    }
    a[2] = 2;
    a[3] = 4;
    push(0, a);
    a[20] = 0;
    a[21] = 50;
    a[22] = 50;
    a[23] = 1;
    push(2, a);
    push(2, a);
    // Explicit wall/direction/shape-21 and multi-tile-footprint boundaries.
    for kind in 1..=5 {
        a[20] = kind;
        push(5, a);
        a[20] = 0;
        a[21] = 0;
        push(4, a);
        a[20] = 0;
        a[21] = 50;
        a[22] = 50;
        a[23] = 4;
        push(3, a);
        a[2] = 2;
        a[3] = 4;
        a[4] = 0;
        push(0, a);
        a[20] = 0;
        a[21] = 50;
        a[22] = 50;
        a[23] = 0;
        push(2, a);
        push(2, a);
        a[23] = 1;
        push(2, a);
    }
    out
}

pub fn input(a: &Row) -> RoofInput {
    RoofInput {
        cycle: a[3],
        level: a[4] as usize,
        camera_state: a[5],
        camera: [a[6], a[7], a[8]],
        pitch: a[9],
        player: [a[10] as f32, a[11] as f32],
        server: [a[12], a[13]],
        base: [a[14], a[15]],
        cam2_look: [f32::from_bits(a[16] as u32), f32::from_bits(a[17] as u32)],
        cam2_eye: [f32::from_bits(a[18] as u32), f32::from_bits(a[19] as u32)],
    }
}

pub fn write(result: &crate::rebuild::Rebuild, dir: &Path) -> anyhow::Result<()> {
    let rows = rows(result);
    let text = rows
        .iter()
        .map(|r| r.iter().map(i32::to_string).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("roof-inputs.txt"), text + "\n")?;
    let heights: Vec<_> = result
        .scene
        .normal
        .iter()
        .map(|g| g.as_ref().unwrap().heights.clone())
        .collect();
    let mut flags = result.flags.clone();
    let mut state = RoofState::default();
    let mut fixture_scene = None;
    let mut trace = Trace::default();
    let mut initial_flags = Vec::new();
    for l in 0..4 {
        for x in 0..result.map_size {
            for z in 0..result.map_size {
                initial_flags.push(flags.get(l, x, z) as i32);
            }
        }
    }
    trace.push("build/roofFlags", initial_flags);
    for a in rows {
        let i = input(&a);
        let mut result_value = -1;
        match a[1] {
            5 => fixture_scene = Some(synthetic_scene(result.map_size, a[20])),
            3 => flags.set(a[20] as usize, a[21] as usize, a[22] as usize, a[23] as i8),
            4 => {
                for l in 0..4 {
                    for x in 0..result.map_size {
                        for z in 0..result.map_size {
                            flags.set(
                                l,
                                x,
                                z,
                                if a[21] == 1 {
                                    if l == 0 && x % 3 == 1 && z % 3 == 1 {
                                        4
                                    } else {
                                        0
                                    }
                                } else {
                                    a[20] as i8
                                },
                            );
                        }
                    }
                }
            }
            _ => {
                let world = RoofWorld {
                    scene: fixture_scene.as_ref().or(result.scene_graph.as_ref()),
                    flags: &flags,
                    heights: &heights,
                };
                match a[1] {
                    0 => state.setup(&world, a[2], i.level, i.cycle),
                    1 => state.update(&world, &i),
                    2 => {
                        result_value = state.flood(
                            &world,
                            crate::roof::FloodSeed {
                                level: i.level,
                                tile: [a[21] as usize, a[22] as usize],
                                box_index: a[20] as usize,
                            },
                            i.cycle,
                            a[23] != 0,
                        ) as i32
                    }
                    _ => anyhow::bail!("unknown roof action"),
                }
            }
        }
        let prefix = format!("roof/{}/", a[0]);
        trace.push(format!("{prefix}input"), a.to_vec());
        trace.push(
            format!("{prefix}state"),
            vec![
                state.mode,
                state.setup_level,
                state.hide_roof as i32,
                state.draw_stamp(i.cycle) as i32,
                result_value,
            ],
        );
        trace.push(
            format!("{prefix}boxes"),
            state.boxes.iter().flatten().copied().collect(),
        );
        trace.push(
            format!("{prefix}stamps"),
            state.stamps.as_ref().map_or_else(Vec::new, |m| {
                m.iter().flatten().flatten().map(|&v| v as i32).collect()
            }),
        );
    }
    std::fs::write(dir.join("roofs.bin"), trace.encode())?;
    Ok(())
}

/// Packed-direction boundary blockers and a footprint extending beyond the
/// one-tile flood boundary. Scene-building behavior is already verified in C;
/// these fixtures isolate the roof consumer's wall/Location decisions.
fn synthetic_scene(n: usize, kind: i32) -> crate::scene::Scene {
    use crate::scene::{Scene, SceneryEntity, WallEntity};
    let mut scene = Scene::new(9, 4, n, n);
    let neighbours = [
        (-1, 0, [18, 211, 19]),
        (-1, 1, [18, 82, 19]),
        (0, 1, [82, 19, 83]),
        (1, 1, [82, 146, 83]),
        (1, 0, [146, 83, 147]),
        (-1, -1, [210, 18, 211]),
        (0, -1, [210, 147, 211]),
        (1, -1, [146, 210, 147]),
    ];
    let loc = |x: i32, z: i32, shape: i32, angle: i32, max_x: i32, max_z: i32| SceneryEntity {
        level: 1,
        occlude_level: 1,
        x: x * 512,
        y: 0,
        z: z * 512,
        min_tx: x,
        max_tx: max_x,
        min_tz: z,
        max_tz: max_z,
        raised: false,
        diag: 0,
        model_y: 0,
        loc_id: 0,
        shape,
        angle,
        active: false,
        use_merged_normals: false,
        has_hard_shadow: false,
        primary_layer: false,
        srt: None,
        dynamic: false,
        model: None,
    };
    if kind == 5 {
        scene.add_entity(loc(50, 50, 10, 0, 53, 53));
    } else {
        for (dx, dz, directions) in neighbours {
            let (x, z) = (50 + dx, 50 + dz);
            for level in 1..4 {
                let d = directions[level - 1];
                if kind == 4 {
                    let shape = if d & 63 == 19 { 21 } else { d & 63 };
                    let mut e = loc(x, z, shape, (d >> 6) & 3, x, z);
                    e.level = level as i32;
                    e.occlude_level = level as i32;
                    scene.add_entity(e);
                } else {
                    let wall = |wall_type| WallEntity {
                        level: level as i32,
                        occlude_level: level as i32,
                        x: x * 512,
                        y: 0,
                        z: z * 512,
                        wall_type,
                        loc_id: 0,
                        shape: 0,
                        angle: 0,
                        active: false,
                        use_merged_normals: false,
                        has_hard_shadow: false,
                        srt: None,
                        dynamic: false,
                        model: None,
                    };
                    let selected = crate::roof::wall_type(directions[(kind - 1) as usize]);
                    if kind == 2 {
                        scene.add_wall(
                            level,
                            x as usize,
                            z as usize,
                            wall(256),
                            Some(wall(selected)),
                        );
                    } else {
                        scene.add_wall(level, x as usize, z as usize, wall(selected), None);
                    }
                }
            }
        }
    }
    scene
}
