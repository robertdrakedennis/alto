//! [`GpuModel::new`]: the lit model built from a decoded [`ModelUnlit`].
//!
//! The build runs in stages: filter and order the faces, build the billboard
//! table, generate the face and vertex normals, deduplicate the vertices per
//! (colour, normal, texture coordinate), group the drawn faces into material
//! batches, and gather the label groups and particle attachments.

use super::sort::quicksort_parallel;
use super::texture_mapping::{face_texture, texture_frames, FaceTexture};
use super::{find_material, GpuModel, ParticleEffectorRef, ParticleEmitterRef};
use crate::billboard::BillboardStore;
use crate::colour::hsl_tables;
use crate::modelunlit::ModelUnlit;
use crate::particle::EmitterStore;
use crate::texture::{AlphaMode, Material, MaterialStore};

/// The stores the build reads: material flags, billboard types and emitter
/// types.
#[derive(Clone, Copy)]
pub struct ModelStores<'a> {
    pub materials: &'a MaterialStore,
    pub billboards: &'a BillboardStore,
    pub emitters: &'a EmitterStore,
}

/// How a model is built: the render flags, the lighting and the detail flags.
#[derive(Clone, Copy, Debug)]
pub struct BuildParams {
    pub flags: i32,
    pub ambient: i32,
    pub contrast: i32,
    pub detail: i32,
}

/// The kept faces in draw order, and what the ordering found out.
struct FaceOrder {
    /// Source face index of each kept face, in draw order.
    faces: Vec<i32>,
    /// Kept faces minus the billboard and emitter faces, which draw nothing.
    drawn: usize,
    /// How many kept-face corners each source vertex has; the vertex table
    /// turns it into slot offsets.
    corner_counts: Vec<i32>,
    has_transparency: bool,
    has_animated_uvs: bool,
}

/// Drops the faces the model never draws (type 2 faces and the faces of low
/// detail materials), orders the rest by material, translucency and priority,
/// and marks the faces a billboard or emitter replaces.
fn order_faces(
    stores: &ModelStores<'_>,
    source: &ModelUnlit,
    params: BuildParams,
) -> anyhow::Result<FaceOrder> {
    let BuildParams { flags, detail, .. } = params;
    let face_total = source.face_count as usize;
    let mut faces = vec![0_i32; face_total];
    let mut corner_counts = vec![0_i32; source.used_vertex_count as usize + 1];
    let mut kept = 0_usize;
    for face in 0..face_total {
        if source.face_type.as_ref().is_none_or(|t| t[face] != 2) {
            if let Some(materials) = &source.face_material {
                if materials[face] != -1 {
                    let material = find_material(stores.materials, materials[face])?;
                    if ((detail & 0x40) == 0 || !material.high_detail) && material.low_detail {
                        continue;
                    }
                }
            }
            faces[kept] = face as i32;
            kept += 1;
            corner_counts[source.face_vertex1[face] as usize] += 1;
            corner_counts[source.face_vertex2[face] as usize] += 1;
            corner_counts[source.face_vertex3[face] as usize] += 1;
        }
    }
    let mut drawn = kept;
    let mut sort_keys = vec![0_i64; kept];
    let sort_by_priority = (flags & 0x100) != 0;
    let mut has_transparency = false;
    let mut has_animated_uvs = false;
    for slot in 0..kept {
        let face = faces[slot] as usize;
        let mut material: Option<&Material> = None;
        let mut priority_bits: i32 = 0;
        let mut effect: i8 = 0;
        let mut effect_param: i8 = 0;
        let mut material_id: i16 = -1;
        if let Some(model_billboards) = &source.billboard {
            let mut replaced = false;
            for billboard in model_billboards {
                if billboard.face == face as i32 {
                    let kind = stores.billboards.get(billboard.billboard_type);
                    if kind.remove_face {
                        replaced = true;
                    }
                    if kind.material != -1 {
                        let material = find_material(stores.materials, kind.material as i16)?;
                        if material.alpha == AlphaMode::Multiply {
                            has_transparency = true;
                        }
                    }
                }
            }
            if replaced {
                sort_keys[slot] = i64::MAX;
                drawn -= 1;
                continue;
            }
        }
        if let Some(emitters) = &source.emitters {
            let mut replaced = false;
            for emitter in emitters {
                if emitter.face == face as i32 && stores.emitters.get(emitter.particle).remove_face
                {
                    replaced = true;
                }
            }
            if replaced {
                sort_keys[slot] = i64::MAX;
                drawn -= 1;
                continue;
            }
        }
        if let Some(face_materials) = &source.face_material {
            material_id = face_materials[face];
            if material_id != -1 {
                let found = find_material(stores.materials, material_id)?;
                if (detail & 0x40) != 0 && found.high_detail {
                    material_id = -1;
                } else {
                    material = Some(found);
                    effect = found.effect as i8;
                    effect_param = found.effect_param as i8;
                }
            }
        }
        let translucent = source.face_trans.as_ref().is_some_and(|t| t[face] != 0)
            || material.is_some_and(|m| m.alpha != AlphaMode::None);
        if let Some(priority) = source
            .face_priority
            .as_ref()
            .filter(|_| sort_by_priority || translucent)
        {
            priority_bits = priority_bits.wrapping_add(i32::from(priority[face]) << 17);
        }
        if translucent {
            priority_bits = priority_bits.wrapping_add(65536);
        }
        let with_effect = ((i32::from(effect) & 0xFF) << 8).wrapping_add(priority_bits);
        let high = (i32::from(effect_param) & 0xFF).wrapping_add(with_effect);
        let with_material = (i32::from(material_id) & 0xFFFF) << 16;
        let low = (slot as i32 & 0xFFFF).wrapping_add(with_material);
        sort_keys[slot] = (i64::from(high) << 32).wrapping_add(i64::from(low));
        has_transparency |= translucent;
        has_animated_uvs |= material.is_some_and(|m| m.speed_u != 0.0 || m.speed_v != 0.0);
    }
    quicksort_parallel(&mut sort_keys[..kept], &mut faces[..kept]);
    faces.truncate(kept);
    Ok(FaceOrder {
        faces,
        drawn,
        corner_counts,
        has_transparency,
        has_animated_uvs,
    })
}

/// The billboard table of the kept faces. Every billboard's face must have
/// survived the face filter.
fn billboard_table(
    stores: &ModelStores<'_>,
    source: &ModelUnlit,
    sorted_faces: &[i32],
    flags: i32,
) -> anyhow::Result<Option<crate::billboard::ModelBillboards>> {
    let Some(model_billboards) = &source.billboard else {
        return Ok(None);
    };
    let rgb = &hsl_tables().rgb;
    let mut faces = Vec::with_capacity(model_billboards.len());
    let mut states = Vec::with_capacity(model_billboards.len());
    for billboard in model_billboards {
        let kind = stores.billboards.get(billboard.billboard_type);
        let face = billboard.face as usize;
        let slot = sorted_faces
            .iter()
            .position(|&f| f == billboard.face)
            .ok_or_else(|| anyhow::anyhow!("billboard face {} not kept", billboard.face))?;
        let colour = rgb[(source.face_colour[face] as u16) as usize] & 0xFF_FFFF;
        let translucency = source.face_trans.as_ref().map_or(0, |t| i32::from(t[face]));
        let argb = colour | (255 - translucency).wrapping_shl(24);
        faces.push(crate::billboard::BillboardFace {
            face: slot,
            source_face: billboard.face,
            vertices: [
                source.face_vertex1[face] as usize,
                source.face_vertex2[face] as usize,
                source.face_vertex3[face] as usize,
            ],
            width: i32::from(kind.width as i16),
            height: i32::from(kind.height as i16),
            material: i32::from(kind.material as i16),
            bloom_hidden: kind.hidden_under_bloom,
            depth_offset: billboard.depth_offset,
            sprite_mode: kind.sprite_mode,
            sprite_blend: kind.sprite_blend,
            remove_face: kind.remove_face,
        });
        // The software palette entry follows the face colour.
        let mut state = crate::billboard::BillboardState::new(argb);
        state.hsl = source.face_colour[face] as u16;
        states.push(state);
    }
    let groups = (flags & 0x400 != 0)
        .then(|| crate::billboard::label_groups(model_billboards.iter().map(|b| b.label)));
    Ok(Some(crate::billboard::ModelBillboards {
        faces,
        states,
        groups,
    }))
}

/// The normals of the source faces: smooth faces add theirs to their three
/// vertices, flat faces keep it.
struct FaceNormals {
    /// Per source vertex: summed `(x, y, z)` and the number of faces.
    smooth: Vec<[i32; 4]>,
    /// Per source face: the normal of a flat face.
    flat: Vec<Option<[i32; 3]>>,
}

fn face_normals(source: &ModelUnlit) -> FaceNormals {
    let (vx, vy, vz) = (&source.vertex_x, &source.vertex_y, &source.vertex_z);
    let mut smooth = vec![[0_i32; 4]; source.used_vertex_count as usize];
    let mut flat: Vec<Option<[i32; 3]>> = vec![None; source.face_count as usize];
    for face in 0..source.face_count as usize {
        let a = source.face_vertex1[face] as usize;
        let b = source.face_vertex2[face] as usize;
        let c = source.face_vertex3[face] as usize;
        let ab = [
            vx[b].wrapping_sub(vx[a]),
            vy[b].wrapping_sub(vy[a]),
            vz[b].wrapping_sub(vz[a]),
        ];
        let ac = [
            vx[c].wrapping_sub(vx[a]),
            vy[c].wrapping_sub(vy[a]),
            vz[c].wrapping_sub(vz[a]),
        ];
        let mut x = ab[1]
            .wrapping_mul(ac[2])
            .wrapping_sub(ab[2].wrapping_mul(ac[1]));
        let mut y = ab[2]
            .wrapping_mul(ac[0])
            .wrapping_sub(ab[0].wrapping_mul(ac[2]));
        let mut z = ab[0]
            .wrapping_mul(ac[1])
            .wrapping_sub(ab[1].wrapping_mul(ac[0]));
        while x > 8192 || y > 8192 || z > 8192 || x < -8192 || y < -8192 || z < -8192 {
            x >>= 1;
            y >>= 1;
            z >>= 1;
        }
        let mut length = f64::from(z * z + x * x + y * y).sqrt() as i32;
        if length <= 0 {
            length = 1;
        }
        let unit = [x * 256 / length, y * 256 / length, z * 256 / length];
        match source.face_type.as_ref().map_or(0, |t| t[face]) {
            0 => {
                for corner in [a, b, c] {
                    smooth[corner][0] += unit[0];
                    smooth[corner][1] += unit[1];
                    smooth[corner][2] += unit[2];
                    smooth[corner][3] += 1;
                }
            }
            1 => flat[face] = Some(unit),
            _ => {}
        }
    }
    FaceNormals { smooth, flat }
}

/// The unique vertices of the model, deduplicated per source vertex by key.
struct VertexTable {
    /// Per source vertex, its first slot in `slots`; one more entry marks the
    /// end.
    slot_start: Vec<i32>,
    /// Unique vertex index + 1 per slot, 0 = free.
    slots: Vec<i16>,
    /// The key each occupied slot was filed under.
    slot_keys: Vec<i64>,
    count: i32,
    face: Vec<i16>,
    source_vertex: Vec<i16>,
    nx: Vec<i16>,
    ny: Vec<i16>,
    nz: Vec<i16>,
    ncount: Vec<i8>,
    u: Vec<f32>,
    v: Vec<f32>,
}

impl VertexTable {
    /// `corner_counts` are the per-vertex corner counts of [`FaceOrder`];
    /// `capacity` is the corner count of all kept faces.
    fn new(mut corner_counts: Vec<i32>, used: usize, capacity: usize) -> Self {
        let mut offset = 0;
        for slot in &mut corner_counts[..used] {
            let count = *slot;
            *slot = offset;
            offset += count;
        }
        corner_counts[used] = offset;
        Self {
            slot_start: corner_counts,
            slots: vec![0; capacity],
            slot_keys: vec![0; capacity],
            count: 0,
            face: vec![0; capacity],
            source_vertex: vec![0; capacity],
            nx: vec![0; capacity],
            ny: vec![0; capacity],
            nz: vec![0; capacity],
            ncount: vec![0; capacity],
            u: vec![0.0; capacity],
            v: vec![0.0; capacity],
        }
    }

    /// The unique vertex of `source_vertex` filed under `key`, added with
    /// this face, normal `[x, y, z, count]` and texture coordinate when it is
    /// new.
    fn intern(
        &mut self,
        source_vertex: usize,
        face_slot: usize,
        key: i64,
        normal: [i32; 4],
        uv: [f32; 2],
    ) -> i16 {
        let first = self.slot_start[source_vertex] as usize;
        let end = self.slot_start[source_vertex + 1] as usize;
        let mut free = 0_usize;
        for slot in first..end {
            if self.slots[slot] == 0 {
                free = slot;
                break;
            }
            let existing = (i32::from(self.slots[slot]) & 0xFFFF) - 1;
            if self.slot_keys[slot] == key {
                return existing as i16;
            }
        }
        self.slots[free] = (self.count + 1) as i16;
        self.slot_keys[free] = key;
        let index = self.count as usize;
        self.face[index] = face_slot as i16;
        self.source_vertex[index] = source_vertex as i16;
        self.nx[index] = normal[0] as i16;
        self.ny[index] = normal[1] as i16;
        self.nz[index] = normal[2] as i16;
        self.ncount[index] = normal[3] as i8;
        self.u[index] = uv[0];
        self.v[index] = uv[1];
        self.count += 1;
        index as i16
    }

    fn finish(&mut self) {
        let n = self.count as usize;
        self.source_vertex.truncate(n);
        self.face.truncate(n);
        self.nx.truncate(n);
        self.ny.truncate(n);
        self.nz.truncate(n);
        self.ncount.truncate(n);
        self.u.truncate(n);
        self.v.truncate(n);
    }
}

/// The runs of equal material over the drawn faces: where each starts, the
/// lowest unique vertex it uses and how many vertices it spans.
struct Batches {
    face_start: Vec<i32>,
    min_vertex: Vec<i32>,
    vertex_span: Vec<i32>,
}

fn material_batches(
    face_material: &[i16],
    drawn: usize,
    corners: [&[i16]; 3],
    unique_count: i32,
) -> Batches {
    let mut batches = Batches {
        face_start: Vec::new(),
        min_vertex: Vec::new(),
        vertex_span: Vec::new(),
    };
    if drawn == 0 {
        return batches;
    }
    let mut runs = 1;
    let mut current = face_material[0];
    for &material in face_material.iter().take(drawn) {
        if current != material {
            runs += 1;
            current = material;
        }
    }
    batches.min_vertex = vec![0; runs];
    batches.vertex_span = vec![0; runs];
    batches.face_start = vec![0; runs + 1];
    let mut lowest = unique_count;
    let mut highest = 0;
    let mut run = 0_usize;
    let mut current = face_material[0];
    for face in 0..drawn {
        let material = face_material[face];
        if current != material {
            batches.min_vertex[run] = lowest;
            batches.vertex_span[run] = highest - lowest + 1;
            run += 1;
            batches.face_start[run] = face as i32;
            lowest = unique_count;
            highest = 0;
            current = material;
        }
        for corner in corners {
            let vertex = i32::from(corner[face]) & 0xFFFF;
            if vertex < lowest {
                lowest = vertex;
            }
            if vertex > highest {
                highest = vertex;
            }
        }
    }
    batches.min_vertex[run] = lowest;
    batches.vertex_span[run] = highest - lowest + 1;
    run += 1;
    batches.face_start[run] = drawn as i32;
    batches
}

/// Indices per skin label: `groups[label]` lists the items with that label.
fn label_groups(labels: Vec<i32>) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); labels.iter().copied().max().unwrap_or(0).max(0) as usize + 1];
    for (index, label) in labels.into_iter().enumerate() {
        if label >= 0 {
            out[label as usize].push(index);
        }
    }
    out
}

impl GpuModel {
    /// Builds the lit model of `source`.
    pub fn new(
        stores: &ModelStores<'_>,
        source: &ModelUnlit,
        params: BuildParams,
    ) -> anyhow::Result<Self> {
        let BuildParams {
            flags,
            ambient,
            contrast,
            detail,
        } = params;
        let used = source.used_vertex_count as usize;
        let order = order_faces(stores, source, params)?;
        let kept = order.faces.len();
        let billboards = billboard_table(stores, source, &order.faces, flags)?;
        let normals = face_normals(source);
        let frames = texture_frames(source, &order.faces)?;

        let mut vertices = VertexTable::new(order.corner_counts, used, kept * 3);
        let mut face_alpha = vec![0_i8; kept];
        let mut face_colour = vec![0_i16; kept];
        let mut face_material = vec![0_i16; kept];
        let mut idx1 = vec![0_i16; kept];
        let mut idx2 = vec![0_i16; kept];
        let mut idx3 = vec![0_i16; kept];
        let mut face_part = source
            .face_source_models
            .as_ref()
            .map(|_| vec![0_i16; kept]);
        for slot in 0..kept {
            let face = order.faces[slot] as usize;
            let colour = i32::from(source.face_colour[face]) & 0xFFFF;
            let alpha = source
                .face_trans
                .as_ref()
                .map_or(0, |t| i32::from(t[face]) & 0xFF);
            let mut material_id: i16 = source.face_material.as_ref().map_or(-1, |m| m[face]);
            if material_id != -1
                && (detail & 0x40) != 0
                && find_material(stores.materials, material_id)?.high_detail
            {
                material_id = -1;
            }
            let texture = if material_id == -1 {
                FaceTexture {
                    uv: [[0.0; 2]; 3],
                    keys: [0; 3],
                }
            } else {
                face_texture(source, &frames, face)?
            };
            let corners = [
                source.face_vertex1[face] as usize,
                source.face_vertex2[face] as usize,
                source.face_vertex3[face] as usize,
            ];
            let mut indices = [0_i16; 3];
            match source.face_type.as_ref().map_or(0, |t| t[face]) {
                0 => {
                    let base = i64::from((colour << 8) + alpha);
                    for corner in 0..3 {
                        indices[corner] = vertices.intern(
                            corners[corner],
                            slot,
                            base | (texture.keys[corner] << 24),
                            normals.smooth[corners[corner]],
                            texture.uv[corner],
                        );
                    }
                }
                1 => {
                    let flat = normals.flat[face].expect("flat face normal");
                    let key = (i64::from(flat[2] + 256) << 24)
                        .wrapping_add(i64::from(flat[0] & i32::MIN) << 9)
                        .wrapping_add(i64::from(flat[1] + 256) << 32)
                        .wrapping_add(i64::from(colour << 8))
                        .wrapping_add(i64::from(alpha));
                    let key = key | (texture.keys[0] << 41);
                    for corner in 0..3 {
                        indices[corner] = vertices.intern(
                            corners[corner],
                            slot,
                            key,
                            [flat[0], flat[1], flat[2], 0],
                            texture.uv[corner],
                        );
                    }
                }
                _ => {}
            }
            [idx1[slot], idx2[slot], idx3[slot]] = indices;
            if let Some(trans) = &source.face_trans {
                face_alpha[slot] = trans[face];
            }
            if let (Some(part), Some(source_part)) =
                (face_part.as_mut(), source.face_source_models.as_ref())
            {
                part[slot] = source_part[face];
            }
            face_colour[slot] = source.face_colour[face];
            face_material[slot] = material_id;
        }
        let batches = material_batches(
            &face_material,
            order.drawn,
            [&idx1, &idx2, &idx3],
            vertices.count,
        );
        vertices.finish();

        let vertex_groups = source
            .vertex_label
            .as_ref()
            .filter(|_| flags & 0x20 != 0)
            .map(|labels| label_groups(labels[..used].to_vec()));
        let face_groups = source
            .face_label
            .as_ref()
            .filter(|_| flags & 0x180 != 0)
            .map(|labels| {
                label_groups(
                    order
                        .faces
                        .iter()
                        .map(|&face| labels[face as usize])
                        .collect(),
                )
            });
        Ok(Self {
            flags,
            detail,
            ambient: ambient as i16,
            contrast: contrast as i16,
            vertex_count_all: source.vertex_count,
            vertex_count: source.used_vertex_count,
            vx: source.vertex_x.clone(),
            vy: source.vertex_y.clone(),
            vz: source.vertex_z.clone(),
            vertex_source_models: source.vertex_source_models.clone(),
            vertex_groups,
            face_groups,
            unique_count: vertices.count,
            unique_vertex: vertices.source_vertex,
            unique_face: vertices.face,
            nx: vertices.nx,
            ny: vertices.ny,
            nz: vertices.nz,
            ncount: vertices.ncount,
            u: vertices.u,
            v: vertices.v,
            merged: None,
            face_count: kept as i32,
            draw_face_count: order.drawn as i32,
            face_colour,
            face_alpha,
            face_material,
            face_part,
            idx1,
            idx2,
            idx3,
            vertex_offsets: vertices.slot_start,
            vertex_slots: vertices.slots,
            batch_face_start: batches.face_start,
            batch_min_vertex: batches.min_vertex,
            batch_vertex_span: batches.vertex_span,
            has_transparency: order.has_transparency,
            has_animated_uvs: order.has_animated_uvs,
            billboards,
            face_source: order.faces,
            source_face_count: source.face_count,
            source_face_priority: source.face_priority.clone(),
            has_particles: source.emitters.is_some() || source.effectors.is_some(),
            particle_emitters: source
                .emitters
                .as_ref()
                .map(|emitters| {
                    emitters
                        .iter()
                        .filter_map(|emitter| {
                            let vertices = [emitter.v1, emitter.v2, emitter.v3]
                                .into_iter()
                                .map(|vertex| usize::try_from(vertex).ok())
                                .collect::<Option<Vec<_>>>()?;
                            Some(ParticleEmitterRef {
                                particle: emitter.particle,
                                vertices: [vertices[0], vertices[1], vertices[2]],
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            particle_effectors: source
                .effectors
                .as_ref()
                .map(|effectors| {
                    effectors
                        .iter()
                        .filter_map(|effector| {
                            Some(ParticleEffectorRef {
                                effector: effector.effector,
                                vertex: usize::try_from(effector.vertex).ok()?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            source_ids: source.source_ids.clone(),
            bounds_valid: false,
            min_x: 0,
            max_x: 0,
            min_y: 0,
            max_y: 0,
            min_z: 0,
            max_z: 0,
            horizontal_radius: 0,
            radius: 0,
            height_valid: false,
            height: 0,
        })
    }
}
