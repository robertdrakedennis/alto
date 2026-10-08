//! RT7 geometry with its level-of-detail lists (`docs/renderer/modern-renderer.md`):
//! the
//! near path's RT7 correspondence ([`crate::models::rt7::build`]) extended
//! from LOD 0 to every index list an RT7 mesh carries.
//!
//! RT7 meshes hold up to five index lists over one vertex array
//! (`Rt7Mesh::lods`, LOD 0 first; the modern client binds the list of
//! the chosen LOD over a shared vertex buffer). LOD 0 is built exactly as
//! [`crate::models::rt7::build`] builds it (the same vertices in the same
//! order, the same material groups, the same tangents), so a batch drawn at LOD 0 is the near path's
//! mesh. The coarser lists index the same RT7 vertices; a vertex only they
//! use is placed through the same correspondence (its raw position's classic
//! source vertex) and takes the colour of a LOD 0 vertex at the same raw
//! position (the mesh's first face colour when there is none). Each list
//! joins the material group of its mesh's LOD 0 faces. A mesh with fewer
//! lists repeats its last one.
//!
//! The classic side is [`ClassicMesh`], a slim copy of the placed model (its
//! final positions, draw faces, colours and materials), so the build can
//! run on a worker thread without the model.

use std::collections::HashMap;
use std::sync::Arc;

use rs910_config::nxt::model_rt7::Rt7Model;

use crate::gpumodel::GpuModel;
use crate::models::mesh::Vertex;
use crate::models::rt7::{fit_affine, Reject, RIGID_TOLERANCE};
use crate::texture::MaterialStore;

/// Level-of-detail lists an RT7 mesh can carry (LOD 0 to 4).
pub(crate) const LODS: usize = 5;

/// What the RT7 correspondence reads of a placed classic model: its final
/// vertex positions (the first `vertex_count`), and per draw face (the
/// `draw_face_count` first) its corners' source vertices, the colour
/// stream entry of its first corner and its material.
#[derive(Clone, Debug, Default)]
pub(crate) struct ClassicMesh {
    pub(crate) positions: Vec<[i32; 3]>,
    pub(crate) faces: Vec<[u32; 3]>,
    pub(crate) colours: Vec<u32>,
    pub(crate) materials: Vec<i16>,
}

impl ClassicMesh {
    /// The slim copy of `model` (`None`: its colours do not resolve).
    pub(crate) fn of(model: &GpuModel, materials: &MaterialStore) -> Option<Self> {
        let count = (model.vertex_count.max(0) as usize).min(model.vx.len());
        let stream = model.colour_stream(materials).ok()?;
        let draw = model.draw_face_count.max(0) as usize;
        let mut mesh = Self {
            positions: (0..count)
                .map(|i| [model.vx[i], model.vy[i], model.vz[i]])
                .collect(),
            faces: Vec::with_capacity(draw),
            colours: Vec::with_capacity(draw),
            materials: Vec::with_capacity(draw),
        };
        // The classic index arrays are shorts read unsigned (models past 32,767
        // vertices exist among the far locs).
        let unsigned = |i: i16| usize::from(i as u16);
        for f in 0..draw {
            let corners = [model.idx1[f], model.idx2[f], model.idx3[f]];
            let source = corners.map(|u| {
                model
                    .unique_vertex
                    .get(unsigned(u))
                    .map_or(u32::MAX, |&s| u32::from(s as u16))
            });
            if source.iter().any(|&s| s as usize >= count) {
                return None;
            }
            mesh.faces.push(source);
            mesh.colours
                .push(*stream.get(unsigned(model.idx1[f]))? as u32);
            mesh.materials.push(model.face_material[f]);
        }
        Some(mesh)
    }
}

impl ClassicMesh {
    /// The draw faces RT7 has: all but those whose corners are one point
    /// (the classic draw-face rule over the copy).
    pub(crate) fn rt7_faces(&self) -> usize {
        self.faces
            .iter()
            .filter(|c| {
                let p = c.map(|s| self.positions[s as usize]);
                !(p[0] == p[1] && p[1] == p[2])
            })
            .count()
    }
}

/// One model's RT7 geometry with its LOD lists: vertices (the model's
/// space), colours, and material groups in RT7 mesh order, each with one
/// triangle list per LOD (indices into `vertices`).
#[derive(Clone, Debug, Default)]
pub(crate) struct LodMesh {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) colours: Vec<u32>,
    pub(crate) groups: Vec<(i32, [Vec<u32>; LODS])>,
}

impl LodMesh {
    /// A mesh of one detail level only (the classic mesh of a model without
    /// an RT7 counterpart): every LOD is `indices`' batches.
    pub(crate) fn single(
        vertices: Vec<Vertex>,
        colours: Vec<u32>,
        indices: &[u16],
        batches: &[(i32, u32, u32)],
    ) -> Self {
        let groups = batches
            .iter()
            .map(|&(m, first, count)| {
                let list: Vec<u32> = indices[first as usize..(first + count) as usize]
                    .iter()
                    .map(|&i| u32::from(i))
                    .collect();
                (m, std::array::from_fn(|_| list.clone()))
            })
            .collect();
        Self {
            vertices,
            colours,
            groups,
        }
    }

    /// Triangles of LOD `lod`.
    pub(crate) fn triangles(&self, lod: usize) -> usize {
        self.groups.iter().map(|(_, l)| l[lod].len() / 3).sum()
    }
}

/// Raw (model-space, classic axes) position key.
type Key = [i32; 3];

/// Build the RT7 geometry of placed model `classic` (its raw shape models
/// merged: `raw`; their RT7 copies: `parts`) with every LOD (see the module
/// docs).
pub(crate) fn build(
    classic: &ClassicMesh,
    raw: &crate::modelunlit::ModelUnlit,
    parts: &[Arc<Rt7Model>],
    y_scale: f32,
) -> Result<LodMesh, Reject> {
    let count = classic.positions.len();
    if count > raw.vertex_count as usize || count < raw.used_vertex_count as usize {
        return Err(Reject::OtherModel);
    }
    let raw_pos = |i: usize| -> Key { [raw.vertex_x[i], raw.vertex_y[i], raw.vertex_z[i]] };
    let mut by_pos: HashMap<Key, usize> = HashMap::new();
    for i in 0..count {
        by_pos.entry(raw_pos(i)).or_insert(i);
    }
    let mut faces: HashMap<[Key; 3], Vec<usize>> = HashMap::new();
    let mut classic_faces = 0;
    for (f, corners) in classic.faces.iter().enumerate() {
        let mut key = corners.map(|s| raw_pos(s as usize));
        if key[0] == key[1] && key[1] == key[2] {
            continue;
        }
        key.sort_unstable();
        classic_faces += 1;
        faces.entry(key).or_default().push(f);
    }
    let pairs: Vec<(glam::Vec3, glam::Vec3)> = (0..count)
        .map(|i| {
            let r = raw_pos(i);
            let p = classic.positions[i];
            (
                glam::Vec3::new(r[0] as f32, r[1] as f32, r[2] as f32),
                glam::Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32),
            )
        })
        .collect();
    let Some((mut linear, _, worst)) = fit_affine(&pairs) else {
        return Err(Reject::NonRigid);
    };
    if worst.x > RIGID_TOLERANCE || worst.z > RIGID_TOLERANCE {
        return Err(Reject::NonRigid);
    }
    if worst.y > RIGID_TOLERANCE {
        let (r0, r2) = (linear.row(0), linear.row(2));
        linear = glam::Mat3::from_cols(
            glam::Vec3::new(r0.x, 0.0, r2.x),
            glam::Vec3::new(r0.y, y_scale, r2.y),
            glam::Vec3::new(r0.z, 0.0, r2.z),
        );
    }
    let normal_matrix = linear.inverse().transpose();
    let mut out = LodMesh::default();
    let mut used = vec![false; classic.faces.len()];
    // The colour of the LOD 0 vertices by raw position (the coarser lists'
    // own vertices take it).
    let mut colour_at: HashMap<Key, u32> = HashMap::new();
    let vertex = |out: &mut LodMesh,
                  mesh: &rs910_config::nxt::model_rt7::Rt7Mesh,
                  c: usize,
                  src: usize,
                  colour: u32| {
        let n = mesh.normals[c];
        let n = normal_matrix * glam::Vec3::new(f32::from(n[0]), -f32::from(n[1]), f32::from(n[2]));
        let p = classic.positions[src];
        let v = out.vertices.len() as u32;
        out.vertices.push(Vertex {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            normal: n.normalize_or_zero().to_array(),
            uv: mesh.uvs[c],
            tangent: [0.0; 4],
        });
        out.colours.push(colour);
        v
    };
    // Per mesh, after its LOD 0: (part, mesh, group, first colour).
    let mut coarse: Vec<(usize, usize, usize, u32)> = Vec::new();
    // Each mesh's corner -> output vertex, kept for its coarser lists.
    let mut locals: Vec<Vec<Vec<Option<u32>>>> = parts
        .iter()
        .map(|p| {
            p.meshes
                .iter()
                .map(|m| vec![None; m.positions.len()])
                .collect()
        })
        .collect();
    for (pi, part) in parts.iter().enumerate() {
        for (mi, mesh) in part.meshes.iter().enumerate() {
            let Some(lod0) = mesh.lods.first() else {
                continue;
            };
            let local = &mut locals[pi][mi];
            let mut first: Option<(usize, u32)> = None;
            for tri in lod0.chunks_exact(3) {
                let corners = [tri[0], tri[1], tri[2]].map(usize::from);
                let keys = corners.map(|c| {
                    let p = mesh.positions[c];
                    [i32::from(p[0]), -i32::from(p[1]), i32::from(p[2])]
                });
                let mut sorted = keys;
                sorted.sort_unstable();
                let Some(&f) = faces
                    .get(&sorted)
                    .and_then(|list| list.iter().find(|&&f| !used[f]))
                else {
                    continue;
                };
                used[f] = true;
                if mesh.hidden() {
                    continue;
                }
                let colour = classic.colours[f];
                let material = i32::from(classic.materials[f]);
                let mut idx = [0_u32; 3];
                for (k, &c) in corners.iter().enumerate() {
                    let src = *by_pos.get(&keys[k]).ok_or(Reject::Unmatched)?;
                    let slot = match local[c] {
                        Some(v) if out.colours[v as usize] == colour => v,
                        _ => {
                            let v = vertex(&mut out, mesh, c, src, colour);
                            colour_at.entry(keys[k]).or_insert(colour);
                            local[c] = Some(v);
                            v
                        }
                    };
                    idx[k] = slot;
                }
                let tri = [idx[0], idx[2], idx[1]];
                match out.groups.last_mut() {
                    Some((m, lists)) if *m == material => lists[0].extend_from_slice(&tri),
                    _ => out
                        .groups
                        .push((material, [tri.to_vec(), vec![], vec![], vec![], vec![]])),
                }
                if first.is_none() {
                    first = Some((out.groups.len() - 1, colour));
                }
            }
            if let Some((group, colour)) = first {
                coarse.push((pi, mi, group, colour));
            }
        }
    }
    if out.vertices.len() > usize::from(u16::MAX) {
        return Err(Reject::TooLarge);
    }
    if used.iter().filter(|&&u| u).count() != classic_faces {
        return Err(Reject::Unmatched);
    }
    // LOD 0's tangents, as the near path's.
    let lod0: Vec<u16> = out
        .groups
        .iter()
        .flat_map(|(_, l)| l[0].iter().map(|&i| i as u16))
        .collect();
    crate::models::mesh::add_tangents(&mut out.vertices, &lod0);
    let lod0_vertices = out.vertices.len();
    // The coarser lists.
    for (pi, mi, group, first_colour) in coarse {
        let mesh = &parts[pi].meshes[mi];
        for lod in 1..LODS {
            let Some(list) = mesh.lods.get(lod).or_else(|| mesh.lods.last()) else {
                continue;
            };
            for tri in list.chunks_exact(3) {
                let corners = [tri[0], tri[1], tri[2]].map(usize::from);
                let mut idx = [0_u32; 3];
                let mut ok = true;
                for (k, &c) in corners.iter().enumerate() {
                    if let Some(v) = locals[pi][mi][c] {
                        idx[k] = v;
                        continue;
                    }
                    let p = mesh.positions[c];
                    let key = [i32::from(p[0]), -i32::from(p[1]), i32::from(p[2])];
                    let Some(&src) = by_pos.get(&key) else {
                        ok = false;
                        break;
                    };
                    let colour = colour_at.get(&key).copied().unwrap_or(first_colour);
                    let v = vertex(&mut out, mesh, c, src, colour);
                    locals[pi][mi][c] = Some(v);
                    idx[k] = v;
                }
                if ok {
                    out.groups[group].1[lod].extend_from_slice(&[idx[0], idx[2], idx[1]]);
                }
            }
        }
    }
    if out.vertices.len() > lod0_vertices {
        if out.vertices.len() > usize::from(u16::MAX) {
            return Err(Reject::TooLarge);
        }
        // The coarser lists' own vertices take their tangents from those
        // lists; LOD 0's keep theirs.
        let mut scratch = out.vertices.clone();
        let coarse: Vec<u16> = out
            .groups
            .iter()
            .flat_map(|(_, l)| l[1..].iter().flatten().map(|&i| i as u16))
            .collect();
        crate::models::mesh::add_tangents(&mut scratch, &coarse);
        for (v, s) in out.vertices[lod0_vertices..]
            .iter_mut()
            .zip(&scratch[lod0_vertices..])
        {
            v.tangent = s.tangent;
        }
    }
    Ok(out)
}
