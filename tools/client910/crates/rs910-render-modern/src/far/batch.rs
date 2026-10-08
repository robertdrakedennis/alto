//! Merged static meshes (milestone F3; performance plan bottleneck #8): many
//! placed models baked into one vertex, colour and index set, drawn with one
//! draw per batch (material, point-light slots, transparency) and run of
//! models at one level of detail, instead of one draw per model and
//! material.
//!
//! The modern client draws a loc container's locs as merged batches. A
//! [`MergedMesh`] is that container: its vertices are placed (each model's matrix applied,
//! positions relative to the mesh's `origin` so they keep their precision)
//! and each batch lays its models out in one order for every LOD, largest
//! category first and, within a category, largest model first. So the
//! models a container class keeps (the categories at or above it) are a
//! prefix, and at one view the models of one LOD are runs (a model's LOD
//! grows as it gets smaller or farther), and [`runs`] turns a frame's
//! choice into few draws.
//!
//! A container is stored as chunks of at most 65,536 vertices
//! ([`merge_chunks`]) with 16-bit indices, each one allocation in a loc page
//! of the far scene's arena (`frame::arenas`, the loc meshes' page format):
//! its draws are ordinary draw packets (`frame::submit`) with the chunk's
//! base vertex, its batch ranges offset by its first index.

use crate::far::lod::{LodMesh, LODS};
use crate::models::bounds::Bounds;
use crate::models::mesh::Vertex;

/// One model of a [`MergedMesh`]: the caller's slot, its bounds (a sphere
/// around its placed box, relative to the mesh's origin) and category.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ModelInfo {
    pub(crate) slot: u32,
    pub(crate) centre: [f32; 3],
    /// Half the placed box's size.
    pub(crate) half: [f32; 3],
    pub(crate) category: u8,
}

impl ModelInfo {
    /// The length of the box's half extent (the model radius).
    pub(crate) fn radius(&self) -> f32 {
        glam::Vec3::from(self.half).length()
    }
}

/// One draw batch of a [`MergedMesh`].
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Batch {
    pub(crate) material: i32,
    /// The instance's point-light slots (`Instance::p2`).
    pub(crate) lights: [f32; 4],
    /// The classic transparent list (drawn after the opaque batches).
    pub(crate) transparent: bool,
    /// Its models in layout order: the index into [`MergedMesh::models`]
    /// and, per LOD, `(first, count)` in [`MergedMesh::indices`].
    pub(crate) models: Vec<(u32, [(u32, u32); LODS])>,
}

/// See the module docs.
#[derive(Clone, Debug, Default)]
pub(crate) struct MergedMesh {
    /// Absolute fine position of the vertices' origin (y down).
    pub(crate) origin: [i32; 3],
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) colours: Vec<u32>,
    pub(crate) indices: Vec<u32>,
    pub(crate) batches: Vec<Batch>,
    pub(crate) models: Vec<ModelInfo>,
    /// Bounds of the vertices, relative to `origin`.
    pub(crate) bounds: [[f32; 3]; 2],
}

/// One model to merge.
pub(crate) struct Part<'a> {
    /// The caller's name for it (the near extension's entity id).
    pub(crate) slot: u32,
    pub(crate) mesh: &'a LodMesh,
    /// Its placement relative to the merged mesh's origin:
    /// matrix entries, column-major (translation in 12..15).
    pub(crate) matrix: [f32; 16],
    pub(crate) lights: [f32; 4],
    pub(crate) transparent: bool,
    pub(crate) category: u8,
}

/// Most vertices one merged mesh holds: its indices are 16-bit, drawn with
/// its base vertex (the loc pages' format, `frame::arenas`).
pub(crate) const MESH_VERTICES: usize = 1 << 16;

/// [`merge`] in chunks of at most [`MESH_VERTICES`] vertices (the models in
/// order; a model never spans two chunks).
pub(crate) fn merge_chunks<'a>(
    origin: [i32; 3],
    parts: impl IntoIterator<Item = Part<'a>>,
) -> Vec<MergedMesh> {
    let mut out = Vec::new();
    let mut group = Vec::new();
    let mut vertices = 0;
    for part in parts {
        let n = part.mesh.vertices.len();
        if vertices + n > MESH_VERTICES && !group.is_empty() {
            out.push(merge(origin, std::mem::take(&mut group)));
            vertices = 0;
        }
        vertices += n;
        group.push(part);
    }
    if !group.is_empty() {
        out.push(merge(origin, group));
    }
    out
}

impl MergedMesh {
    /// Its geometry as 16-bit streams (the loc pages' format; the mesh
    /// keeps its layout: batches, models, bounds).
    pub(crate) fn take_streams(&mut self) -> crate::models::mesh::ModelStreams {
        crate::models::mesh::ModelStreams {
            vertices: std::mem::take(&mut self.vertices),
            colours: std::mem::take(&mut self.colours),
            indices: std::mem::take(&mut self.indices)
                .into_iter()
                .map(|i| i as u16)
                .collect(),
            batches: Vec::new(),
        }
    }

    /// The GPU bytes of its geometry.
    pub(crate) fn bytes(&self) -> u64 {
        (self.vertices.len() * std::mem::size_of::<Vertex>()
            + self.colours.len() * 4
            + self.indices.len() * 2) as u64
    }
}

/// Merge `parts` at `origin` (see the module docs).
pub(crate) fn merge<'a>(origin: [i32; 3], parts: impl IntoIterator<Item = Part<'a>>) -> MergedMesh {
    let parts: Vec<Part<'a>> = parts.into_iter().collect();
    let mut out = MergedMesh {
        origin,
        bounds: [[f32::MAX; 3], [f32::MIN; 3]],
        ..MergedMesh::default()
    };
    // Place every model's vertices (input order).
    let mut bases = Vec::with_capacity(parts.len());
    for part in &parts {
        let m = part.matrix;
        let base = out.vertices.len() as u32;
        let apply3 = |v: [f32; 3]| {
            [
                m[0] * v[0] + m[4] * v[1] + m[8] * v[2],
                m[1] * v[0] + m[5] * v[1] + m[9] * v[2],
                m[2] * v[0] + m[6] * v[1] + m[10] * v[2],
            ]
        };
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &part.mesh.vertices {
            let r = apply3(v.pos);
            let pos = [r[0] + m[12], r[1] + m[13], r[2] + m[14]];
            for k in 0..3 {
                lo[k] = lo[k].min(pos[k]);
                hi[k] = hi[k].max(pos[k]);
            }
            let t = apply3([v.tangent[0], v.tangent[1], v.tangent[2]]);
            out.vertices.push(Vertex {
                pos,
                normal: apply3(v.normal),
                uv: v.uv,
                tangent: [t[0], t[1], t[2], v.tangent[3]],
            });
        }
        out.colours.extend_from_slice(&part.mesh.colours);
        if part.mesh.vertices.is_empty() {
            (lo, hi) = ([0.0; 3], [0.0; 3]);
        }
        for k in 0..3 {
            out.bounds[0][k] = out.bounds[0][k].min(lo[k]);
            out.bounds[1][k] = out.bounds[1][k].max(hi[k]);
        }
        out.models.push(ModelInfo {
            slot: part.slot,
            centre: std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5),
            half: std::array::from_fn(|k| (hi[k] - lo[k]) * 0.5),
            category: part.category,
        });
        bases.push(base);
    }
    // Lay the batches out: largest category, then largest model first.
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by(|&a, &b| {
        let (ia, ib) = (&out.models[a], &out.models[b]);
        ib.category
            .cmp(&ia.category)
            .then(ib.radius().total_cmp(&ia.radius()))
    });
    #[derive(Default)]
    struct Staging {
        lists: [Vec<u32>; LODS],
        models: Vec<(u32, [(u32, u32); LODS])>,
    }
    let mut keys: Vec<(i32, [u32; 4], bool)> = Vec::new();
    let mut staging: Vec<Staging> = Vec::new();
    for &p in &order {
        let part = &parts[p];
        for (material, lists) in &part.mesh.groups {
            let key = (*material, part.lights.map(f32::to_bits), part.transparent);
            let at = keys.iter().position(|k| *k == key).unwrap_or_else(|| {
                keys.push(key);
                staging.push(Staging::default());
                keys.len() - 1
            });
            let s = &mut staging[at];
            let mut ranges = [(0, 0); LODS];
            for (lod, list) in lists.iter().enumerate() {
                let first = s.lists[lod].len() as u32;
                s.lists[lod].extend(list.iter().map(|&i| i + bases[p]));
                ranges[lod] = (first, list.len() as u32);
            }
            match s.models.last_mut() {
                // A model's second group in the same batch (split materials).
                Some((m, r)) if *m == p as u32 => {
                    for lod in 0..LODS {
                        r[lod].1 += ranges[lod].1;
                    }
                }
                _ => s.models.push((p as u32, ranges)),
            }
        }
    }
    for ((material, lights, transparent), s) in keys.into_iter().zip(staging) {
        let mut firsts = [0_u32; LODS];
        for lod in 0..LODS {
            // A list equal to the previous one shares its range.
            if lod > 0 && s.lists[lod] == s.lists[lod - 1] {
                firsts[lod] = firsts[lod - 1];
                continue;
            }
            firsts[lod] = out.indices.len() as u32;
            out.indices.extend_from_slice(&s.lists[lod]);
        }
        out.batches.push(Batch {
            material,
            lights: lights.map(f32::from_bits),
            transparent,
            models: s
                .models
                .into_iter()
                .map(|(m, r)| {
                    (
                        m,
                        std::array::from_fn(|lod| (r[lod].0 + firsts[lod], r[lod].1)),
                    )
                })
                .collect(),
        });
    }
    if out.vertices.is_empty() {
        out.bounds = [[0.0; 3]; 2];
    }
    out
}

/// The draw ranges of `batch` at one frame: each of its models `lod` gives
/// a level for (`None`: not drawn), merged into runs of consecutive models
/// at the same level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DrawRun {
    pub(crate) first: u32,
    pub(crate) count: u32,
    /// Only the placed models actually drawn by this run, relative to the
    /// merged origin; skipped categories and LODs cannot widen this box.
    pub(crate) bounds: Bounds,
}

pub(crate) fn runs(
    batch: &Batch,
    models: &[ModelInfo],
    lod: impl Fn(u32) -> Option<u8>,
    out: &mut Vec<DrawRun>,
) {
    out.clear();
    let mut last: Option<u8> = None;
    for (m, ranges) in &batch.models {
        let Some(level) = lod(*m) else {
            last = None;
            continue;
        };
        let (first, count) = ranges[usize::from(level).min(LODS - 1)];
        if count == 0 {
            continue;
        }
        let model = &models[*m as usize];
        let bounds = Bounds {
            min: std::array::from_fn(|axis| model.centre[axis] - model.half[axis]),
            max: std::array::from_fn(|axis| model.centre[axis] + model.half[axis]),
        };
        match out.last_mut() {
            Some(run) if last == Some(level) && run.first + run.count == first => {
                run.count += count;
                for axis in 0..3 {
                    run.bounds.min[axis] = run.bounds.min[axis].min(bounds.min[axis]);
                    run.bounds.max[axis] = run.bounds.max[axis].max(bounds.max[axis]);
                }
            }
            _ => out.push(DrawRun {
                first,
                count,
                bounds,
            }),
        }
        last = Some(level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-triangle model of `material` and `size` whose coarser LODs
    /// drop the triangle.
    fn tri(material: i32, size: f32) -> LodMesh {
        let v = |x: f32, y: f32| Vertex {
            pos: [x * size, y * size, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
        };
        LodMesh {
            vertices: vec![v(0.0, 0.0), v(1.0, 0.0), v(0.0, 1.0)],
            colours: vec![1, 2, 3],
            groups: vec![(
                material,
                [vec![0, 1, 2], vec![0, 1, 2], vec![], vec![], vec![]],
            )],
        }
    }

    fn at<'a>(slot: u32, mesh: &'a LodMesh, x: f32, category: u8) -> Part<'a> {
        Part {
            slot,
            mesh,
            matrix: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, x, 0.0, 0.0, 1.0,
            ],
            lights: [0.0; 4],
            transparent: false,
            category,
        }
    }

    /// Placement moves positions and turns normals; batches lay their
    /// models out by category then size; equal LOD lists share a range; the
    /// runs join consecutive models at one level and split around the
    /// models left out.
    #[test]
    fn merged_batches_place_models_and_draw_runs() {
        let (small, big, other) = (tri(7, 1.0), tri(7, 10.0), tri(-1, 1.0));
        let turn = [
            0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 100.0, 0.0, 0.0, 1.0,
        ];
        let merged = merge(
            [512, 0, 512],
            [
                at(10, &small, 0.0, 0),
                at(11, &other, 10.0, 0),
                at(12, &big, 20.0, 2),
                Part {
                    matrix: turn,
                    ..at(13, &small, 0.0, 0)
                },
            ],
        );
        assert_eq!(merged.models.len(), 4);
        assert_eq!(merged.batches.len(), 2);
        // Model 3 turned a quarter about y, at x + 100.
        assert_eq!(merged.vertices[10].pos, [100.0, 0.0, -1.0]);
        assert_eq!(merged.vertices[10].normal, [1.0, 0.0, 0.0]);
        assert_eq!(merged.bounds, [[0.0, 0.0, -1.0], [100.0, 10.0, 0.0]]);
        // Batch 7: the category-2 model first, then the small ones.
        let batch = &merged.batches[0];
        assert_eq!(batch.material, 7);
        let slots: Vec<u32> = batch
            .models
            .iter()
            .map(|(m, _)| merged.models[*m as usize].slot)
            .collect();
        assert_eq!(slots, [12, 10, 13]);
        // LOD 1 equals LOD 0: stored once; LODs 2-4 are empty.
        assert_eq!(batch.models[0].1[0], batch.models[0].1[1]);
        assert_eq!(merged.indices.len(), 9 + 3);
        let mut out = Vec::new();
        runs(batch, &merged.models, |_| Some(0), &mut out);
        let ranges = |out: &[DrawRun]| {
            out.iter()
                .map(|run| (run.first, run.count))
                .collect::<Vec<_>>()
        };
        assert_eq!(ranges(&out), vec![(0, 9)]);
        assert_eq!(
            out[0].bounds,
            Bounds {
                min: [0.0, 0.0, -1.0],
                max: [100.0, 10.0, 0.0]
            }
        );
        // The class leaves the category-0 models out; a gap splits runs.
        runs(
            batch,
            &merged.models,
            |m| (merged.models[m as usize].category >= 1).then_some(0),
            &mut out,
        );
        assert_eq!(ranges(&out), vec![(0, 3)]);
        assert_eq!(
            out[0].bounds,
            Bounds {
                min: [20.0, 0.0, 0.0],
                max: [30.0, 10.0, 0.0]
            }
        );
        runs(
            batch,
            &merged.models,
            |m| (merged.models[m as usize].slot != 10).then_some(0),
            &mut out,
        );
        assert_eq!(ranges(&out), vec![(0, 3), (6, 3)]);
        assert_eq!(
            out[1].bounds,
            Bounds {
                min: [100.0, 0.0, -1.0],
                max: [100.0, 1.0, 0.0]
            }
        );
        // Mixed levels: LOD 4 draws nothing for these models.
        runs(
            batch,
            &merged.models,
            |m| {
                Some(if merged.models[m as usize].slot == 12 {
                    0
                } else {
                    4
                })
            },
            &mut out,
        );
        assert_eq!(ranges(&out), vec![(0, 3)]);
        assert_eq!(
            out[0].bounds,
            Bounds {
                min: [20.0, 0.0, 0.0],
                max: [30.0, 10.0, 0.0]
            }
        );
    }
}
