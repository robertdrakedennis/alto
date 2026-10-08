//! The lit, uploadable model: a decoded [`ModelUnlit`](crate::modelunlit::ModelUnlit) with its faces
//! filtered and sorted, vertices deduplicated per (normal, colour, texture
//! coordinate), normals and texture coordinates generated, and the material
//! batches the renderers draw.
//!
//! - `build`: [`GpuModel::new`], the build from a [`ModelUnlit`](crate::modelunlit::ModelUnlit).
//! - `texture_mapping`: the per-face texture coordinates (default, direct,
//!   projected, cylindrical, cube and spherical mappings).
//! - `transform`: turns, mirror, scale, translate, terrain draping and the
//!   replacement transform. `bounds`: cached extents, radii and height.
//! - `normals`: merging the normals of coincident vertices of two models.
//! - `colour`: recolour, retexture, tint and the per-vertex colour bake.
//! - `streams`: the vertex and index streams a renderer uploads.
//! - `shadow`: the sun-projected hard shadow.
//! - `attachments`: particle anchors and billboards.
//! - `animation`: label-group deformation from classic and skeletal poses.
//!
//! Placement copies own their arrays. Particle emitter triangles and effector
//! vertices, and billboards, are retained beside the geometry and follow its
//! colour edits and poses.

use crate::texture::{Material, MaterialStore};

mod animation;
mod attachments;
mod bounds;
mod build;
mod colour;
mod normals;
mod shadow;
mod sort;
mod streams;
mod texture_mapping;
mod transform;

#[cfg(any(test, feature = "test-hooks"))] // the unselected forms are exercised by tests only
pub use animation::{classic_transforms, classic_transforms_masked};
pub use animation::{classic_transforms_selected, ClassicPose, PoseTarget, Transform};
pub use build::{BuildParams, ModelStores};
pub use transform::TerrainHeights;

/// The build detail flags the client sets with lighting detail 1 and textures
/// on: `0x1 | 0x10 | 0x20 | 0x2 | 0x4`.
pub const MODEL_DETAIL_FLAGS: i32 = 0x37;
/// The extra detail bit when textures are off.
pub const MODEL_DETAIL_NO_TEXTURES: i32 = 0x40;

/// The merged-normal copies [`GpuModel::merge_normals`] writes, so the base
/// arrays stay shared until two models meet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedNormals {
    pub nx: Vec<i16>,
    pub ny: Vec<i16>,
    pub nz: Vec<i16>,
    /// Faces contributing to each normal.
    pub count: Vec<i8>,
}

/// A particle emitter attached to a face of the source model: the emitter type
/// and the face's three vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParticleEmitterRef {
    pub particle: i32,
    pub vertices: [usize; 3],
}

/// A particle effector attached to a vertex of the source model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParticleEffectorRef {
    pub effector: i32,
    pub vertex: usize,
}

/// The lit model. Faces are the source's kept faces in draw order; vertices
/// are the unique (uploaded) vertices, each remembering the source vertex and
/// face it came from.
#[derive(Clone, Debug)]
pub struct GpuModel {
    /// Render flags the model was built with.
    pub flags: i32,
    /// Detail flags (`MODEL_DETAIL_FLAGS`).
    pub detail: i32,
    pub ambient: i16,
    pub contrast: i16,
    /// The source's vertex count.
    pub vertex_count_all: i32,
    /// The source vertices poses and transforms touch (highest face vertex
    /// index + 1).
    pub vertex_count: i32,
    /// Source vertex positions.
    pub vx: Vec<i32>,
    pub vy: Vec<i32>,
    pub vz: Vec<i32>,
    /// Per source vertex, the bit of the part (merged source model) it came
    /// from.
    pub vertex_source_models: Option<Vec<i16>>,
    /// Source vertices per skin label, and sorted faces per skin label.
    pub vertex_groups: Option<Vec<Vec<usize>>>,
    pub face_groups: Option<Vec<Vec<usize>>>,
    /// Unique (uploaded) vertex count.
    pub unique_count: i32,
    /// Unique vertex -> source vertex.
    pub unique_vertex: Vec<i16>,
    /// Unique vertex -> face.
    pub unique_face: Vec<i16>,
    /// Unique vertex normals and the count of faces behind each.
    pub nx: Vec<i16>,
    pub ny: Vec<i16>,
    pub nz: Vec<i16>,
    pub ncount: Vec<i8>,
    /// Unique vertex texture coordinates.
    pub u: Vec<f32>,
    pub v: Vec<f32>,
    pub merged: Option<MergedNormals>,
    /// Kept faces.
    pub face_count: i32,
    /// Drawn faces: the kept faces minus billboard and emitter faces.
    pub draw_face_count: i32,
    pub face_colour: Vec<i16>,
    pub face_alpha: Vec<i8>,
    pub face_material: Vec<i16>,
    /// Per face, the bit of the part it came from.
    pub face_part: Option<Vec<i16>>,
    /// Unique vertex indices of each face's corners.
    pub idx1: Vec<i16>,
    pub idx2: Vec<i16>,
    pub idx3: Vec<i16>,
    /// Per source vertex, its first slot in `vertex_slots`.
    pub vertex_offsets: Vec<i32>,
    /// Unique vertex index + 1 per slot, 0 = free.
    pub vertex_slots: Vec<i16>,
    /// First face of each material batch (`batches + 1` entries).
    pub batch_face_start: Vec<i32>,
    /// Lowest unique vertex and unique vertex span of each batch.
    pub batch_min_vertex: Vec<i32>,
    pub batch_vertex_span: Vec<i32>,
    pub has_transparency: bool,
    pub has_animated_uvs: bool,
    /// The billboard table, `None` without billboards.
    pub billboards: Option<crate::billboard::ModelBillboards>,
    /// The sorted kept faces' indices in the source model.
    pub face_source: Vec<i32>,
    /// The source's face count and per-face priorities, for consumers that
    /// walk faces in source order.
    pub source_face_count: i32,
    pub source_face_priority: Option<Vec<i8>>,
    /// The source has emitter or effector tables.
    pub has_particles: bool,
    /// Emitter faces and effector vertices, for the scene's particle owner.
    pub particle_emitters: Vec<ParticleEmitterRef>,
    pub particle_effectors: Vec<ParticleEffectorRef>,
    /// The source [`ModelUnlit::source_ids`](crate::modelunlit::ModelUnlit::source_ids) (the archive groups of its
    /// faces, in face order), shared by every copy. Read only by the NXT
    /// renderer's RT7 posing.
    pub source_ids: Option<std::sync::Arc<[u32]>>,
    bounds_valid: bool,
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    min_z: i32,
    max_z: i32,
    horizontal_radius: i32,
    radius: i32,
    /// The frozen height ([`GpuModel::height`]).
    height_valid: bool,
    height: i32,
}

impl GpuModel {
    /// `*self = source.clone()`, reusing this model's allocations (field by
    /// field `clone_from`; programme Phase 6: the animated loc and actor
    /// models are copied from their base model every frame). The
    /// destructuring is exhaustive, so a new field fails to compile here
    /// until it is copied too.
    pub fn copy_from(&mut self, source: &GpuModel) {
        let GpuModel {
            flags,
            detail,
            ambient,
            contrast,
            vertex_count_all,
            vertex_count,
            vx,
            vy,
            vz,
            vertex_source_models,
            vertex_groups,
            face_groups,
            unique_count,
            unique_vertex,
            unique_face,
            nx,
            ny,
            nz,
            ncount,
            u,
            v,
            merged,
            face_count,
            draw_face_count,
            face_colour,
            face_alpha,
            face_material,
            face_part,
            idx1,
            idx2,
            idx3,
            vertex_offsets,
            vertex_slots,
            batch_face_start,
            batch_min_vertex,
            batch_vertex_span,
            has_transparency,
            has_animated_uvs,
            billboards,
            face_source,
            source_face_count,
            source_face_priority,
            has_particles,
            particle_emitters,
            particle_effectors,
            source_ids,
            bounds_valid,
            min_x,
            max_x,
            min_y,
            max_y,
            min_z,
            max_z,
            horizontal_radius,
            radius,
            height_valid,
            height,
        } = source;
        self.flags = *flags;
        self.detail = *detail;
        self.ambient = *ambient;
        self.contrast = *contrast;
        self.vertex_count_all = *vertex_count_all;
        self.vertex_count = *vertex_count;
        self.vx.clone_from(vx);
        self.vy.clone_from(vy);
        self.vz.clone_from(vz);
        self.vertex_source_models.clone_from(vertex_source_models);
        self.vertex_groups.clone_from(vertex_groups);
        self.face_groups.clone_from(face_groups);
        self.unique_count = *unique_count;
        self.unique_vertex.clone_from(unique_vertex);
        self.unique_face.clone_from(unique_face);
        self.nx.clone_from(nx);
        self.ny.clone_from(ny);
        self.nz.clone_from(nz);
        self.ncount.clone_from(ncount);
        self.u.clone_from(u);
        self.v.clone_from(v);
        self.merged.clone_from(merged);
        self.face_count = *face_count;
        self.draw_face_count = *draw_face_count;
        self.face_colour.clone_from(face_colour);
        self.face_alpha.clone_from(face_alpha);
        self.face_material.clone_from(face_material);
        self.face_part.clone_from(face_part);
        self.idx1.clone_from(idx1);
        self.idx2.clone_from(idx2);
        self.idx3.clone_from(idx3);
        self.vertex_offsets.clone_from(vertex_offsets);
        self.vertex_slots.clone_from(vertex_slots);
        self.batch_face_start.clone_from(batch_face_start);
        self.batch_min_vertex.clone_from(batch_min_vertex);
        self.batch_vertex_span.clone_from(batch_vertex_span);
        self.has_transparency = *has_transparency;
        self.has_animated_uvs = *has_animated_uvs;
        self.billboards.clone_from(billboards);
        self.face_source.clone_from(face_source);
        self.source_face_count = *source_face_count;
        self.source_face_priority.clone_from(source_face_priority);
        self.has_particles = *has_particles;
        self.particle_emitters.clone_from(particle_emitters);
        self.particle_effectors.clone_from(particle_effectors);
        self.source_ids.clone_from(source_ids);
        self.bounds_valid = *bounds_valid;
        self.min_x = *min_x;
        self.max_x = *max_x;
        self.min_y = *min_y;
        self.max_y = *max_y;
        self.min_z = *min_z;
        self.max_z = *max_z;
        self.horizontal_radius = *horizontal_radius;
        self.radius = *radius;
        self.height_valid = *height_valid;
        self.height = *height;
    }
}

/// The material `id` refers to; a model naming a material the cache lacks is
/// an error.
fn find_material(materials: &MaterialStore, id: i16) -> anyhow::Result<&Material> {
    let idx = u32::from(id as u16);
    materials
        .get(idx)
        .ok_or_else(|| anyhow::anyhow!("material {idx} referenced by a model is missing"))
}

#[cfg(test)]
mod tests;
