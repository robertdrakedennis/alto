//! CPU vertex streams of the NXT renderer's meshes (renderer plan M1): one
//! vertex format for floors and models (position, normal, UV; colour in a
//! second stream, per batch for floors, since the classic floor batches share
//! the positions but carry their own colours and ownership alpha).
//!
//! - Models: the `GpuModel` streams the snapshot's contract names
//!   (`position_stream`, `normal_stream`, `uv_stream`, `index_stream`,
//!   `batches`) with the colour stream the classic renderer made (`colour_stream`:
//!   the HSL colour with the model's own ambient, `GpuModel::scale_lightness`'s
//!   lightness scale by `ambient / 128`, where a loc's ambient is its config value
//!   plus 64). That stream carries no sun or direction: the faithful model
//!   shader lights it with the same sun as the floors, so it is the albedo
//!   the NXT lighting must multiply to match the faithful brightness (lane
//!   Q-LOOK: with `albedo_stream`, ambient 128, loc models drew up to twice
//!   as light and the offline views' mean value rose 15-20% above the
//!   faithful frames'; with `colour_stream` they are within 5%).
//!   `albedo_stream` stays available ([`Colour::Albedo`]).
//! - Floors: `FloorGeometry.stream0` positions and UVs with the classic grid
//!   normals (`FloorGeometry::normal_grid`/`normal_at`, the normals the
//!   faithful build keeps only with `(flags & 7) != 0`), each batch's colour
//!   stream, and its indices over the frame's tile selection
//!   (`FloorBatch::build_indices`, as `FloorMesh::select_tiles`).
//! - Tangents (M2, for the RT7 normal maps): per vertex, from the UVs of
//!   the triangles that use it ([`add_tangents`]). The modern client has no
//!   tangent stream to follow (its model and terrain shaders take no normal
//!   map; the only normal maps it samples are the flat water's, in world axes).

use crate::floor::FloorGeometry;
use crate::gpumodel::GpuModel;
use crate::texture::MaterialStore;

/// One vertex (stream 0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Unit tangent along +U (orthogonal to `normal`), and in `w` the side
    /// of the +V bitangent: `cross(normal, tangent) * w` (see
    /// [`add_tangents`]).
    pub tangent: [f32; 4],
}

/// Fill every vertex's tangent from the triangles of `indices` (a triangle
/// list over `vertices`): each triangle's UV gradient of position (the
/// directions of +U and +V on its plane), summed per vertex, then made
/// orthogonal to the vertex normal. A vertex without a usable UV gradient
/// (untextured faces: zero UVs) keeps any axis orthogonal to its normal;
/// the shader applies no normal map through such a vertex's material.
pub fn add_tangents(vertices: &mut [Vertex], indices: &[u16]) {
    let mut sums = vec![([0.0_f32; 3], [0.0_f32; 3]); vertices.len()];
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    for tri in indices.chunks_exact(3) {
        let [i0, i1, i2] = [tri[0], tri[1], tri[2]].map(usize::from);
        if i0 >= vertices.len() || i1 >= vertices.len() || i2 >= vertices.len() {
            continue;
        }
        let (v0, v1, v2) = (&vertices[i0], &vertices[i1], &vertices[i2]);
        let e1 = sub(v1.pos, v0.pos);
        let e2 = sub(v2.pos, v0.pos);
        let (du1, dv1) = (v1.uv[0] - v0.uv[0], v1.uv[1] - v0.uv[1]);
        let (du2, dv2) = (v2.uv[0] - v0.uv[0], v2.uv[1] - v0.uv[1]);
        let det = du1 * dv2 - du2 * dv1;
        if !det.is_finite() || det.abs() < 1e-12 {
            continue;
        }
        let r = 1.0 / det;
        let t: [f32; 3] = std::array::from_fn(|k| (e1[k] * dv2 - e2[k] * dv1) * r);
        let b: [f32; 3] = std::array::from_fn(|k| (e2[k] * du1 - e1[k] * du2) * r);
        if !t.iter().chain(&b).all(|v| v.is_finite()) {
            continue;
        }
        for i in [i0, i1, i2] {
            for k in 0..3 {
                sums[i].0[k] += t[k];
                sums[i].1[k] += b[k];
            }
        }
    }
    for (v, (t, b)) in vertices.iter_mut().zip(sums) {
        let n = glam::Vec3::from(v.normal).normalize_or_zero();
        let t = glam::Vec3::from(t);
        let mut tangent = (t - n * n.dot(t)).normalize_or_zero();
        if tangent == glam::Vec3::ZERO {
            tangent = if n == glam::Vec3::ZERO {
                glam::Vec3::X
            } else {
                n.any_orthonormal_vector()
            };
        }
        let side = if n.cross(tangent).dot(glam::Vec3::from(b)) < 0.0 {
            -1.0
        } else {
            1.0
        };
        v.tangent = [tangent.x, tangent.y, tangent.z, side];
    }
}

/// A model's streams: vertices, colours (`R | G << 8 | B << 16 | A << 24`,
/// the GLX byte order), indices and its material batches as `(material,
/// first index, index count)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelStreams {
    pub vertices: Vec<Vertex>,
    pub colours: Vec<u32>,
    pub indices: Vec<u16>,
    pub batches: Vec<(i32, u32, u32)>,
}

/// Which colour a model's stream carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Colour {
    /// `albedo_stream`: the HSL colour at ambient 128.
    Albedo,
    /// `colour_stream`: as the classic renderer made the model, at its own ambient (the
    /// scene's models, lit by the NXT pass, and the sky's unlit colours).
    Classic,
}

/// The streams of `model` (`None`: an empty model, or its colours do not
/// resolve against `materials`).
#[must_use]
pub fn model_streams(
    model: &GpuModel,
    materials: &MaterialStore,
    colour: Colour,
) -> Option<ModelStreams> {
    let positions = model.position_stream();
    if positions.is_empty() {
        return None;
    }
    let normals = model.normal_stream();
    let uvs = model.uv_stream();
    let colours = match colour {
        Colour::Albedo => model.albedo_stream(materials),
        Colour::Classic => model.colour_stream(materials),
    }
    .ok()?;
    let indices = model.index_stream();
    let mut vertices: Vec<Vertex> = positions
        .iter()
        .zip(&normals)
        .zip(&uvs)
        .map(|((&pos, &normal), &uv)| Vertex {
            pos,
            normal,
            uv,
            tangent: [0.0; 4],
        })
        .collect();
    add_tangents(&mut vertices, &indices);
    let mut batches = Vec::new();
    for (material, face_start, face_count, _, _) in model.batches() {
        let start = face_start as usize * 3;
        let end = (face_start + face_count) as usize * 3;
        if end <= start || end > indices.len() {
            continue;
        }
        batches.push((i32::from(material), start as u32, (end - start) as u32));
    }
    if batches.is_empty() {
        return None;
    }
    Some(ModelStreams {
        vertices,
        colours: colours.into_iter().map(|c| c as u32).collect(),
        indices,
        batches,
    })
}

/// A floor level's vertices: `stream0` positions and UVs with the classic grid
/// normals where the floor kept its construction calls, else the build's
/// own normals, else straight up. Tangents are zero until
/// [`add_tangents`] runs over the level's triangles.
#[must_use]
pub fn floor_vertices(geometry: &FloorGeometry) -> Vec<Vertex> {
    let stride = geometry.stride_floats;
    let grid = geometry.normal_grid();
    (0..geometry.vertex_count)
        .map(|i| {
            let f = &geometry.stream0[i * stride..(i + 1) * stride];
            let normal = match &grid {
                Some(grid) => geometry.normal_at(grid, f[0] as i32, f[2] as i32),
                None if geometry.has_normals => {
                    let k = if geometry.has_depth { 6 } else { 5 };
                    [f[k], f[k + 1], f[k + 2]]
                }
                None => [0.0, -1.0, 0.0],
            };
            Vertex {
                pos: [f[0], f[1], f[2]],
                normal,
                uv: [f[3], f[4]],
                tangent: [0.0; 4],
            }
        })
        .collect()
}

/// The UV scale of floor batch `scale` (the faithful floor's texture matrix).
#[must_use]
pub fn floor_uv_scale(scale: f32) -> f32 {
    if scale != 0.0 {
        1.0 / scale
    } else {
        0.0
    }
}
