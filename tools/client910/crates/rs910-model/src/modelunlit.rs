//! The model as it is stored in the cache, before lighting: the decoder,
//! the merge of several parts into one model, and the edits (translate,
//! rotate, recolour, rematerial, scale) applied before a lit model is built.
//!
//! This is a second decoder next to `model.rs` on purpose: the lit-model build
//! branches on which streams are *absent* (no face types, no priorities, no
//! translucency, no materials, no mappings, no vertex labels) and on the
//! particle and billboard tables, none of which `model::RawModel` preserves.
//! Optional streams are `Option`s.
//!
//! Face and vertex indices are stored as `i16` and used sign-extended; real
//! data never exceeds 32767 vertices.

use crate::cache::Pack;
use rs910_core::reader::{Eof, Reader as CoreReader};

/// The cache archive of the models (group = model id, file 0).
pub const MODEL_ARCHIVE: &str = "models";

/// A particle emitter attached to a face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelParticleEmitter {
    /// Emitter type id.
    pub particle: i32,
    pub face: i32,
    /// The face's three vertices.
    pub v1: i32,
    pub v2: i32,
    pub v3: i32,
    pub priority: i8,
}

/// A particle effector attached to a vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelParticleEffector {
    /// Effector type id.
    pub effector: i32,
    pub vertex: i32,
}

/// A billboard attached to a face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelBillboard {
    pub billboard_type: i32,
    pub face: i32,
    /// Skin label (`-1` none).
    pub label: i32,
    /// How far towards the eye the billboard is pulled.
    pub depth_offset: i32,
}

/// A decoded model. Optional fields are absent streams.
#[derive(Clone, Debug, Default)]
pub struct ModelUnlit {
    /// Format version (12 by default).
    pub version: i32,
    pub vertex_count: i32,
    /// Highest face vertex index + 1: the vertices faces use.
    pub used_vertex_count: i32,
    pub vertex_x: Vec<i32>,
    pub vertex_y: Vec<i32>,
    pub vertex_z: Vec<i32>,
    /// Per vertex: offset of its texture coordinates in the coordinate
    /// streams (a running sum of the per-vertex counts).
    pub vertex_texture_vertex: Option<Vec<i32>>,
    /// Per vertex skin label.
    pub vertex_label: Option<Vec<i32>>,
    /// Merged models only: bit `1 << part` per vertex.
    pub vertex_source_models: Option<Vec<i16>>,
    pub face_count: i32,
    /// Textured vertex count: the length of the texture coordinate streams.
    pub textured_vertex_count: i32,
    pub texture_vertex_u: Option<Vec<f32>>,
    pub texture_vertex_v: Option<Vec<f32>>,
    pub face_vertex1: Vec<i16>,
    pub face_vertex2: Vec<i16>,
    pub face_vertex3: Vec<i16>,
    /// Per face: offsets from each corner's first texture coordinate.
    pub face_texture_vertex_offset1: Option<Vec<i8>>,
    pub face_texture_vertex_offset2: Option<Vec<i8>>,
    pub face_texture_vertex_offset3: Option<Vec<i8>>,
    /// Per face render type (0 smooth, 1 flat, 2 not drawn).
    pub face_type: Option<Vec<i8>>,
    pub face_priority: Option<Vec<i8>>,
    /// Per face translucency.
    pub face_trans: Option<Vec<i8>>,
    /// Per face texture triangle (or the direct and default sentinels).
    pub face_mapping: Option<Vec<i16>>,
    pub face_colour: Vec<i16>,
    pub face_material: Option<Vec<i16>>,
    /// Per face skin label.
    pub face_label: Option<Vec<i32>>,
    /// The single priority when there are no per-face priorities.
    pub default_priority: i8,
    /// Merged models only: the bit of the source part per face.
    pub face_source_models: Option<Vec<i16>>,
    /// Texture triangle count.
    pub texture_triangle_count: i32,
    pub texture_triangle_type: Option<Vec<i8>>,
    pub texture_triangle_vertex1: Option<Vec<i16>>,
    pub texture_triangle_vertex2: Option<Vec<i16>>,
    pub texture_triangle_vertex3: Option<Vec<i16>>,
    pub texture_triangle_scale_x: Option<Vec<i32>>,
    pub texture_triangle_scale_y: Option<Vec<i32>>,
    pub texture_triangle_scale_z: Option<Vec<i32>>,
    pub texture_triangle_speed: Option<Vec<i32>>,
    pub texture_triangle_translation_u: Option<Vec<i32>>,
    pub texture_triangle_translation_v: Option<Vec<i32>>,
    /// Per texture triangle: the spin about Y, in 1/256 turns.
    pub texture_triangle_rotation: Option<Vec<i8>>,
    pub texture_triangle_direction: Option<Vec<i8>>,
    pub emitters: Option<Vec<ModelParticleEmitter>>,
    pub effectors: Option<Vec<ModelParticleEffector>>,
    pub billboard: Option<Vec<ModelBillboard>>,
    /// Provenance: the archive model groups whose faces this model holds, in
    /// face order (one per [`Self::load`], the parts' lists concatenated by
    /// [`Self::merge_slots`]); `None` for a decoded or synthetic model. Read
    /// only by the NXT renderer's RT7 posing.
    pub source_ids: Option<std::sync::Arc<[u32]>>,
}

/// The reads the decoder uses.
struct Packet<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Packet<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// One `rs910_core::reader` read at `pos`. `pos` follows the read; a
    /// failure names the offset of the missing byte.
    fn read<T>(
        &mut self,
        read: impl FnOnce(&mut CoreReader<'a>) -> Result<T, Eof>,
    ) -> anyhow::Result<T> {
        let mut r = CoreReader::at(self.data, self.pos);
        let out = read(&mut r);
        self.pos = r.pos();
        out.map_err(truncated)
    }

    fn g1(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g1).map(i32::from)
    }

    fn g1b(&mut self) -> anyhow::Result<i8> {
        self.read(CoreReader::g1b)
    }

    fn g2(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g2).map(i32::from)
    }

    fn g2s(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g2s).map(i32::from)
    }

    fn g3(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::g3).map(|v| v as i32)
    }

    /// A signed one-or-two byte value.
    fn gsmart1or2s(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::gsmart1or2s)
    }

    /// An unsigned one-or-two byte value.
    fn gsmart1or2(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::gsmart1or2)
    }

    /// An unsigned one-or-two byte value where the one-byte 255 is `-1`.
    fn gsmart1or2null(&mut self) -> anyhow::Result<i32> {
        self.read(CoreReader::gsmart1or2null)
    }
}

/// The decoder's truncation error (the offset of the missing byte).
fn truncated(e: Eof) -> anyhow::Error {
    anyhow::anyhow!("model data truncated at {}", e.pos)
}

impl ModelUnlit {
    /// Loads the model stored under `model_id` in the pack.
    pub fn load(pack: &Pack, model_id: u32) -> anyhow::Result<Self> {
        let files = pack.read_group(MODEL_ARCHIVE, model_id)?;
        let bytes = files
            .get(&0)
            .ok_or_else(|| anyhow::anyhow!("model {model_id}: no file 0"))?;
        let mut m = Self::decode(bytes).map_err(|e| anyhow::anyhow!("model {model_id}: {e:#}"))?;
        m.source_ids = Some(std::sync::Arc::from([model_id]));
        Ok(m)
    }

    /// Decodes a model. The file is a run of streams laid out back to back:
    /// a 3-byte header, the texture triangle types, the per-vertex flags, the
    /// optional face types, the face opcodes, priorities, labels, vertex
    /// labels, translucency, face index deltas, materials, mappings, colours,
    /// the three coordinate delta streams, the texture triangle data, and the
    /// particle and billboard attachments. A 26-byte footer holds the counts,
    /// the stream flags and the lengths that place each stream, and with
    /// texture coordinates a second footer points to their streams.
    #[allow(clippy::too_many_lines)]
    pub fn decode(data: &[u8]) -> anyhow::Result<Self> {
        let mut m = Self {
            version: 12,
            ..Self::default()
        };
        let mut p2 = Packet::new(data);
        let mut p3 = Packet::new(data);
        let mut p4 = Packet::new(data);
        let mut p5 = Packet::new(data);
        let mut p6 = Packet::new(data);
        let mut p7 = Packet::new(data);
        let mut p8 = Packet::new(data);
        let tag = p2.g1()?;
        if tag != 1 {
            // 485 prints and leaves an empty model.
            anyhow::bail!("unsupported model tag {tag}");
        }
        p2.g1()?;
        m.version = p2.g1()?;
        if data.len() < 26 {
            anyhow::bail!("model shorter than its footer");
        }
        p2.pos = data.len() - 26;
        m.vertex_count = p2.g2()?;
        m.face_count = p2.g2()?;
        m.texture_triangle_count = p2.g2()?;
        let flags = p2.g1()?;
        let has_face_types = (flags & 0x1) == 1;
        let has_particles = (flags & 0x2) == 2;
        let has_billboards = (flags & 0x4) == 4;
        let wide_vertex_labels = (flags & 0x10) == 16;
        let wide_face_labels = (flags & 0x20) == 32;
        let wide_billboard_labels = (flags & 0x40) == 64;
        let has_texture_coordinates = (flags & 0x80) == 128;
        let priority_mode = p2.g1()?;
        let trans_mode = p2.g1()?;
        let face_label_mode = p2.g1()?;
        let material_mode = p2.g1()?;
        let vertex_label_mode = p2.g1()?;
        let x_length = p2.g2()?;
        let y_length = p2.g2()?;
        let z_length = p2.g2()?;
        let face_index_length = p2.g2()?;
        let face_mapping_length = p2.g2()?;
        let mut vertex_label_length = p2.g2()?;
        let mut face_label_length = p2.g2()?;
        if !wide_vertex_labels {
            vertex_label_length = if vertex_label_mode == 1 {
                m.vertex_count
            } else {
                0
            };
        }
        if !wide_face_labels {
            face_label_length = if face_label_mode == 1 {
                m.face_count
            } else {
                0
            };
        }
        let mut plain_triangle_count = 0;
        let mut mapped_triangle_count = 0;
        let mut cube_triangle_count = 0;
        if m.texture_triangle_count > 0 {
            let mut types = Vec::with_capacity(m.texture_triangle_count as usize);
            p2.pos = 3;
            for _ in 0..m.texture_triangle_count {
                let kind = p2.g1b()?;
                types.push(kind);
                if kind == 0 {
                    plain_triangle_count += 1;
                }
                if (1..=3).contains(&kind) {
                    mapped_triangle_count += 1;
                }
                if kind == 2 {
                    cube_triangle_count += 1;
                }
            }
            m.texture_triangle_type = Some(types);
        }
        let vertex_flags_start = m.texture_triangle_count + 3;
        let mut face_opcodes_start = m.vertex_count + vertex_flags_start;
        let face_types_start = face_opcodes_start;
        if has_face_types {
            face_opcodes_start += m.face_count;
        }
        let mut face_labels_start = m.face_count + face_opcodes_start;
        let priorities_start = face_labels_start;
        if priority_mode == 255 {
            face_labels_start += m.face_count;
        }
        let vertex_labels_start = face_label_length + face_labels_start;
        let mut face_index_start = vertex_label_length + vertex_labels_start;
        let face_trans_start = face_index_start;
        if trans_mode == 1 {
            face_index_start += m.face_count;
        }
        let mut face_mapping_start = face_index_length + face_index_start;
        let face_material_start = face_mapping_start;
        if material_mode == 1 {
            face_mapping_start += m.face_count * 2;
        }
        let face_colour_start = face_mapping_length + face_mapping_start;
        let vertex_x_start = m.face_count * 2 + face_colour_start;
        let vertex_y_start = x_length + vertex_x_start;
        let vertex_z_start = y_length + vertex_y_start;
        let plain_triangle_start = z_length + vertex_z_start;
        let mapped_triangle_start = plain_triangle_count * 6 + plain_triangle_start;
        let scale_start = mapped_triangle_count * 6 + mapped_triangle_start;
        let scale_size = if m.version == 14 {
            7
        } else if m.version >= 15 {
            9
        } else {
            6
        };
        let rotation_start = mapped_triangle_count * scale_size + scale_start;
        let direction_start = mapped_triangle_count + rotation_start;
        let speed_start = mapped_triangle_count + direction_start;
        let attachments_start = cube_triangle_count * 2 + mapped_triangle_count + speed_start;
        let mut face_offsets_start = data.len() as i32;
        let mut vertex_texture_counts_start = data.len() as i32;
        let mut u_start = data.len() as i32;
        let mut v_start = data.len() as i32;
        if has_texture_coordinates {
            let mut extra = Packet::new(data);
            extra.pos = data.len() - 26;
            let back = data[extra.pos - 1] as i8;
            extra.pos = (extra.pos as i64 - i64::from(back)) as usize;
            m.textured_vertex_count = extra.g2()?;
            let attachments_length = extra.g2()?;
            let face_offsets_length = extra.g2()?;
            face_offsets_start = attachments_start + attachments_length;
            vertex_texture_counts_start = face_offsets_start + face_offsets_length;
            u_start = m.vertex_count + vertex_texture_counts_start;
            v_start = m.textured_vertex_count * 2 + u_start;
        }
        let vc = m.vertex_count as usize;
        let fc = m.face_count as usize;
        m.vertex_x = vec![0; vc];
        m.vertex_y = vec![0; vc];
        m.vertex_z = vec![0; vc];
        m.face_vertex1 = vec![0; fc];
        m.face_vertex2 = vec![0; fc];
        m.face_vertex3 = vec![0; fc];
        if vertex_label_mode == 1 {
            m.vertex_label = Some(vec![0; vc]);
        }
        if has_face_types {
            m.face_type = Some(vec![0; fc]);
        }
        if priority_mode == 255 {
            m.face_priority = Some(vec![0; fc]);
        } else {
            m.default_priority = priority_mode as i8;
        }
        if trans_mode == 1 {
            m.face_trans = Some(vec![0; fc]);
        }
        if face_label_mode == 1 {
            m.face_label = Some(vec![0; fc]);
        }
        if material_mode == 1 {
            m.face_material = Some(vec![0; fc]);
        }
        if material_mode == 1 && (m.texture_triangle_count > 0 || m.textured_vertex_count > 0) {
            m.face_mapping = Some(vec![0; fc]);
        }
        m.face_colour = vec![0; fc];
        if m.texture_triangle_count > 0 {
            let tc = m.texture_triangle_count as usize;
            m.texture_triangle_vertex1 = Some(vec![0; tc]);
            m.texture_triangle_vertex2 = Some(vec![0; tc]);
            m.texture_triangle_vertex3 = Some(vec![0; tc]);
            if mapped_triangle_count > 0 {
                let n = mapped_triangle_count as usize;
                m.texture_triangle_scale_x = Some(vec![0; n]);
                m.texture_triangle_scale_y = Some(vec![0; n]);
                m.texture_triangle_scale_z = Some(vec![0; n]);
                m.texture_triangle_rotation = Some(vec![0; n]);
                m.texture_triangle_direction = Some(vec![0; n]);
                m.texture_triangle_speed = Some(vec![0; n]);
            }
            if cube_triangle_count > 0 {
                let n = cube_triangle_count as usize;
                m.texture_triangle_translation_u = Some(vec![0; n]);
                m.texture_triangle_translation_v = Some(vec![0; n]);
            }
        }
        p2.pos = vertex_flags_start as usize;
        p3.pos = vertex_x_start as usize;
        p4.pos = vertex_y_start as usize;
        p5.pos = vertex_z_start as usize;
        p6.pos = vertex_labels_start as usize;
        let mut previous_x = 0;
        let mut previous_y = 0;
        let mut previous_z = 0;
        for vertex_index in 0..vc {
            let vertex_flags = p2.g1()?;
            let mut dx = 0;
            if (vertex_flags & 0x1) != 0 {
                dx = p3.gsmart1or2s()?;
            }
            let mut dy = 0;
            if (vertex_flags & 0x2) != 0 {
                dy = p4.gsmart1or2s()?;
            }
            let mut dz = 0;
            if (vertex_flags & 0x4) != 0 {
                dz = p5.gsmart1or2s()?;
            }
            m.vertex_x[vertex_index] = previous_x + dx;
            m.vertex_y[vertex_index] = previous_y + dy;
            m.vertex_z[vertex_index] = previous_z + dz;
            previous_x = m.vertex_x[vertex_index];
            previous_y = m.vertex_y[vertex_index];
            previous_z = m.vertex_z[vertex_index];
            if vertex_label_mode == 1 {
                let labels = m.vertex_label.as_mut().expect("allocated");
                if wide_vertex_labels {
                    labels[vertex_index] = p6.gsmart1or2null()?;
                } else {
                    labels[vertex_index] = p6.g1()?;
                    if labels[vertex_index] == 255 {
                        labels[vertex_index] = -1;
                    }
                }
            }
        }
        if m.textured_vertex_count > 0 {
            p2.pos = vertex_texture_counts_start as usize;
            p3.pos = u_start as usize;
            p4.pos = v_start as usize;
            let mut vtv = vec![0; vc];
            let mut texture_offset = 0;
            for slot in vtv.iter_mut() {
                *slot = texture_offset;
                texture_offset += p2.g1()?;
            }
            m.vertex_texture_vertex = Some(vtv);
            m.face_texture_vertex_offset1 = Some(vec![0; fc]);
            m.face_texture_vertex_offset2 = Some(vec![0; fc]);
            m.face_texture_vertex_offset3 = Some(vec![0; fc]);
            let n = m.textured_vertex_count as usize;
            let mut us = Vec::with_capacity(n);
            let mut vs = Vec::with_capacity(n);
            for _ in 0..n {
                us.push(p3.g2s()? as f32 / 4096.0);
                vs.push(p4.g2s()? as f32 / 4096.0);
            }
            m.texture_vertex_u = Some(us);
            m.texture_vertex_v = Some(vs);
        }
        p2.pos = face_colour_start as usize;
        p3.pos = face_types_start as usize;
        p4.pos = priorities_start as usize;
        p5.pos = face_trans_start as usize;
        p6.pos = face_labels_start as usize;
        p7.pos = face_material_start as usize;
        p8.pos = face_mapping_start as usize;
        for face in 0..fc {
            m.face_colour[face] = p2.g2()? as i16;
            if has_face_types {
                m.face_type.as_mut().expect("allocated")[face] = p3.g1b()?;
            }
            if priority_mode == 255 {
                m.face_priority.as_mut().expect("allocated")[face] = p4.g1b()?;
            }
            if trans_mode == 1 {
                m.face_trans.as_mut().expect("allocated")[face] = p5.g1b()?;
            }
            if face_label_mode == 1 {
                let labels = m.face_label.as_mut().expect("allocated");
                if wide_face_labels {
                    labels[face] = p6.gsmart1or2null()?;
                } else {
                    labels[face] = p6.g1()?;
                    if labels[face] == 255 {
                        labels[face] = -1;
                    }
                }
            }
            if material_mode == 1 {
                m.face_material.as_mut().expect("allocated")[face] = (p7.g2()? - 1) as i16;
            }
            if let Some(face_mapping) = m.face_mapping.as_mut() {
                let material = m.face_material.as_ref().expect("materials")[face];
                let mapping = if material == -1 {
                    -1
                } else if m.version >= 16 {
                    (p8.gsmart1or2()? - 1) as i16
                } else {
                    (p8.g1()? - 1) as i16
                };
                face_mapping[face] = mapping;
            }
        }
        m.used_vertex_count = -1;
        p2.pos = face_index_start as usize;
        p3.pos = face_opcodes_start as usize;
        p4.pos = face_offsets_start as usize;
        m.read_face_indices(&mut p2, &mut p3, &mut p4)?;
        p2.pos = plain_triangle_start as usize;
        p3.pos = mapped_triangle_start as usize;
        p4.pos = scale_start as usize;
        p5.pos = rotation_start as usize;
        p6.pos = direction_start as usize;
        p7.pos = speed_start as usize;
        m.read_texture_triangles(&mut p2, &mut p3, &mut p4, &mut p5, &mut p6, &mut p7)?;
        p2.pos = attachments_start as usize;
        if has_particles {
            let emitter_count = p2.g1()?;
            if emitter_count > 0 {
                let mut emitters = Vec::with_capacity(emitter_count as usize);
                for _ in 0..emitter_count {
                    let particle = p2.g2()?;
                    let emitter_face = p2.g2()?;
                    let priority = if priority_mode == 255 {
                        m.face_priority.as_ref().expect("priorities")[emitter_face as usize]
                    } else {
                        priority_mode as i8
                    };
                    emitters.push(ModelParticleEmitter {
                        particle,
                        face: emitter_face,
                        v1: i32::from(m.face_vertex1[emitter_face as usize]),
                        v2: i32::from(m.face_vertex2[emitter_face as usize]),
                        v3: i32::from(m.face_vertex3[emitter_face as usize]),
                        priority,
                    });
                }
                m.emitters = Some(emitters);
            }
            let effector_count = p2.g1()?;
            if effector_count > 0 {
                let mut effectors = Vec::with_capacity(effector_count as usize);
                for _ in 0..effector_count {
                    let effector = p2.g2()?;
                    let effector_vertex = p2.g2()?;
                    effectors.push(ModelParticleEffector {
                        effector,
                        vertex: effector_vertex,
                    });
                }
                m.effectors = Some(effectors);
            }
        }
        if has_billboards {
            let billboard_count = p2.g1()?;
            if billboard_count > 0 {
                let mut billboards = Vec::with_capacity(billboard_count as usize);
                for _ in 0..billboard_count {
                    let billboard_type = p2.g2()?;
                    let billboard_face = p2.g2()?;
                    let label = if wide_billboard_labels {
                        p2.gsmart1or2null()?
                    } else {
                        let v = p2.g1()?;
                        if v == 255 {
                            -1
                        } else {
                            v
                        }
                    };
                    let depth = p2.g1b()?;
                    billboards.push(ModelBillboard {
                        billboard_type,
                        face: billboard_face,
                        label,
                        depth_offset: i32::from(depth),
                    });
                }
                m.billboard = Some(billboards);
            }
        }
        Ok(m)
    }

    /// Reads the face corners: each face's opcode says which of the previous
    /// face's corners it reuses and how many new deltas follow.
    fn read_face_indices(
        &mut self,
        index_deltas: &mut Packet<'_>,
        opcodes: &mut Packet<'_>,
        texture_offsets: &mut Packet<'_>,
    ) -> anyhow::Result<()> {
        let mut a: i16 = 0;
        let mut b: i16 = 0;
        let mut c: i16 = 0;
        let mut last: i16 = 0;
        for face in 0..self.face_count as usize {
            let opcode_byte = opcodes.g1()?;
            let opcode = opcode_byte & 0x7;
            if opcode == 1 {
                a = (index_deltas.gsmart1or2s()? + i32::from(last)) as i16;
                self.face_vertex1[face] = a;
                b = (index_deltas.gsmart1or2s()? + i32::from(a)) as i16;
                self.face_vertex2[face] = b;
                c = (index_deltas.gsmart1or2s()? + i32::from(b)) as i16;
                self.face_vertex3[face] = c;
                last = c;
                if i32::from(a) > self.used_vertex_count {
                    self.used_vertex_count = i32::from(a);
                }
                if i32::from(b) > self.used_vertex_count {
                    self.used_vertex_count = i32::from(b);
                }
                if i32::from(c) > self.used_vertex_count {
                    self.used_vertex_count = i32::from(c);
                }
            }
            if opcode == 2 {
                b = c;
                c = (index_deltas.gsmart1or2s()? + i32::from(last)) as i16;
                last = c;
                self.face_vertex1[face] = a;
                self.face_vertex2[face] = b;
                self.face_vertex3[face] = c;
                if i32::from(c) > self.used_vertex_count {
                    self.used_vertex_count = i32::from(c);
                }
            }
            if opcode == 3 {
                a = c;
                c = (index_deltas.gsmart1or2s()? + i32::from(last)) as i16;
                last = c;
                self.face_vertex1[face] = a;
                self.face_vertex2[face] = b;
                self.face_vertex3[face] = c;
                if i32::from(c) > self.used_vertex_count {
                    self.used_vertex_count = i32::from(c);
                }
            }
            if opcode == 4 {
                let previous_a = a;
                std::mem::swap(&mut a, &mut b);
                c = (index_deltas.gsmart1or2s()? + i32::from(last)) as i16;
                last = c;
                self.face_vertex1[face] = a;
                self.face_vertex2[face] = previous_a;
                self.face_vertex3[face] = c;
                if i32::from(c) > self.used_vertex_count {
                    self.used_vertex_count = i32::from(c);
                }
            }
            if self.textured_vertex_count > 0 && (opcode_byte & 0x8) != 0 {
                self.face_texture_vertex_offset1
                    .as_mut()
                    .expect("tex offsets")[face] = texture_offsets.g1()? as i8;
                self.face_texture_vertex_offset2
                    .as_mut()
                    .expect("tex offsets")[face] = texture_offsets.g1()? as i8;
                self.face_texture_vertex_offset3
                    .as_mut()
                    .expect("tex offsets")[face] = texture_offsets.g1()? as i8;
            }
        }
        self.used_vertex_count += 1;
        Ok(())
    }

    /// Reads the texture triangles: plain triangles carry three vertices,
    /// mapped ones (kinds 1 to 3) also a scale, rotation, direction, speed and
    /// (kind 2) translation.
    fn read_texture_triangles(
        &mut self,
        plain_vertices: &mut Packet<'_>,
        mapped_vertices: &mut Packet<'_>,
        scales: &mut Packet<'_>,
        rotations: &mut Packet<'_>,
        directions: &mut Packet<'_>,
        speeds: &mut Packet<'_>,
    ) -> anyhow::Result<()> {
        let count = self.texture_triangle_count as usize;
        for triangle in 0..count {
            let kind = self.texture_triangle_type.as_ref().expect("types")[triangle] as u8;
            if kind == 0 {
                self.texture_triangle_vertex1.as_mut().expect("tt")[triangle] =
                    plain_vertices.g2()? as i16;
                self.texture_triangle_vertex2.as_mut().expect("tt")[triangle] =
                    plain_vertices.g2()? as i16;
                self.texture_triangle_vertex3.as_mut().expect("tt")[triangle] =
                    plain_vertices.g2()? as i16;
            }
            if (1..=3).contains(&kind) {
                self.texture_triangle_vertex1.as_mut().expect("tt")[triangle] =
                    mapped_vertices.g2()? as i16;
                self.texture_triangle_vertex2.as_mut().expect("tt")[triangle] =
                    mapped_vertices.g2()? as i16;
                self.texture_triangle_vertex3.as_mut().expect("tt")[triangle] =
                    mapped_vertices.g2()? as i16;
                let (sx, sy, sz) = if self.version < 15 {
                    let sx = scales.g2()?;
                    let sy = if self.version < 14 {
                        scales.g2()?
                    } else {
                        scales.g3()?
                    };
                    let sz = scales.g2()?;
                    (sx, sy, sz)
                } else {
                    (scales.g3()?, scales.g3()?, scales.g3()?)
                };
                self.texture_triangle_scale_x.as_mut().expect("scale")[triangle] = sx;
                self.texture_triangle_scale_y.as_mut().expect("scale")[triangle] = sy;
                self.texture_triangle_scale_z.as_mut().expect("scale")[triangle] = sz;
                self.texture_triangle_rotation.as_mut().expect("rot")[triangle] =
                    rotations.g1b()?;
                self.texture_triangle_direction.as_mut().expect("dir")[triangle] =
                    directions.g1b()?;
                self.texture_triangle_speed.as_mut().expect("speed")[triangle] =
                    i32::from(speeds.g1b()?);
                if kind == 2 {
                    self.texture_triangle_translation_u.as_mut().expect("trans")[triangle] =
                        i32::from(speeds.g1b()?);
                    self.texture_triangle_translation_v.as_mut().expect("trans")[triangle] =
                        i32::from(speeds.g1b()?);
                }
            }
        }
        Ok(())
    }

    /// Merges several parts into one model, joining vertices at equal
    /// positions.
    #[allow(clippy::too_many_lines)]
    #[must_use]
    pub fn merge(parts: &[&ModelUnlit]) -> Self {
        Self::merge_slots(&parts.iter().copied().map(Some).collect::<Vec<_>>())
    }

    /// [`Self::merge`] with empty slots: a missing part keeps its slot index in
    /// the source-model masks (an empty placeholder model would also change
    /// how the priority array is allocated).
    pub fn merge_slots(parts: &[Option<&ModelUnlit>]) -> Self {
        let mut m = Self {
            version: 12,
            // Provenance: the parts' model ids in face order.
            source_ids: parts
                .iter()
                .flatten()
                .map(|p| p.source_ids.as_deref())
                .collect::<Option<Vec<&[u32]>>>()
                .map(|ids| ids.concat().into()),
            ..Self::default()
        };
        let mut emitter_total = 0_usize;
        let mut effector_total = 0_usize;
        let mut billboard_total = 0_usize;
        let mut any_face_type = false;
        let mut per_face_priority = false;
        let mut any_trans = false;
        let mut any_mapping = false;
        let mut any_material = false;
        let mut any_face_label = false;
        m.default_priority = -1;
        for source_part in parts.iter().flatten() {
            m.vertex_count += source_part.vertex_count;
            m.face_count += source_part.face_count;
            m.texture_triangle_count += source_part.texture_triangle_count;
            m.textured_vertex_count += source_part.textured_vertex_count;
            if let Some(e) = &source_part.emitters {
                emitter_total += e.len();
            }
            if let Some(e) = &source_part.effectors {
                effector_total += e.len();
            }
            if let Some(b) = &source_part.billboard {
                billboard_total += b.len();
            }
            any_face_type |= source_part.face_type.is_some();
            if source_part.face_priority.is_none() {
                if m.default_priority == -1 {
                    m.default_priority = source_part.default_priority;
                }
                if m.default_priority != source_part.default_priority {
                    per_face_priority = true;
                }
            } else {
                per_face_priority = true;
            }
            any_trans |= source_part.face_trans.is_some();
            any_mapping |= source_part.face_mapping.is_some();
            any_material |= source_part.face_material.is_some();
            any_face_label |= source_part.face_label.is_some();
        }
        let vc = m.vertex_count as usize;
        let fc = m.face_count as usize;
        m.vertex_x = vec![0; vc];
        m.vertex_y = vec![0; vc];
        m.vertex_z = vec![0; vc];
        m.vertex_label = Some(vec![0; vc]);
        m.vertex_source_models = Some(vec![0; vc]);
        m.face_vertex1 = vec![0; fc];
        m.face_vertex2 = vec![0; fc];
        m.face_vertex3 = vec![0; fc];
        if any_face_type {
            m.face_type = Some(vec![0; fc]);
        }
        if per_face_priority {
            m.face_priority = Some(vec![0; fc]);
        }
        if any_trans {
            m.face_trans = Some(vec![0; fc]);
        }
        if any_mapping {
            m.face_mapping = Some(vec![0; fc]);
        }
        m.face_colour = vec![0; fc];
        if any_material {
            m.face_material = Some(vec![0; fc]);
        }
        if any_face_label {
            m.face_label = Some(vec![0; fc]);
        }
        m.face_source_models = Some(vec![0; fc]);
        if m.texture_triangle_count > 0 {
            let tc = m.texture_triangle_count as usize;
            m.texture_triangle_type = Some(vec![0; tc]);
            m.texture_triangle_vertex1 = Some(vec![0; tc]);
            m.texture_triangle_vertex2 = Some(vec![0; tc]);
            m.texture_triangle_vertex3 = Some(vec![0; tc]);
            m.texture_triangle_scale_x = Some(vec![0; tc]);
            m.texture_triangle_scale_y = Some(vec![0; tc]);
            m.texture_triangle_scale_z = Some(vec![0; tc]);
            m.texture_triangle_rotation = Some(vec![0; tc]);
            m.texture_triangle_direction = Some(vec![0; tc]);
            m.texture_triangle_speed = Some(vec![0; tc]);
            m.texture_triangle_translation_u = Some(vec![0; tc]);
            m.texture_triangle_translation_v = Some(vec![0; tc]);
        }
        if billboard_total > 0 {
            m.billboard = Some(Vec::with_capacity(billboard_total));
        }
        if emitter_total > 0 {
            m.emitters = Some(Vec::with_capacity(emitter_total));
        }
        if effector_total > 0 {
            m.effectors = Some(Vec::with_capacity(effector_total));
        }
        let tex_count = m.textured_vertex_count as usize;
        if m.textured_vertex_count > 0 {
            m.texture_vertex_u = Some(vec![0.0; tex_count]);
            m.texture_vertex_v = Some(vec![0.0; tex_count]);
            m.vertex_texture_vertex = Some(vec![0; vc]);
            m.face_texture_vertex_offset1 = Some(vec![0; fc]);
            m.face_texture_vertex_offset2 = Some(vec![0; fc]);
            m.face_texture_vertex_offset3 = Some(vec![0; fc]);
        }
        let mut texture_vertex_counts = vec![0_i32; vc];
        let mut texture_sort_keys = vec![0_i32; tex_count];
        let mut vertex_map = vec![0_i32; vc];
        let mut texture_base = vec![0_i32; vc];
        let mut corners = [0_i32; 3];
        m.vertex_count = 0;
        m.face_count = 0;
        m.texture_triangle_count = 0;
        m.textured_vertex_count = 0;
        for (part_index, part) in parts.iter().enumerate() {
            let Some(part) = part else {
                continue;
            };
            let part_bit = (1_i32 << part_index) as i16;
            let first_face = m.face_count;
            let mut texture_copied = vec![false; part.vertex_count as usize];
            if let Some(bbs) = &part.billboard {
                for billboard in bbs {
                    m.billboard
                        .as_mut()
                        .expect("billboards")
                        .push(ModelBillboard {
                            face: billboard.face + m.face_count,
                            ..*billboard
                        });
                }
            }
            for face in 0..part.face_count as usize {
                corners[0] = i32::from(part.face_vertex1[face]);
                corners[1] = i32::from(part.face_vertex2[face]);
                corners[2] = i32::from(part.face_vertex3[face]);
                for &corner in &corners {
                    let corner_index = corner as usize;
                    let x = part.vertex_x[corner_index];
                    let y = part.vertex_y[corner_index];
                    let z = part.vertex_z[corner_index];
                    let mut merged_vertex = 0_usize;
                    while merged_vertex < m.vertex_count as usize {
                        if m.vertex_x[merged_vertex] == x
                            && m.vertex_y[merged_vertex] == y
                            && m.vertex_z[merged_vertex] == z
                        {
                            m.vertex_source_models.as_mut().expect("src")[merged_vertex] |=
                                part_bit;
                            vertex_map[corner_index] = merged_vertex as i32;
                            break;
                        }
                        merged_vertex += 1;
                    }
                    if part.textured_vertex_count > 0 && !texture_copied[corner_index] {
                        let vtv = part.vertex_texture_vertex.as_ref().expect("vtv");
                        let texture_count = (if corner < part.vertex_count - 1 {
                            vtv[corner_index + 1]
                        } else {
                            part.textured_vertex_count
                        }) - vtv[corner_index];
                        let us = part.texture_vertex_u.as_ref().expect("u");
                        let vs = part.texture_vertex_v.as_ref().expect("v");
                        for texture_step in 0..texture_count {
                            let idx = m.textured_vertex_count as usize;
                            m.texture_vertex_u.as_mut().expect("u")[idx] =
                                us[(vtv[corner_index] + texture_step) as usize];
                            m.texture_vertex_v.as_mut().expect("v")[idx] =
                                vs[(vtv[corner_index] + texture_step) as usize];
                            // `merged_vertex` may equal the merged vertex count
                            // (a fresh vertex); the array is sized for the
                            // total, so it is in range.
                            texture_sort_keys[idx] = ((merged_vertex as i32) << 16)
                                | (texture_vertex_counts[merged_vertex] + texture_step);
                            m.textured_vertex_count += 1;
                        }
                        texture_base[corner_index] = texture_vertex_counts[merged_vertex];
                        texture_vertex_counts[merged_vertex] += texture_count;
                        texture_copied[corner_index] = true;
                    }
                    if merged_vertex >= m.vertex_count as usize {
                        let n = m.vertex_count as usize;
                        m.vertex_x[n] = x;
                        m.vertex_y[n] = y;
                        m.vertex_z[n] = z;
                        m.vertex_source_models.as_mut().expect("src")[n] = part_bit;
                        m.vertex_label.as_mut().expect("labels")[n] =
                            part.vertex_label.as_ref().map_or(-1, |l| l[corner_index]);
                        vertex_map[corner_index] = m.vertex_count;
                        m.vertex_count += 1;
                    }
                }
            }
            for copy_face in 0..part.face_count as usize {
                let f = m.face_count as usize;
                if any_face_type {
                    if let Some(t) = &part.face_type {
                        m.face_type.as_mut().expect("ft")[f] = t[copy_face];
                    }
                }
                if per_face_priority {
                    m.face_priority.as_mut().expect("fp")[f] = match &part.face_priority {
                        None => part.default_priority,
                        Some(p) => p[copy_face],
                    };
                }
                if any_trans {
                    if let Some(t) = &part.face_trans {
                        m.face_trans.as_mut().expect("tr")[f] = t[copy_face];
                    }
                }
                if any_material {
                    m.face_material.as_mut().expect("mat")[f] = match &part.face_material {
                        None => -1,
                        Some(mt) => mt[copy_face],
                    };
                }
                if any_face_label {
                    m.face_label.as_mut().expect("fl")[f] = match &part.face_label {
                        None => -1,
                        Some(l) => l[copy_face],
                    };
                }
                if part.textured_vertex_count > 0 {
                    let o1 = part.face_texture_vertex_offset1.as_ref().expect("o1");
                    let o2 = part.face_texture_vertex_offset2.as_ref().expect("o2");
                    let o3 = part.face_texture_vertex_offset3.as_ref().expect("o3");
                    m.face_texture_vertex_offset1.as_mut().expect("o1")[f] =
                        (texture_base[part.face_vertex1[copy_face] as usize]
                            + i32::from(o1[copy_face])) as i8;
                    m.face_texture_vertex_offset2.as_mut().expect("o2")[f] =
                        (texture_base[part.face_vertex2[copy_face] as usize]
                            + i32::from(o2[copy_face])) as i8;
                    m.face_texture_vertex_offset3.as_mut().expect("o3")[f] =
                        (texture_base[part.face_vertex3[copy_face] as usize]
                            + i32::from(o3[copy_face])) as i8;
                }
                m.face_vertex1[f] = vertex_map[part.face_vertex1[copy_face] as usize] as i16;
                m.face_vertex2[f] = vertex_map[part.face_vertex2[copy_face] as usize] as i16;
                m.face_vertex3[f] = vertex_map[part.face_vertex3[copy_face] as usize] as i16;
                m.face_source_models.as_mut().expect("f1399")[f] = part_bit;
                m.face_colour[f] = part.face_colour[copy_face];
                m.face_count += 1;
            }
            if let Some(em) = &part.emitters {
                for e in em {
                    m.emitters
                        .as_mut()
                        .expect("emitters")
                        .push(ModelParticleEmitter {
                            particle: e.particle,
                            face: e.face + first_face,
                            v1: vertex_map[e.v1 as usize],
                            v2: vertex_map[e.v2 as usize],
                            v3: vertex_map[e.v3 as usize],
                            priority: e.priority,
                        });
                }
            }
            if let Some(ef) = &part.effectors {
                for e in ef {
                    m.effectors
                        .as_mut()
                        .expect("effectors")
                        .push(ModelParticleEffector {
                            effector: e.effector,
                            vertex: vertex_map[e.vertex as usize],
                        });
                }
            }
        }
        m.used_vertex_count = m.vertex_count;
        if m.textured_vertex_count > 0 {
            let us = m.texture_vertex_u.as_mut().expect("u");
            let vs = m.texture_vertex_v.as_mut().expect("v");
            sort_texture_coordinates(&mut texture_sort_keys, us, vs);
            let vtv = m.vertex_texture_vertex.as_mut().expect("vtv");
            let mut running = 0;
            for vertex in 0..m.vertex_count as usize {
                vtv[vertex] = running;
                running += texture_vertex_counts[vertex];
            }
        }
        let mut face_cursor = 0_usize;
        for (part_slot, source) in parts.iter().enumerate() {
            let Some(source) = source else {
                continue;
            };
            let source_bit = (1_i32 << part_slot) as i16;
            if any_mapping {
                for mapping_face in 0..source.face_count as usize {
                    let mapping = source
                        .face_mapping
                        .as_ref()
                        .map_or(-1, |fm| fm[mapping_face]);
                    let fm = m.face_mapping.as_mut().expect("fm");
                    fm[face_cursor] = mapping;
                    if fm[face_cursor] > -1 && fm[face_cursor] < 32766 {
                        fm[face_cursor] =
                            (i32::from(fm[face_cursor]) + m.texture_triangle_count) as i16;
                    }
                    face_cursor += 1;
                }
            }
            for triangle in 0..source.texture_triangle_count as usize {
                let t = m.texture_triangle_count as usize;
                let kind = source.texture_triangle_type.as_ref().expect("tt")[triangle];
                m.texture_triangle_type.as_mut().expect("tt")[t] = kind;
                if kind == 0 {
                    let a = m.find_or_add_vertex(
                        source,
                        i32::from(source.texture_triangle_vertex1.as_ref().expect("v1")[triangle]),
                        source_bit,
                    );
                    m.texture_triangle_vertex1.as_mut().expect("v1")[t] = a as i16;
                    let b = m.find_or_add_vertex(
                        source,
                        i32::from(source.texture_triangle_vertex2.as_ref().expect("v2")[triangle]),
                        source_bit,
                    );
                    m.texture_triangle_vertex2.as_mut().expect("v2")[t] = b as i16;
                    let c = m.find_or_add_vertex(
                        source,
                        i32::from(source.texture_triangle_vertex3.as_ref().expect("v3")[triangle]),
                        source_bit,
                    );
                    m.texture_triangle_vertex3.as_mut().expect("v3")[t] = c as i16;
                } else if (1..=3).contains(&kind) {
                    m.texture_triangle_vertex1.as_mut().expect("v1")[t] =
                        source.texture_triangle_vertex1.as_ref().expect("v1")[triangle];
                    m.texture_triangle_vertex2.as_mut().expect("v2")[t] =
                        source.texture_triangle_vertex2.as_ref().expect("v2")[triangle];
                    m.texture_triangle_vertex3.as_mut().expect("v3")[t] =
                        source.texture_triangle_vertex3.as_ref().expect("v3")[triangle];
                    m.texture_triangle_scale_x.as_mut().expect("sx")[t] =
                        source.texture_triangle_scale_x.as_ref().expect("sx")[triangle];
                    m.texture_triangle_scale_y.as_mut().expect("sy")[t] =
                        source.texture_triangle_scale_y.as_ref().expect("sy")[triangle];
                    m.texture_triangle_scale_z.as_mut().expect("sz")[t] =
                        source.texture_triangle_scale_z.as_ref().expect("sz")[triangle];
                    m.texture_triangle_rotation.as_mut().expect("rot")[t] =
                        source.texture_triangle_rotation.as_ref().expect("rot")[triangle];
                    m.texture_triangle_direction.as_mut().expect("dir")[t] =
                        source.texture_triangle_direction.as_ref().expect("dir")[triangle];
                    m.texture_triangle_speed.as_mut().expect("speed")[t] =
                        source.texture_triangle_speed.as_ref().expect("speed")[triangle];
                }
                // The kind-2 translations are not copied (a client quirk kept
                // as it is).
                m.texture_triangle_count += 1;
            }
        }
        m
    }

    /// Finds or adds the vertex of
    /// `part` at `vertex` in the merged model.
    fn find_or_add_vertex(&mut self, part: &ModelUnlit, vertex: i32, part_bit: i16) -> i32 {
        let x = part.vertex_x[vertex as usize];
        let y = part.vertex_y[vertex as usize];
        let z = part.vertex_z[vertex as usize];
        for merged in 0..self.vertex_count as usize {
            if self.vertex_x[merged] == x
                && self.vertex_y[merged] == y
                && self.vertex_z[merged] == z
            {
                self.vertex_source_models.as_mut().expect("src")[merged] |= part_bit;
                return merged as i32;
            }
        }
        let n = self.vertex_count as usize;
        self.vertex_x[n] = x;
        self.vertex_y[n] = y;
        self.vertex_z[n] = z;
        self.vertex_source_models.as_mut().expect("src")[n] = part_bit;
        self.vertex_label.as_mut().expect("labels")[n] = part
            .vertex_label
            .as_ref()
            .map_or(-1, |l| l[vertex as usize]);
        self.vertex_count += 1;
        n as i32
    }

    /// Moves every vertex by `(x, y, z)` before a lit model is built.
    pub fn translate(&mut self, x: i32, y: i32, z: i32) {
        for i in 0..self.vertex_count as usize {
            self.vertex_x[i] = self.vertex_x[i].wrapping_add(x);
            self.vertex_y[i] = self.vertex_y[i].wrapping_add(y);
            self.vertex_z[i] = self.vertex_z[i].wrapping_add(z);
        }
    }
    /// Turns the model by fixed-point Z, X, Y rotations, in that order.
    pub fn rotate(&mut self, x: i32, y: i32, z: i32) {
        for (angle, axis) in [(z, 2), (x, 0), (y, 1)] {
            if angle == 0 {
                continue;
            }
            assert!((0..16384).contains(&angle), "rotation table index");
            let sn = crate::trig::sin(angle);
            let cs = crate::trig::cos(angle);
            for i in 0..self.vertex_count as usize {
                let (a, b) = match axis {
                    2 => (self.vertex_x[i], self.vertex_y[i]),
                    0 => (self.vertex_y[i], self.vertex_z[i]),
                    _ => (self.vertex_x[i], self.vertex_z[i]),
                };
                let (a, b) = if axis == 0 {
                    (
                        a.wrapping_mul(cs).wrapping_sub(b.wrapping_mul(sn)) >> 14,
                        b.wrapping_mul(cs).wrapping_add(a.wrapping_mul(sn)) >> 14,
                    )
                } else {
                    (
                        b.wrapping_mul(sn).wrapping_add(a.wrapping_mul(cs)) >> 14,
                        b.wrapping_mul(cs).wrapping_sub(a.wrapping_mul(sn)) >> 14,
                    )
                };
                match axis {
                    2 => {
                        self.vertex_x[i] = a;
                        self.vertex_y[i] = b;
                    }
                    0 => {
                        self.vertex_y[i] = a;
                        self.vertex_z[i] = b;
                    }
                    _ => {
                        self.vertex_x[i] = a;
                        self.vertex_z[i] = b;
                    }
                }
            }
        }
    }

    /// Replaces the face colour `from` by `to`.
    pub fn recolor(&mut self, from: i16, to: i16) {
        for c in self.face_colour.iter_mut().take(self.face_count as usize) {
            if *c == from {
                *c = to;
            }
        }
    }

    /// Replaces the face material `from` by `to`.
    pub fn rematerial(&mut self, from: i16, to: i16) {
        let count = self.face_count as usize;
        if let Some(mats) = self.face_material.as_mut() {
            for mt in mats.iter_mut().take(count) {
                if *mt == from {
                    *mt = to;
                }
            }
        }
    }

    /// Scales the vertices, and the texture triangle scales, by a float
    /// factor with truncation towards zero.
    pub fn scale_by(&mut self, scale: f32) {
        for i in 0..self.vertex_count as usize {
            self.vertex_x[i] = (self.vertex_x[i] as f32 * scale) as i32;
            self.vertex_y[i] = (self.vertex_y[i] as f32 * scale) as i32;
            self.vertex_z[i] = (self.vertex_z[i] as f32 * scale) as i32;
        }
        if self.texture_triangle_count <= 0 || self.texture_triangle_scale_x.is_none() {
            return;
        }
        let types = self.texture_triangle_type.as_ref().expect("types");
        let sx = self.texture_triangle_scale_x.as_mut().expect("sx");
        let sy = self.texture_triangle_scale_y.as_mut().expect("sy");
        let sz = self.texture_triangle_scale_z.as_mut().expect("sz");
        for i in 0..sx.len() {
            sx[i] = (sx[i] as f32 * scale) as i32;
            sy[i] = (sy[i] as f32 * scale) as i32;
            if types[i] != 1 {
                sz[i] = (sz[i] as f32 * scale) as i32;
            }
        }
    }

    /// Scales the vertices, and the texture triangle scales, by `1 << shift`.
    pub fn scale_by_power_of_two(&mut self, shift: i32) {
        for vertex in 0..self.vertex_count as usize {
            self.vertex_x[vertex] <<= shift;
            self.vertex_y[vertex] <<= shift;
            self.vertex_z[vertex] <<= shift;
        }
        if self.texture_triangle_count <= 0 || self.texture_triangle_scale_x.is_none() {
            return;
        }
        let types = self.texture_triangle_type.as_ref().expect("types");
        let sx = self.texture_triangle_scale_x.as_mut().expect("sx");
        let sy = self.texture_triangle_scale_y.as_mut().expect("sy");
        let sz = self.texture_triangle_scale_z.as_mut().expect("sz");
        for triangle in 0..sx.len() {
            sx[triangle] <<= shift;
            sy[triangle] <<= shift;
            if types[triangle] != 1 {
                sz[triangle] <<= shift;
            }
        }
    }
}

/// Sorts the texture coordinates of a merged model by `keys` ascending, with
/// a deterministic jitter on the pivot comparison (the order of equal keys is
/// part of the result).
fn sort_texture_coordinates(keys: &mut [i32], us: &mut [f32], vs: &mut [f32]) {
    if keys.is_empty() {
        return;
    }
    sort_texture_range(keys, us, vs, 0, keys.len() as i32 - 1);
}

fn sort_texture_range(keys: &mut [i32], us: &mut [f32], vs: &mut [f32], low: i32, high: i32) {
    if low >= high {
        return;
    }
    let pivot_at = ((low + high) / 2) as usize;
    let mut store = low as usize;
    let end = high as usize;
    let pivot_key = keys[pivot_at];
    keys.swap(pivot_at, end);
    let pivot_u = us[pivot_at];
    us.swap(pivot_at, end);
    let pivot_v = vs[pivot_at];
    vs.swap(pivot_at, end);
    // Every other index compares against the pivot plus one.
    let jitter_mask = 1;
    for i in low as usize..end {
        if keys[i] < (i as i32 & jitter_mask) + pivot_key {
            keys.swap(i, store);
            us.swap(i, store);
            vs.swap(i, store);
            store += 1;
        }
    }
    keys[end] = keys[store];
    keys[store] = pivot_key;
    us[end] = us[store];
    us[store] = pivot_u;
    vs[end] = vs[store];
    vs[store] = pivot_v;
    sort_texture_range(keys, us, vs, low, store as i32 - 1);
    sort_texture_range(keys, us, vs, store as i32 + 1, high);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jittered_sort_orders_keys() {
        let mut keys = vec![5, 3, 9, 1, 7, 3];
        let mut a: Vec<f32> = keys.iter().map(|&k| k as f32).collect();
        let mut b = a.clone();
        sort_texture_coordinates(&mut keys, &mut a, &mut b);
        assert_eq!(keys, vec![1, 3, 3, 5, 7, 9]);
        assert_eq!(a, vec![1.0, 3.0, 3.0, 5.0, 7.0, 9.0]);
        assert_eq!(a, b);
    }
}
