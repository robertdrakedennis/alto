//! Texture coordinates of a face.
//!
//! A face takes its coordinates from one of four sources, chosen by its
//! mapping entry: explicit per-vertex coordinates stored with the model, the
//! default unit triangle, the barycentric projection onto a texture triangle
//! (kind 0), or one of three procedural projections around a texture
//! triangle's centre (kinds 1 to 3: cylindrical, cube, spherical). The
//! procedural kinds share a frame (centre plus a 3x3 matrix) built once per
//! texture triangle by [`texture_frames`].
//!
//! Each corner also gets a key that tells the vertex deduplication whether two
//! corners of different faces may share one uploaded vertex.

use crate::modelunlit::ModelUnlit;

/// The frame of a procedural texture triangle: the centre of the kept faces
/// that use it and the mapping matrix (absent for kind 0, which has none).
#[derive(Clone, Copy)]
pub(super) struct TextureFrame {
    pub centre: [i32; 3],
    pub matrix: Option<[f32; 9]>,
}

/// The coordinates and deduplication keys of one face.
pub(super) struct FaceTexture {
    /// `(u, v)` of the three corners.
    pub uv: [[f32; 2]; 3],
    /// One key per corner.
    pub keys: [i64; 3],
}

/// A quarter-turn multiple applied to a coordinate pair: 1 turns by a
/// quarter, 2 by a half, 3 by three quarters.
fn quarter_turn(u: f32, v: f32, turns: i32) -> [f32; 2] {
    match turns {
        1 => [-v, u],
        2 => [-u, -v],
        3 => [v, -u],
        _ => [u, v],
    }
}

/// The offset of `point` from `centre`, taken through the frame matrix.
fn in_frame(point: [i32; 3], centre: [i32; 3], matrix: &[f32; 9]) -> [f32; 3] {
    let dx = (point[0] - centre[0]) as f32;
    let dy = (point[1] - centre[1]) as f32;
    let dz = (point[2] - centre[2]) as f32;
    [
        matrix[2] * dz + matrix[0] * dx + matrix[1] * dy,
        matrix[5] * dz + matrix[3] * dx + matrix[4] * dy,
        matrix[8] * dz + matrix[6] * dx + matrix[7] * dy,
    ]
}

/// Spherical projection: longitude across `u`, latitude up `v`.
fn spherical_uv(
    point: [i32; 3],
    centre: [i32; 3],
    matrix: &[f32; 9],
    turns: i32,
    scroll: f32,
) -> [f32; 2] {
    let [x, y, z] = in_frame(point, centre, matrix);
    let length = f64::from(z * z + x * x + y * y).sqrt() as f32;
    let u = (f64::from(x).atan2(f64::from(z)) as f32) / 6.283_185_5_f32 + 0.5;
    let v = (f64::from(y / length).asin() as f32) / std::f32::consts::PI + 0.5 + scroll;
    quarter_turn(u, v, turns)
}

/// Cylindrical projection: angle around the axis across `u` (repeated
/// `repeat` times), height along it up `v`.
fn cylindrical_uv(
    point: [i32; 3],
    centre: [i32; 3],
    matrix: &[f32; 9],
    repeat: f32,
    turns: i32,
    scroll: f32,
) -> [f32; 2] {
    let [x, y, z] = in_frame(point, centre, matrix);
    let mut u = (f64::from(x).atan2(f64::from(z)) as f32) / 6.283_185_5_f32 + 0.5;
    if repeat != 1.0 {
        u *= repeat;
    }
    let v = y + 0.5 + scroll;
    quarter_turn(u, v, turns)
}

/// The cube face a normal points through: 0 and 1 are the up and down faces
/// (`y`), 2 and 3 the front and back (`z`), 4 and 5 the two sides (`x`).
pub(super) fn dominant_axis(x: f32, y: f32, z: f32) -> i32 {
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    if ay > ax && ay > az {
        if y > 0.0 {
            0
        } else {
            1
        }
    } else if az > ax && az > ay {
        if z > 0.0 {
            2
        } else {
            3
        }
    } else if x > 0.0 {
        4
    } else {
        5
    }
}

/// Cube projection onto the face `axis`, shifted per axis by `offset`.
fn cube_uv(
    point: [i32; 3],
    centre: [i32; 3],
    matrix: &[f32; 9],
    axis: i32,
    turns: i32,
    offset: [f32; 3],
) -> [f32; 2] {
    let [x, y, z] = in_frame(point, centre, matrix);
    let [ox, oy, oz] = offset;
    let (u, v) = match axis {
        0 => (ox + x + 0.5, -z + oz + 0.5),
        1 => (ox + x + 0.5, oz + z + 0.5),
        2 => (-x + ox + 0.5, -y + oy + 0.5),
        3 => (ox + x + 0.5, -y + oy + 0.5),
        4 => (oz + z + 0.5, -y + oy + 0.5),
        _ => (-z + oz + 0.5, -y + oy + 0.5),
    };
    quarter_turn(u, v, turns)
}

/// The mapping matrix of a texture triangle: a spin of `rotation` (units of
/// 1/256 turn) about the Y axis, carried onto `direction`, then scaled per
/// row.
fn frame_matrix(direction: [i32; 3], rotation: i32, scale: [f32; 3]) -> [f32; 9] {
    let [dx, dy, dz] = direction;
    let cos = f64::from(rotation as f32 * 0.024_543_693_f32).cos() as f32;
    let sin = f64::from(rotation as f32 * 0.024_543_693_f32).sin() as f32;
    let spin = [cos, 0.0, sin, 0.0, 1.0, 0.0, -sin, 0.0, cos];
    let mut out = [0.0_f32; 9];
    let mut axis_x = 1.0_f32;
    let mut axis_z = 0.0_f32;
    let tilt_cos = dy as f32 / 32767.0;
    let tilt_sin = -(f64::from(1.0 - tilt_cos * tilt_cos).sqrt() as f32);
    let one_minus_cos = 1.0 - tilt_cos;
    let horizontal = f64::from(dx * dx + dz * dz).sqrt() as f32;
    if horizontal == 0.0 && tilt_cos == 0.0 {
        out = spin;
    } else {
        if horizontal != 0.0 {
            axis_x = -dz as f32 / horizontal;
            axis_z = dx as f32 / horizontal;
        }
        let tilt = [
            axis_x * axis_x * one_minus_cos + tilt_cos,
            axis_z * tilt_sin,
            axis_x * axis_z * one_minus_cos,
            -axis_z * tilt_sin,
            tilt_cos,
            axis_x * tilt_sin,
            axis_x * axis_z * one_minus_cos,
            -axis_x * tilt_sin,
            axis_z * axis_z * one_minus_cos + tilt_cos,
        ];
        for row in 0..3 {
            let (a, b, c) = (spin[row * 3], spin[row * 3 + 1], spin[row * 3 + 2]);
            for column in 0..3 {
                out[row * 3 + column] =
                    c * tilt[6 + column] + a * tilt[column] + b * tilt[3 + column];
            }
        }
    }
    for row in 0..3 {
        for column in 0..3 {
            out[row * 3 + column] *= scale[row];
        }
    }
    out
}

/// The centre and matrix of every procedural texture triangle over the kept
/// faces (`faces`, source face indices). Empty when the model has no mapping
/// table. A face that names a texture triangle the model does not have fails
/// the build, as it does in the original client.
pub(super) fn texture_frames(
    source: &ModelUnlit,
    faces: &[i32],
) -> anyhow::Result<Vec<TextureFrame>> {
    let Some(mapping) = &source.face_mapping else {
        return Ok(Vec::new());
    };
    let count = source.texture_triangle_count as usize;
    let mut low = vec![[i32::MAX; 3]; count];
    let mut high = vec![[-2_147_483_647_i32; 3]; count];
    for &face in faces {
        let face = face as usize;
        let triangle = mapping[face];
        if triangle > -1 && triangle < 32766 {
            let triangle = triangle as usize;
            if triangle >= count {
                anyhow::bail!("face {face} maps texture triangle {triangle} of {count}");
            }
            for corner in [
                source.face_vertex1[face],
                source.face_vertex2[face],
                source.face_vertex3[face],
            ] {
                let position = corner_position(source, corner as usize);
                for axis in 0..3 {
                    low[triangle][axis] = low[triangle][axis].min(position[axis]);
                    high[triangle][axis] = high[triangle][axis].max(position[axis]);
                }
            }
        }
    }
    let mut frames = vec![
        TextureFrame {
            centre: [0; 3],
            matrix: None,
        };
        count
    ];
    if count == 0 {
        return Ok(frames);
    }
    let kinds = source.texture_triangle_type.as_ref().expect("types");
    for triangle in 0..count {
        let kind = kinds[triangle];
        if kind > 0 {
            let frame = &mut frames[triangle];
            for axis in 0..3 {
                frame.centre[axis] = (low[triangle][axis] + high[triangle][axis]) / 2;
            }
            let scale_x = source.texture_triangle_scale_x.as_ref().expect("sx");
            let scale_y = source.texture_triangle_scale_y.as_ref().expect("sy");
            let scale_z = source.texture_triangle_scale_z.as_ref().expect("sz");
            let (factor_x, factor_z, factor_y);
            if kind == 1 {
                let stretch = scale_x[triangle];
                if stretch == 0 {
                    factor_x = 1.0;
                    factor_z = 1.0;
                } else if stretch > 0 {
                    factor_x = 1.0;
                    factor_z = stretch as f32 / 1024.0;
                } else {
                    factor_z = 1.0;
                    factor_x = -stretch as f32 / 1024.0;
                }
                factor_y = 64.0 / scale_y[triangle] as f32;
            } else if kind == 2 {
                factor_x = 64.0 / scale_x[triangle] as f32;
                factor_y = 64.0 / scale_y[triangle] as f32;
                factor_z = 64.0 / scale_z[triangle] as f32;
            } else {
                factor_x = scale_x[triangle] as f32 / 1024.0;
                factor_y = scale_y[triangle] as f32 / 1024.0;
                factor_z = scale_z[triangle] as f32 / 1024.0;
            }
            let rotation =
                i32::from(source.texture_triangle_rotation.as_ref().expect("rot")[triangle] as u8);
            let direction = [
                i32::from(source.texture_triangle_vertex1.as_ref().expect("v1")[triangle]),
                i32::from(source.texture_triangle_vertex2.as_ref().expect("v2")[triangle]),
                i32::from(source.texture_triangle_vertex3.as_ref().expect("v3")[triangle]),
            ];
            frame.matrix = Some(frame_matrix(
                direction,
                rotation,
                [factor_x, factor_y, factor_z],
            ));
        }
    }
    Ok(frames)
}

fn corner_position(source: &ModelUnlit, corner: usize) -> [i32; 3] {
    [
        source.vertex_x[corner],
        source.vertex_y[corner],
        source.vertex_z[corner],
    ]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `z * c + x * a + y * b` of `p = [x, y, z]` with `[a, b, c]`, summed in that
/// order.
fn dot_zxy(p: [f32; 3], [a, b, c]: [f32; 3]) -> f32 {
    p[2] * c + p[0] * a + p[1] * b
}

/// Keeps a projection from stretching a face across its seam: a corner whose
/// coordinate is more than `half` from the first corner's moves by `wrap`
/// towards it. Returns the flag of each of the last two corners: 1 moved down,
/// 2 moved up, 0 not moved. The coordinate compared is `u` for even `turns`,
/// else `v`.
fn close_seams(uv: &mut [[f32; 2]; 3], turns: i32, half: f32, wrap: f32) -> [i32; 2] {
    let axis = usize::from(turns & 1 != 0);
    let mut flags = [0; 2];
    for corner in 1..3 {
        let first = uv[0][axis];
        let here = uv[corner][axis];
        if here - first > half {
            uv[corner][axis] -= wrap;
            flags[corner - 1] = 1;
        } else if first - here > half {
            uv[corner][axis] += wrap;
            flags[corner - 1] = 2;
        }
    }
    flags
}

/// Direct per-vertex coordinates stored with the model. A face whose stored
/// coordinates lie outside the model's table (or a model that has no table)
/// cannot be built: the original client throws while building it, so the
/// build fails here and the caller drops the model.
fn stored_coordinates(source: &ModelUnlit, face: usize) -> anyhow::Result<FaceTexture> {
    let missing = || anyhow::anyhow!("face {face} has stored texture coordinates but no table");
    let offset = |offsets: &Option<Vec<i8>>| -> anyhow::Result<i32> {
        let offsets = offsets.as_ref().ok_or_else(missing)?;
        Ok(i32::from(offsets[face]) & 0xFF)
    };
    let offsets = [
        offset(&source.face_texture_vertex_offset1)?,
        offset(&source.face_texture_vertex_offset2)?,
        offset(&source.face_texture_vertex_offset3)?,
    ];
    let first_texture_vertex = source.vertex_texture_vertex.as_ref().ok_or_else(missing)?;
    let us = source.texture_vertex_u.as_ref().ok_or_else(missing)?;
    let vs = source.texture_vertex_v.as_ref().ok_or_else(missing)?;
    let corners = [
        source.face_vertex1[face],
        source.face_vertex2[face],
        source.face_vertex3[face],
    ];
    let mut index = [0_i32; 3];
    let mut uv = [[0.0_f32; 2]; 3];
    for corner in 0..3 {
        let first = first_texture_vertex
            .get(corners[corner] as usize)
            .ok_or_else(missing)?;
        index[corner] = first.wrapping_add(offsets[corner]);
        let slot = usize::try_from(index[corner]).ok();
        let (Some(&u), Some(&v)) = (
            slot.and_then(|slot| us.get(slot)),
            slot.and_then(|slot| vs.get(slot)),
        ) else {
            anyhow::bail!(
                "face {face} reads texture vertex {} of {}",
                index[corner],
                us.len()
            );
        };
        uv[corner] = [u, v];
    }
    Ok(FaceTexture {
        uv,
        // Every corner is keyed by the first corner's texture vertex.
        keys: [i64::from(index[0]); 3],
    })
}

/// The barycentric projection of the face onto a kind-0 texture triangle.
fn projected_onto_triangle(source: &ModelUnlit, face: usize, triangle: usize) -> [[f32; 2]; 3] {
    let corners = [
        source.face_vertex1[face] as usize,
        source.face_vertex2[face] as usize,
        source.face_vertex3[face] as usize,
    ];
    let anchors = [
        source.texture_triangle_vertex1.as_ref().expect("v1")[triangle] as usize,
        source.texture_triangle_vertex2.as_ref().expect("v2")[triangle] as usize,
        source.texture_triangle_vertex3.as_ref().expect("v3")[triangle] as usize,
    ];
    let origin = corner_position(source, anchors[0]).map(|c| c as f32);
    let relative = |corner: usize| -> [f32; 3] {
        let p = corner_position(source, corner);
        [
            p[0] as f32 - origin[0],
            p[1] as f32 - origin[1],
            p[2] as f32 - origin[2],
        ]
    };
    let edge1 = relative(anchors[1]);
    let edge2 = relative(anchors[2]);
    let normal = cross(edge1, edge2);
    let axis_u = cross(edge2, normal);
    let scale_u = 1.0 / dot_zxy(edge1, axis_u);
    let axis_v = cross(edge1, normal);
    let scale_v = 1.0 / dot_zxy(edge2, axis_v);
    let mut uv = [[0.0; 2]; 3];
    for (slot, &corner) in corners.iter().enumerate() {
        let p = relative(corner);
        uv[slot][0] = dot_zxy(p, axis_u) * scale_u;
        uv[slot][1] = dot_zxy(p, axis_v) * scale_v;
    }
    uv
}

/// What a procedural (kind 1 to 3) projection gives a face.
struct Projection {
    uv: [[f32; 2]; 3],
    /// The cube face of a kind-2 projection, else 0.
    cube_axis: i32,
    /// The seam flags of the last two corners (see [`close_seams`]).
    seams: [i32; 2],
}

/// The projection of `face` around its texture triangle's frame. A kind above
/// 3 has no projection: its coordinates stay zero.
fn procedural(
    source: &ModelUnlit,
    frames: &[TextureFrame],
    face: usize,
    triangle: usize,
    kind: i8,
) -> anyhow::Result<Projection> {
    let corners = [
        corner_position(source, source.face_vertex1[face] as usize),
        corner_position(source, source.face_vertex2[face] as usize),
        corner_position(source, source.face_vertex3[face] as usize),
    ];
    let frame = &frames[triangle];
    let centre = frame.centre;
    let matrix = frame
        .matrix
        .ok_or_else(|| anyhow::anyhow!("texture triangle {triangle} has no frame"))?;
    let turns = i32::from(source.texture_triangle_direction.as_ref().expect("dir")[triangle]);
    let scroll = source.texture_triangle_speed.as_ref().expect("speed")[triangle] as f32 / 256.0;
    let mut out = Projection {
        uv: [[0.0; 2]; 3],
        cube_axis: 0,
        seams: [0; 2],
    };
    match kind {
        1 => {
            let repeat =
                source.texture_triangle_scale_z.as_ref().expect("sz")[triangle] as f32 / 1024.0;
            for (slot, &corner) in corners.iter().enumerate() {
                out.uv[slot] = cylindrical_uv(corner, centre, &matrix, repeat, turns, scroll);
            }
            out.seams = close_seams(&mut out.uv, turns, repeat / 2.0, repeat);
        }
        2 => {
            let translation_u = source.texture_triangle_translation_u.as_ref().expect("tu")
                [triangle] as f32
                / 256.0;
            let translation_v = source.texture_triangle_translation_v.as_ref().expect("tv")
                [triangle] as f32
                / 256.0;
            let edge1: [i32; 3] =
                std::array::from_fn(|i| corners[1][i].wrapping_sub(corners[0][i]));
            let edge2: [i32; 3] =
                std::array::from_fn(|i| corners[2][i].wrapping_sub(corners[0][i]));
            let normal = [
                edge1[1]
                    .wrapping_mul(edge2[2])
                    .wrapping_sub(edge1[2].wrapping_mul(edge2[1])),
                edge1[2]
                    .wrapping_mul(edge2[0])
                    .wrapping_sub(edge1[0].wrapping_mul(edge2[2])),
                edge1[0]
                    .wrapping_mul(edge2[1])
                    .wrapping_sub(edge1[1].wrapping_mul(edge2[0])),
            ];
            let scale_x =
                64.0 / source.texture_triangle_scale_x.as_ref().expect("sx")[triangle] as f32;
            let scale_y =
                64.0 / source.texture_triangle_scale_y.as_ref().expect("sy")[triangle] as f32;
            let scale_z =
                64.0 / source.texture_triangle_scale_z.as_ref().expect("sz")[triangle] as f32;
            let mapped_x = (matrix[2] * normal[2] as f32
                + matrix[0] * normal[0] as f32
                + matrix[1] * normal[1] as f32)
                / scale_x;
            let mapped_y = (matrix[5] * normal[2] as f32
                + matrix[3] * normal[0] as f32
                + matrix[4] * normal[1] as f32)
                / scale_y;
            let mapped_z = (matrix[8] * normal[2] as f32
                + matrix[6] * normal[0] as f32
                + matrix[7] * normal[1] as f32)
                / scale_z;
            out.cube_axis = dominant_axis(mapped_x, mapped_y, mapped_z);
            for (slot, &corner) in corners.iter().enumerate() {
                out.uv[slot] = cube_uv(
                    corner,
                    centre,
                    &matrix,
                    out.cube_axis,
                    turns,
                    [scroll, translation_u, translation_v],
                );
            }
        }
        3 => {
            for (slot, &corner) in corners.iter().enumerate() {
                out.uv[slot] = spherical_uv(corner, centre, &matrix, turns, scroll);
            }
            out.seams = close_seams(&mut out.uv, turns, 0.5, 1.0);
        }
        _ => {}
    }
    Ok(out)
}

/// The coordinates and keys of `face` (a source face index) with a real
/// material. `frames` come from [`texture_frames`].
pub(super) fn face_texture(
    source: &ModelUnlit,
    frames: &[TextureFrame],
    face: usize,
) -> anyhow::Result<FaceTexture> {
    let mapping = source.face_mapping.as_ref().map_or(-1, |m| m[face]);
    if mapping == 32766 {
        return stored_coordinates(source, face);
    }
    if mapping == -1 {
        return Ok(FaceTexture {
            uv: [[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]],
            keys: [65535, 131_071, 196_607],
        });
    }
    let triangle = (i32::from(mapping) & 0xFFFF) as usize;
    let kind = source
        .texture_triangle_type
        .as_ref()
        .and_then(|kinds| kinds.get(triangle))
        .copied()
        .ok_or_else(|| anyhow::anyhow!("face {face} maps texture triangle {triangle}"))?;
    let projection = if kind == 0 {
        Projection {
            uv: projected_onto_triangle(source, face, triangle),
            cube_axis: 0,
            seams: [0; 2],
        }
    } else {
        procedural(source, frames, face, triangle, kind)?
    };
    let base = i64::from((projection.cube_axis << 16) | triangle as i32);
    Ok(FaceTexture {
        uv: projection.uv,
        keys: [
            base,
            i64::from(projection.seams[0] << 19) | base,
            i64::from(projection.seams[1] << 19) | base,
        ],
    })
}
