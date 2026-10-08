//! RT7 geometry for animated models (renderer plan §4(o)):
//! NPC bodies, player bodies, dynamic (animated) locs, spot anims,
//! projectiles and the other transient models draw their RT7 meshes
//! (archive 47) posed exactly as their classic models, where M10
//! ([`crate::models::rt7`]) draws only the static locs from RT7.
//!
//! # How a posed RT7 mesh is built
//!
//! The snapshot hands each animated entity its classic model already posed on
//! the CPU (`GpuModel::apply_animation` and the actor's placement), and the
//! model names the archive-7 groups its faces came from, in face order
//! (`GpuModel::source_ids`, provenance kept by `ModelUnlit::load` and
//! `merge_slots`; RT7 group `n` is classic model `n` re-exported,
//! `nxt-data-formats.md` §9). Per distinct model structure (the ids and the
//! classic face/vertex topology, [`fingerprint`]) an [`AnimMap`] is built
//! once and cached:
//!
//! - each RT7 LOD-0 triangle of part `p` is matched to the part's raw face
//!   with the same three raw positions (M10's correspondence, but per part
//!   and in the part's own raw space, so the per-slot offsets, rotations
//!   and BAS transforms applied before the merge do not matter);
//! - the raw face is the merged face `offset(p) + face`, and the classic draw
//!   face that holds it is found through `GpuModel::face_source` (the
//!   constructor's sorted kept faces);
//! - each RT7 corner takes the classic *source vertex* of the matching corner
//!   of that classic face (`unique_vertex[idx*]`): corner `j` of the raw face
//!   is corner `j` of the classic face, or corner `2 - j` when the model was
//!   mirrored (`GpuModel::mirror` swaps `idx1`/`idx3`); the orientation is
//!   decided once per model by which of the two gives every raw vertex one
//!   classic vertex;
//! - colour, alpha and material are the classic face's (as M10), UVs RT7's.
//!
//! Every frame the positions are gathered from the posed classic vertices
//! (so an RT7 vertex sits exactly on its classic vertex in every frame), the
//! colours from the classic colour stream (tints and colour ops are the classic ones).
//!
//! # Normals and tangents: the bone transform
//!
//! The modern client skins on the GPU: it builds per-label bone matrices,
//! and its vertex shader transforms position, normal and tangent by the
//! vertex's bone matrix times the model matrix (the normal and the tangent
//! direction by the matrix's linear part, normalised, the handedness kept).
//! The classic animation ops move whole label groups (`gpumodel_anim.rs`
//! `apply_animation`), so each group's raw -> posed map is affine; this
//! module recovers its linear part `L` per (part, label) group by least
//! squares over the group's vertices (the base covariance inverted once per
//! map) and transforms the RT7 normal and tangent by it, as the bone
//! matrix's linear part does. A group the fit cannot determine (fewer than four
//! points or a flat or thin group) or does not fit (a contoured loc's
//! `hillchange` drape) uses the rotation of its vertices' own posed
//! triangles instead (each triangle's base -> posed edge/normal frame,
//! summed over the vertex's triangles in its group): the same rotation for
//! a rigid group, and the surface's own turn where the map is not affine.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use glam::{Mat3, Vec3};
use rs910_config::nxt::model_rt7::Rt7Model;

use crate::gpumodel::GpuModel;
use crate::models::draw_list::{EntityDraw, Kind};
use crate::models::mesh::{ModelStreams, Vertex};
use crate::modelunlit::ModelUnlit;
use crate::scene_snapshot::SceneSnapshot;
use crate::texture::MaterialStore;

/// Counters of the animated RT7 path (cumulative over the renderer's life).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnimStats {
    /// Entity draws posed from RT7.
    pub drawn: usize,
    /// Their triangles.
    pub faces: usize,
    /// Maps built (distinct model structures).
    pub maps: usize,
    /// A model without provenance (`GpuModel::source_ids`).
    pub no_ids: usize,
    /// A part without an RT7 or classic model.
    pub no_rt7: usize,
    /// The ids do not add up to the classic model's faces.
    pub other_model: usize,
    /// A classic draw face without an RT7 triangle, or corners that do not
    /// correspond.
    pub unmatched: usize,
    /// More vertices than 16-bit indices address.
    pub too_large: usize,
    /// RT7 corners whose vertex label differs from their classic vertex's (in
    /// the maps built; 0 when the correspondence is right).
    pub label_mismatches: usize,
    /// Label groups posed by their affine fit / by their triangles'
    /// rotations (per drawn entity).
    pub fit_groups: usize,
    pub face_groups: usize,
}

/// The per-renderer state: the maps by structure, the counters, the check.
#[derive(Default)]
pub struct AnimCache {
    maps: HashMap<u64, Option<Arc<AnimMap>>>,
    pub stats: AnimStats,
    /// CPU time building maps / posing (the log and the cost test).
    pub build_time: Duration,
    pub pose_time: Duration,
    /// `CLIENT910_MODERN_CHECK`: this frame's checked entities and the ones
    /// whose triangles are not their classic faces; frames with a mismatch.
    frame: u64,
    checked: usize,
    bad: usize,
    pub mismatch_frames: usize,
    /// The last frame's `(checked, bad)` (tests).
    pub last_check: (usize, usize),
    scratch: PoseScratch,
}

/// Reusable per-pose buffers.
#[derive(Default)]
struct Scratch {
    pos: Vec<Vec3>,
    linear: Vec<Option<Mat3>>,
    rot: Vec<Option<Mat3>>,
}

/// Maps kept before the cache is cleared (a new scene or many actors).
const MAX_MAPS: usize = 4096;

/// Why a model keeps its classic mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    OtherModel,
    Unmatched,
    TooLarge,
}

/// One label group of a map: its members (output vertices) and the base
/// side of its least-squares fit.
struct Group {
    members: std::ops::Range<u32>,
    centre: Vec3,
    /// `(sum d d^T)^-1` over the members' centred base positions, when the
    /// group spans three dimensions.
    inverse: Option<Mat3>,
    /// Largest base distance from the centre (the fit tolerance).
    radius: f32,
}

/// The cached correspondence of one model structure (see the module docs).
pub struct AnimMap {
    /// Per output vertex: the classic source vertex, the classic face whose
    /// colour it carries, its group, the RT7 base position, normal (unit)
    /// and UV-derived tangent (classic axes), and UV.
    src: Vec<u32>,
    face: Vec<u32>,
    group: Vec<u32>,
    base: Vec<Vec3>,
    normal: Vec<Vec3>,
    tangent: Vec<[f32; 4]>,
    uv: Vec<[f32; 2]>,
    indices: Vec<u16>,
    batches: Vec<(i32, u32, u32)>,
    /// Per triangle: its classic face and the transposed base frame.
    tri_face: Vec<u32>,
    tri_base: Vec<Option<Mat3>>,
    /// Per vertex, its triangles (in its group when it has any there).
    incident_start: Vec<u32>,
    incident: Vec<u32>,
    groups: Vec<Group>,
    group_members: Vec<u32>,
    /// The model was mirrored (classic winding reversed).
    mirrored: bool,
    /// RT7 corners whose label is not their classic vertex's.
    label_mismatches: usize,
}

impl AnimMap {
    /// Output triangles (one per non-degenerate classic draw face).
    #[must_use]
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// `v as u16 as usize`: the classic `short` indices read unsigned.
fn us(v: i16) -> usize {
    usize::from(v as u16)
}

/// The classic source vertices of draw face `f`'s corners.
fn corners(model: &GpuModel, f: usize) -> [usize; 3] {
    [model.idx1[f], model.idx2[f], model.idx3[f]].map(|u| us(model.unique_vertex[us(u)]))
}

/// A cache key for `model`'s structure under `ids`: the ids, the counts
/// and every draw face's source face and corner source vertices (what the
/// map depends on; positions, colours and alphas may change per frame).
#[must_use]
pub fn fingerprint(model: &GpuModel, ids: &[u32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        h = (h ^ v).wrapping_mul(0x0000_0100_0000_01b3);
        h ^= h >> 29;
    };
    mix(ids.len() as u64);
    for &id in ids {
        mix(u64::from(id));
    }
    mix(u64::from(model.vertex_count as u32) | u64::from(model.source_face_count as u32) << 32);
    mix(u64::from(model.face_count as u32) | u64::from(model.draw_face_count as u32) << 32);
    let draw = (model.draw_face_count.max(0) as usize)
        .min(model.face_source.len())
        .min(model.idx1.len());
    for f in 0..draw {
        let [a, b, c] = corners(model, f);
        mix((a as u64) | (b as u64) << 16 | (c as u64) << 32 | (model.face_source[f] as u64) << 48);
    }
    h
}

/// The edge/normal frame of a triangle (`None` when degenerate).
fn frame(p0: Vec3, p1: Vec3, p2: Vec3) -> Option<Mat3> {
    let e = p1 - p0;
    let n = e.cross(p2 - p0);
    // (NaN compares false: a non-finite triangle is degenerate too.)
    let usable =
        |v: Vec3| v.length_squared().partial_cmp(&1e-8) == Some(std::cmp::Ordering::Greater);
    if !usable(n) || !usable(e) {
        return None;
    }
    let e = e.normalize();
    let n = n.normalize();
    Some(Mat3::from_cols(e, n.cross(e), n))
}

/// Build the map of `model` whose faces are `raws` (raw classic models, times
/// 4 below version 13, as RT7) with RT7 copies `parts`.
pub fn build_map(
    model: &GpuModel,
    raws: &[Arc<ModelUnlit>],
    parts: &[Arc<Rt7Model>],
) -> Result<AnimMap, Reject> {
    let total: usize = raws.iter().map(|r| r.face_count.max(0) as usize).sum();
    let draw = model.draw_face_count.max(0) as usize;
    if total != model.source_face_count.max(0) as usize
        || model.face_source.len() < draw
        || model.idx1.len() < draw
        || raws.len() != parts.len()
    {
        return Err(Reject::OtherModel);
    }
    let mut offsets = Vec::with_capacity(raws.len());
    let mut sum = 0;
    for r in raws {
        offsets.push(sum);
        sum += r.face_count.max(0) as usize;
    }
    let part_of = |sf: usize| offsets.partition_point(|&o| o <= sf) - 1;
    let vertices = model.vx.len().min(model.vy.len()).min(model.vz.len());
    // Merged face -> classic draw face.
    let mut inv = vec![u32::MAX; total];
    for f in 0..draw {
        let sf = model.face_source[f];
        let Some(slot) = usize::try_from(sf).ok().and_then(|sf| inv.get_mut(sf)) else {
            return Err(Reject::OtherModel);
        };
        *slot = f as u32;
        if corners(model, f).iter().any(|&s| s >= vertices) {
            return Err(Reject::OtherModel);
        }
    }
    let raw_face = |p: usize, lf: usize| -> [usize; 3] {
        let r = &raws[p];
        [
            us(r.face_vertex1[lf]),
            us(r.face_vertex2[lf]),
            us(r.face_vertex3[lf]),
        ]
    };
    let raw_pos = |p: usize, v: usize| -> [i32; 3] {
        let r = &raws[p];
        [r.vertex_x[v], r.vertex_y[v], r.vertex_z[v]]
    };
    // Orientation: raw corner j is classic corner j (straight) or 2 - j
    // (mirrored); the right one maps every raw vertex to one classic vertex.
    let mut conflicts = [0_usize; 2];
    let mut expected = 0;
    {
        let mut maps: [Vec<Vec<u32>>; 2] = [
            raws.iter()
                .map(|r| vec![u32::MAX; r.vertex_count.max(0) as usize])
                .collect(),
            raws.iter()
                .map(|r| vec![u32::MAX; r.vertex_count.max(0) as usize])
                .collect(),
        ];
        for f in 0..draw {
            let sf = model.face_source[f] as usize;
            let p = part_of(sf);
            let lf = sf - offsets[p];
            let local = raw_face(p, lf);
            if local.iter().any(|&v| v >= raws[p].vertex_count as usize) {
                return Err(Reject::OtherModel);
            }
            let pos = local.map(|v| raw_pos(p, v));
            if pos[0] == pos[1] && pos[1] == pos[2] {
                continue;
            }
            expected += 1;
            let classic = corners(model, f);
            for (h, map) in maps.iter_mut().enumerate() {
                for j in 0..3 {
                    let s = if h == 0 { classic[j] } else { classic[2 - j] } as u32;
                    let slot = &mut map[p][local[j]];
                    if *slot == u32::MAX {
                        *slot = s;
                    } else if *slot != s {
                        conflicts[h] += 1;
                    }
                }
            }
        }
    }
    let mirrored = match conflicts {
        [0, _] => false,
        [_, 0] => true,
        _ => return Err(Reject::Unmatched),
    };
    // classic vertex labels (`vertex_groups`, label -> vertices).
    let classic_labels: Option<Vec<i32>> = model.vertex_groups.as_ref().map(|groups| {
        let mut labels = vec![-1; vertices];
        for (label, list) in groups.iter().enumerate() {
            for &v in list {
                if let Some(l) = labels.get_mut(v) {
                    *l = label as i32;
                }
            }
        }
        labels
    });
    let mut map = AnimMap {
        src: Vec::new(),
        face: Vec::new(),
        group: Vec::new(),
        base: Vec::new(),
        normal: Vec::new(),
        tangent: Vec::new(),
        uv: Vec::new(),
        indices: Vec::new(),
        batches: Vec::new(),
        tri_face: Vec::new(),
        tri_base: Vec::new(),
        incident_start: Vec::new(),
        incident: Vec::new(),
        groups: Vec::new(),
        group_members: Vec::new(),
        mirrored,
        label_mismatches: 0,
    };
    let mut group_ids: HashMap<(usize, i16), u32> = HashMap::new();
    // (classic face, triangle) in RT7 mesh order.
    let mut tris: Vec<(u32, [u16; 3])> = Vec::new();
    type Key = [i32; 3];
    for (p, part) in parts.iter().enumerate() {
        // The part's non-degenerate raw faces by their sorted positions.
        let r = &raws[p];
        let mut by_pos: HashMap<[Key; 3], Vec<u32>> = HashMap::new();
        for lf in 0..r.face_count.max(0) as usize {
            let local = raw_face(p, lf);
            if local.iter().any(|&v| v >= r.vertex_count as usize) {
                return Err(Reject::OtherModel);
            }
            let mut key = local.map(|v| raw_pos(p, v));
            if key[0] == key[1] && key[1] == key[2] {
                continue;
            }
            key.sort_unstable();
            by_pos.entry(key).or_default().push(lf as u32);
        }
        let mut used = vec![false; r.face_count.max(0) as usize];
        for mesh in &part.meshes {
            let Some(lod0) = mesh.lods.first() else {
                continue;
            };
            let hidden = mesh.hidden();
            let mut slots: HashMap<(u16, u32, i16, i8, i16), u32> = HashMap::new();
            for (t, tri) in lod0.chunks_exact(3).enumerate() {
                let rt7 = [tri[0], tri[1], tri[2]].map(usize::from);
                if rt7.iter().any(|&c| c >= mesh.positions.len()) {
                    return Err(Reject::Unmatched);
                }
                let keys: [Key; 3] = rt7.map(|c| {
                    let q = mesh.positions[c];
                    [i32::from(q[0]), -i32::from(q[1]), i32::from(q[2])]
                });
                let mut sorted = keys;
                sorted.sort_unstable();
                // The first unused raw face on these positions, one classic
                // draws for a drawn mesh (one it does not for a hidden one)
                // when there is a choice.
                let Some(list) = by_pos.get(&sorted) else {
                    continue;
                };
                let drawn = |lf: u32| inv[offsets[p] + lf as usize] != u32::MAX;
                let Some(&lf) = list
                    .iter()
                    .find(|&&lf| !used[lf as usize] && drawn(lf) != hidden)
                    .or_else(|| list.iter().find(|&&lf| !used[lf as usize]))
                else {
                    continue;
                };
                used[lf as usize] = true;
                let f = inv[offsets[p] + lf as usize];
                if hidden || f == u32::MAX {
                    continue;
                }
                let f = f as usize;
                let local = raw_face(p, lf as usize);
                let local_pos = local.map(|v| raw_pos(p, v));
                let classic = corners(model, f);
                let colour = model.face_colour[f];
                let alpha = model.face_alpha[f];
                let face_label = mesh.face_labels.get(t).copied().unwrap_or(-1);
                let mut idx = [0_u32; 3];
                let mut taken = [false; 3];
                for k in 0..3 {
                    let j = (0..3)
                        .find(|&j| !taken[j] && local_pos[j] == keys[k])
                        .or_else(|| (0..3).find(|&j| local_pos[j] == keys[k]))
                        .ok_or(Reject::Unmatched)?;
                    taken[j] = true;
                    let s = if mirrored { classic[2 - j] } else { classic[j] } as u32;
                    let c = rt7[k];
                    let rt7_label = mesh.vertex_labels.get(c).copied();
                    // The vertex moves with its classic label: a vertex the
                    // merge shared between parts (`vertexSourceModels`, more
                    // than one bit) keeps the first part's, which may differ
                    // from this part's RT7 label; elsewhere they are equal.
                    let label = match &classic_labels {
                        Some(labels) => {
                            let classic = labels[s as usize];
                            let shared = model
                                .vertex_source_models
                                .as_ref()
                                .and_then(|m| m.get(s as usize))
                                .is_some_and(|&m| (m as u16).count_ones() > 1);
                            if rt7_label.is_some_and(|l| i32::from(l) != classic) && !shared {
                                map.label_mismatches += 1;
                            }
                            classic as i16
                        }
                        None => rt7_label.unwrap_or(-1),
                    };
                    let key = (c as u16, s, colour, alpha, face_label);
                    let v = match slots.get(&key) {
                        Some(&v) => v,
                        None => {
                            let v = map.src.len() as u32;
                            let next = group_ids.len() as u32;
                            let g = *group_ids.entry((p, label)).or_insert(next);
                            let n = mesh.normals.get(c).copied().unwrap_or([0, 0, 0]);
                            map.src.push(s);
                            map.face.push(f as u32);
                            map.group.push(g);
                            map.base.push(Vec3::new(
                                keys[k][0] as f32,
                                keys[k][1] as f32,
                                keys[k][2] as f32,
                            ));
                            map.normal.push(
                                Vec3::new(f32::from(n[0]), -f32::from(n[1]), f32::from(n[2]))
                                    .normalize_or_zero(),
                            );
                            map.uv.push(mesh.uvs.get(c).copied().unwrap_or([0.0; 2]));
                            slots.insert(key, v);
                            v
                        }
                    };
                    idx[k] = v;
                }
                if map.src.len() > usize::from(u16::MAX) {
                    return Err(Reject::TooLarge);
                }
                // The classic winding: RT7 (a, b, c) is classic (f1, f3, f2); a
                // mirrored model's classic faces are wound the other way.
                let tri = if mirrored {
                    [idx[0], idx[1], idx[2]]
                } else {
                    [idx[0], idx[2], idx[1]]
                }
                .map(|i| i as u16);
                tris.push((f as u32, tri));
            }
        }
    }
    if tris.len() != expected {
        return Err(Reject::Unmatched);
    }
    // The classic draw order: the classic faces are sorted by priority and material
    // (`GpuModel`'s constructor), so the transparent ones (fountain water,
    // windows) draw after the faces behind them; RT7's mesh order does not
    // keep that. Triangles in their classic face's order, batched by material.
    tris.sort_by_key(|&(f, _)| f);
    for (f, tri) in tris {
        let material = i32::from(model.face_material[f as usize]);
        let at = map.indices.len() as u32;
        match map.batches.last_mut() {
            Some((m, _, count)) if *m == material => *count += 3,
            _ => map.batches.push((material, at, 3)),
        }
        map.indices.extend_from_slice(&tri);
        map.tri_face.push(f);
    }
    // Base tangents from the RT7 UVs at the base pose (M2's convention).
    let mut base_vertices: Vec<Vertex> = (0..map.src.len())
        .map(|v| Vertex {
            pos: map.base[v].to_array(),
            normal: map.normal[v].to_array(),
            uv: map.uv[v],
            tangent: [0.0; 4],
        })
        .collect();
    crate::models::mesh::add_tangents(&mut base_vertices, &map.indices);
    map.tangent = base_vertices.iter().map(|v| v.tangent).collect();
    // Base frames of the triangles.
    map.tri_base = map
        .indices
        .chunks_exact(3)
        .map(|t| {
            frame(
                map.base[usize::from(t[0])],
                map.base[usize::from(t[1])],
                map.base[usize::from(t[2])],
            )
            .map(|m| m.transpose())
        })
        .collect();
    // Groups: members and the base side of the fit.
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); group_ids.len()];
    for (v, &g) in map.group.iter().enumerate() {
        members[g as usize].push(v as u32);
    }
    for list in members {
        let start = map.group_members.len() as u32;
        let n = list.len() as f32;
        let centre = list
            .iter()
            .fold(Vec3::ZERO, |a, &v| a + map.base[v as usize])
            / n.max(1.0);
        let mut m = Mat3::ZERO;
        let mut radius = 0.0_f32;
        for &v in &list {
            let d = map.base[v as usize] - centre;
            m += Mat3::from_cols(d * d.x, d * d.y, d * d.z);
            radius = radius.max(d.length());
        }
        let trace = m.x_axis.x + m.y_axis.y + m.z_axis.z;
        let det = m.determinant();
        let inverse = (list.len() >= 4 && trace > 0.0 && det > 1e-4 * (trace / 3.0).powi(3))
            .then(|| m.inverse());
        map.group_members.extend_from_slice(&list);
        map.groups.push(Group {
            members: start..map.group_members.len() as u32,
            centre,
            inverse,
            radius,
        });
    }
    // Incident triangles per vertex, those inside its group preferred.
    let mut all: Vec<Vec<u32>> = vec![Vec::new(); map.src.len()];
    let mut inside: Vec<Vec<u32>> = vec![Vec::new(); map.src.len()];
    for (t, tri) in map.indices.chunks_exact(3).enumerate() {
        let g = [tri[0], tri[1], tri[2]].map(|i| map.group[usize::from(i)]);
        let same = g[0] == g[1] && g[1] == g[2];
        for &i in tri {
            all[usize::from(i)].push(t as u32);
            if same {
                inside[usize::from(i)].push(t as u32);
            }
        }
    }
    for (a, i) in all.into_iter().zip(inside) {
        map.incident_start.push(map.incident.len() as u32);
        map.incident.extend(if i.is_empty() { a } else { i });
    }
    map.incident_start.push(map.incident.len() as u32);
    Ok(map)
}

impl AnimMap {
    /// RT7 corners whose label is not their classic vertex's.
    #[must_use]
    pub fn label_mismatches(&self) -> usize {
        self.label_mismatches
    }

    /// Whether the map found the model mirrored.
    #[must_use]
    pub fn is_mirrored(&self) -> bool {
        self.mirrored
    }

    /// Output vertex `v`'s RT7 base position (classic axes).
    #[must_use]
    pub fn base_position(&self, v: usize) -> [f32; 3] {
        self.base[v].to_array()
    }

    /// [`Self::check`] for tests and tools.
    #[must_use]
    pub fn matches(&self, model: &GpuModel, streams: &ModelStreams) -> bool {
        self.check(model, streams)
    }

    /// [`Self::pose`] with fresh buffers (tests and tools): the posed
    /// streams of `model` this frame.
    #[must_use]
    pub fn posed(&self, model: &GpuModel, materials: &MaterialStore) -> Option<ModelStreams> {
        let colours = model.colour_stream(materials).ok()?;
        self.pose(
            model,
            &colours,
            &mut Scratch::default(),
            &mut AnimStats::default(),
        )
    }
}

/// The fit tolerance of a group (fine units): the classic fixed-point rounding
/// over a chain of label ops, plus a little per size.
fn tolerance(radius: f32) -> f32 {
    3.0 + 0.02 * radius
}

/// What one pose adds to [`AnimCache`]'s counters (poses run on the
/// renderer's worker threads, `frame::posing`; the counts are added in the
/// draw order).
#[derive(Clone, Copy, Debug, Default)]
pub struct PoseCount {
    /// `fit_groups`, `face_groups`, `drawn` and `faces`.
    pub stats: AnimStats,
    /// `CLIENT910_MODERN_CHECK`: checked, and not the classic faces.
    pub checked: usize,
    pub bad: usize,
    pub time: Duration,
}

/// Reusable per-pose buffers (one per posing thread).
#[derive(Default)]
pub struct PoseScratch(Scratch);

impl AnimMap {
    /// The posed streams of `model` with this map (`None`: its classic mesh)
    /// and what the pose counts. Reads only the map: the renderer poses
    /// its models on several threads.
    pub fn pose_counted(
        &self,
        model: &GpuModel,
        materials: &MaterialStore,
        scratch: &mut PoseScratch,
    ) -> (Option<ModelStreams>, PoseCount) {
        let mut count = PoseCount::default();
        let start = std::time::Instant::now();
        let out = model
            .colour_stream(materials)
            .ok()
            .and_then(|colours| self.pose(model, &colours, &mut scratch.0, &mut count.stats));
        count.time = start.elapsed();
        let Some(out) = out else {
            return (None, count);
        };
        count.stats.drawn = 1;
        count.stats.faces = self.triangles();
        if crate::modern_debug_flags::flags().check {
            count.checked = 1;
            count.bad = usize::from(!self.check(model, &out));
        }
        (Some(out), count)
    }
}

impl AnimMap {
    /// The posed streams of `model` (its positions and colours this frame).
    fn pose(
        &self,
        model: &GpuModel,
        colours: &[i32],
        scratch: &mut Scratch,
        stats: &mut AnimStats,
    ) -> Option<ModelStreams> {
        let n = self.src.len();
        scratch.pos.clear();
        scratch.pos.extend(self.src.iter().map(|&s| {
            let s = s as usize;
            Vec3::new(model.vx[s] as f32, model.vy[s] as f32, model.vz[s] as f32)
        }));
        let pos = &scratch.pos;
        // Per group: the linear part of its fit, when it fits.
        scratch.linear.clear();
        let mut need_faces = false;
        for g in &self.groups {
            let members = &self.group_members[g.members.start as usize..g.members.end as usize];
            let linear = g.inverse.and_then(|inverse| {
                let mut b = Mat3::ZERO;
                let mut cf = Vec3::ZERO;
                for &v in members {
                    let f = pos[v as usize];
                    let d = self.base[v as usize] - g.centre;
                    b += Mat3::from_cols(f * d.x, f * d.y, f * d.z);
                    cf += f;
                }
                cf /= members.len() as f32;
                let l = b * inverse;
                let t = cf - l * g.centre;
                let tol = tolerance(g.radius);
                members
                    .iter()
                    .all(|&v| {
                        (l * self.base[v as usize] + t - pos[v as usize])
                            .abs()
                            .max_element()
                            <= tol
                    })
                    .then_some(l)
                    .filter(|l| l.determinant().abs() > 1e-6)
            });
            if linear.is_some() {
                stats.fit_groups += 1;
            } else {
                stats.face_groups += 1;
                need_faces = true;
            }
            scratch.linear.push(linear);
        }
        // Per triangle: its base -> posed frame rotation (for the groups
        // without a fit).
        scratch.rot.clear();
        if need_faces {
            let flip = Mat3::from_diagonal(Vec3::new(1.0, 1.0, -1.0));
            for (t, tri) in self.indices.chunks_exact(3).enumerate() {
                let r = self.tri_base[t].and_then(|bt| {
                    let p = frame(
                        pos[usize::from(tri[0])],
                        pos[usize::from(tri[1])],
                        pos[usize::from(tri[2])],
                    )?;
                    // A mirrored model's posed frame is left-handed against
                    // its base: the reflection is `P diag(1, 1, -1) B^T`.
                    Some(if self.mirrored { p * flip * bt } else { p * bt })
                });
                scratch.rot.push(r);
            }
        }
        let mut out = ModelStreams {
            vertices: Vec::with_capacity(n),
            colours: Vec::with_capacity(n),
            indices: self.indices.clone(),
            batches: self.batches.clone(),
        };
        for (v, &p) in pos.iter().enumerate().take(n) {
            let transform = scratch.linear[self.group[v] as usize].unwrap_or_else(|| {
                let (a, b) = (
                    self.incident_start[v] as usize,
                    self.incident_start[v + 1] as usize,
                );
                let sum = self.incident[a..b]
                    .iter()
                    .filter_map(|&t| scratch.rot[t as usize])
                    .fold(Mat3::ZERO, |s, r| s + r);
                if sum == Mat3::ZERO {
                    Mat3::IDENTITY
                } else {
                    sum
                }
            });
            let normal = (transform * self.normal[v]).normalize_or_zero();
            let t = self.tangent[v];
            let tangent = (transform * Vec3::new(t[0], t[1], t[2])).normalize_or_zero();
            // A reflection turns the bitangent side.
            let side = if transform.determinant() < 0.0 {
                -t[3]
            } else {
                t[3]
            };
            out.vertices.push(Vertex {
                pos: p.to_array(),
                normal: normal.to_array(),
                uv: self.uv[v],
                tangent: [tangent.x, tangent.y, tangent.z, side],
            });
            let f = self.face[v] as usize;
            let colour = colours.get(us(model.idx1[f])).copied()?;
            out.colours.push(colour as u32);
        }
        Some(out)
    }

    /// `CLIENT910_MODERN_CHECK`: whether every triangle of `streams` is its
    /// classic face (the same three posed corner positions) and there is one
    /// per non-degenerate classic draw face.
    fn check(&self, model: &GpuModel, streams: &ModelStreams) -> bool {
        let key = |p: [f32; 3]| p.map(|c| c as i32);
        self.indices
            .chunks_exact(3)
            .zip(&self.tri_face)
            .all(|(tri, &f)| {
                let mut ours =
                    [tri[0], tri[1], tri[2]].map(|i| key(streams.vertices[usize::from(i)].pos));
                let mut classic =
                    corners(model, f as usize).map(|s| [model.vx[s], model.vy[s], model.vz[s]]);
                ours.sort_unstable();
                classic.sort_unstable();
                ours == classic
            })
    }
}

/// The raw classic model (times 4 below version 13, as the loc loader makes it)
/// and the RT7 copy of model group `id` (`crate::models::rt7::Rt7Cache`'s loaders).
pub type Sources<'s> =
    dyn FnMut(&crate::cache::Pack, u32) -> Option<(Arc<ModelUnlit>, Arc<Rt7Model>)> + 's;

impl AnimCache {
    /// The posed RT7 streams of animated `entity` (see the module docs),
    /// `None` to keep its classic mesh. `frame` is the renderer's frame for a
    /// per-frame (arena) draw, `None` for a cached dynamic loc; `sources`
    /// loads a part's raw and RT7 models. [`Self::map_for`], then
    /// [`AnimMap::pose_counted`] and [`Self::count`].
    pub fn animated(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        materials: &MaterialStore,
        entity: &EntityDraw<'_>,
        frame: Option<u64>,
        sources: &mut Sources<'_>,
    ) -> Option<ModelStreams> {
        let map = self.map_for(snapshot, entity, frame, sources)?;
        let mut scratch = std::mem::take(&mut self.scratch);
        let (out, count) = map.pose_counted(entity.model, materials, &mut scratch);
        self.scratch = scratch;
        self.count(&count);
        out
    }

    /// The map animated `entity` poses with (built on first use, here: the
    /// cache is the renderer's), `None` to keep its classic mesh.
    pub fn map_for(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        entity: &EntityDraw<'_>,
        frame: Option<u64>,
        sources: &mut Sources<'_>,
    ) -> Option<Arc<AnimMap>> {
        if let Some(frame) = frame {
            self.next_frame(frame);
        }
        if !matches!(entity.kind, Kind::Model | Kind::Body) {
            return None;
        }
        let pack = snapshot.pack?;
        let model = entity.model;
        let Some(ids) = model.source_ids.clone() else {
            self.stats.no_ids += 1;
            return None;
        };
        if ids.is_empty() || model.draw_face_count <= 0 {
            return None;
        }
        let key = fingerprint(model, &ids);
        if !self.maps.contains_key(&key) {
            let start = std::time::Instant::now();
            let built = self.build_anim_map(pack, model, &ids, sources);
            self.build_time += start.elapsed();
            if self.maps.len() >= MAX_MAPS {
                self.maps.clear();
            }
            self.maps.insert(key, built);
        }
        self.maps.get(&key).cloned().flatten()
    }

    /// Add one pose's counts ([`AnimMap::pose_counted`]).
    pub fn count(&mut self, count: &PoseCount) {
        self.pose_time += count.time;
        self.stats.fit_groups += count.stats.fit_groups;
        self.stats.face_groups += count.stats.face_groups;
        self.stats.drawn += count.stats.drawn;
        self.stats.faces += count.stats.faces;
        self.checked += count.checked;
        self.bad += count.bad;
    }

    fn build_anim_map(
        &mut self,
        pack: &crate::cache::Pack,
        model: &GpuModel,
        ids: &[u32],
        sources: &mut Sources<'_>,
    ) -> Option<Arc<AnimMap>> {
        let mut parts = Vec::with_capacity(ids.len());
        let mut raws = Vec::with_capacity(ids.len());
        for &id in ids {
            let Some((raw, rt7)) = sources(pack, id) else {
                self.stats.no_rt7 += 1;
                return None;
            };
            parts.push(rt7);
            raws.push(raw);
        }
        match build_map(model, &raws, &parts) {
            Ok(map) => {
                self.stats.maps += 1;
                self.stats.label_mismatches += map.label_mismatches;
                Some(Arc::new(map))
            }
            Err(why) => {
                match why {
                    Reject::OtherModel => self.stats.other_model += 1,
                    Reject::Unmatched => self.stats.unmatched += 1,
                    Reject::TooLarge => self.stats.too_large += 1,
                }
                None
            }
        }
    }
}

impl AnimCache {
    /// [`Self::next_frame`] for the cached (dynamic loc) path.
    pub fn begin_frame(&mut self, frame: u64) {
        self.next_frame(frame);
    }

    /// Close the check and log of the previous frame when `frame` begins.
    fn next_frame(&mut self, frame: u64) {
        if frame == self.frame {
            return;
        }
        if crate::modern_debug_flags::flags().check && self.frame != 0 {
            if self.bad > 0 {
                self.mismatch_frames += 1;
                log::warn!(
                    "[modern] rt7 anim check: {} of {} posed RT7 entities differ from their classic faces ({} mismatching frames)",
                    self.bad,
                    self.checked,
                    self.mismatch_frames
                );
            } else if self.frame == 2 || self.frame.is_multiple_of(600) {
                log::info!(
                    "[modern] rt7 anim check: {} posed RT7 entities draw their classic faces",
                    self.checked
                );
            }
        }
        if matches!(self.frame, 2 | 5 | 20) || (self.frame > 0 && self.frame.is_multiple_of(600)) {
            log::info!(
                "[modern] rt7 animated models: {:?}, {} maps cached, built in {:.1} ms, posed in {:.1} ms",
                self.stats,
                self.maps.len(),
                self.build_time.as_secs_f64() * 1000.0,
                self.pose_time.as_secs_f64() * 1000.0
            );
        }
        self.last_check = (self.checked, self.bad);
        self.checked = 0;
        self.bad = 0;
        self.frame = frame;
    }

    /// Close the current frame's check now (tests).
    pub fn finish_frame(&mut self) -> (usize, usize) {
        let frame = self.frame + 1;
        self.next_frame(frame);
        self.last_check
    }
}

#[cfg(test)]
pub(crate) mod tests;
