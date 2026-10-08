//! Corpus check of the model layer: every model, animation frame set and
//! skeletal keyframe set of the local cache is decoded, run through the
//! build, lighting and posing paths the client uses, and reduced to a hash.
//! `fixtures/model-corpus.tsv` holds one digest per type, so a change in what
//! the model layer produces (values, defaults, errors, panics) fails the test
//! with the type named. The hash covers values only, never field names, so
//! renaming a field does not move a digest.
//!
//! Types (one row each per run):
//!
//! - `unlit`: `ModelUnlit::load`, every field of the decoded model.
//! - `unlit-edit`: the model after translate, rotate, recolour, rematerial and
//!   both scale forms.
//! - `unlit-merge`: each model merged with its successor in id order.
//! - `raw`: the wire-exact `model::decode` plus its texture payloads and the
//!   per-face corner UVs.
//! - `lit`: `GpuModel::new` at two lighting settings; every upload stream,
//!   the batches, the flags, the bounds and the particle and billboard tables.
//! - `posed`: a lit model after each transform the scene applies (mirror,
//!   turn, scale, tint, recolour, retexture), after a hill change of each
//!   kind onto a synthetic terrain, after a normal merge with its successor,
//!   and its hard shadow.
//! - `frameset`: `load_frameset`, every frame and skeleton of an `anims` group.
//! - `pose`: up to eight frames of each frame set applied to a fixed pool of
//!   labelled carrier models, alone and tweened with the next frame, at each
//!   of the four turn angles.
//! - `keyframes`: `load_keyframeset` and the skeletal pose transforms of every
//!   `anims.keyframes` set at four ticks.
//!
//! Two runs share the fixture: `sample` (every 200th model, every 100th frame
//! set, every 8th keyframe set; part of the default suite) and `full` (the
//! whole cache, `#[ignore]`d). The digest holds counts and hashes only, never
//! model data. Regenerate it only for an intended change of a decoded or
//! built value: `RS910_UPDATE_CORPUS=1 cargo test -p rs910-model --release
//! corpus -- --include-ignored`. `RS910_CORPUS_DUMP=<file>` writes every row
//! hash (`type<TAB>id<TAB>hash`) so two runs can be diffed row by row.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::anim::{
    load_frameset, load_keyframeset, AnimBase, AnimFrame, FrameSetData, KeyFrameSet,
};
use crate::billboard::BillboardStore;
use crate::cache::Pack;
use crate::floor::{FloorHeights, SunLighting};
use crate::gpumodel::{
    classic_transforms_selected, BuildParams, ClassicPose, GpuModel, ModelStores, PoseTarget,
    TerrainHeights, Transform, MODEL_DETAIL_FLAGS, MODEL_DETAIL_NO_TEXTURES,
};
use crate::model;
use crate::modelunlit::{ModelUnlit, MODEL_ARCHIVE};
use crate::particle::EmitterStore;
use crate::texture::MaterialStore;

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

/// FNV-1a 64.
struct Digest(u64);

impl Digest {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn put<T: Feed + ?Sized>(&mut self, value: &T) {
        value.feed(self);
    }
}

/// Values a digest can take, with their length or presence marks so that two
/// different shapes never produce the same byte stream.
trait Feed {
    fn feed(&self, digest: &mut Digest);
}

macro_rules! feed_int {
    ($($t:ty),*) => {$(
        impl Feed for $t {
            fn feed(&self, digest: &mut Digest) {
                digest.bytes(&self.to_le_bytes());
            }
        }
    )*};
}
feed_int!(u8, i8, u16, i16, u32, i32, u64, i64);

impl Feed for usize {
    fn feed(&self, digest: &mut Digest) {
        digest.bytes(&(*self as u64).to_le_bytes());
    }
}

impl Feed for bool {
    fn feed(&self, digest: &mut Digest) {
        digest.bytes(&[u8::from(*self)]);
    }
}

impl Feed for f32 {
    fn feed(&self, digest: &mut Digest) {
        digest.bytes(&self.to_bits().to_le_bytes());
    }
}

impl Feed for str {
    fn feed(&self, digest: &mut Digest) {
        digest.bytes(&(self.len() as u64).to_le_bytes());
        digest.bytes(self.as_bytes());
    }
}

impl<T: Feed> Feed for [T] {
    fn feed(&self, digest: &mut Digest) {
        digest.bytes(&(self.len() as u64).to_le_bytes());
        for item in self {
            item.feed(digest);
        }
    }
}

impl<T: Feed> Feed for Vec<T> {
    fn feed(&self, digest: &mut Digest) {
        self.as_slice().feed(digest);
    }
}

impl<T: Feed, const N: usize> Feed for [T; N] {
    fn feed(&self, digest: &mut Digest) {
        for item in self {
            item.feed(digest);
        }
    }
}

impl<T: Feed> Feed for Option<T> {
    fn feed(&self, digest: &mut Digest) {
        match self {
            None => digest.bytes(&[0]),
            Some(value) => {
                digest.bytes(&[1]);
                value.feed(digest);
            }
        }
    }
}

impl<A: Feed, B: Feed> Feed for (A, B) {
    fn feed(&self, digest: &mut Digest) {
        self.0.feed(digest);
        self.1.feed(digest);
    }
}

impl<A: Feed, B: Feed, C: Feed> Feed for (A, B, C) {
    fn feed(&self, digest: &mut Digest) {
        self.0.feed(digest);
        self.1.feed(digest);
        self.2.feed(digest);
    }
}

impl<A: Feed, B: Feed, C: Feed, D: Feed, E: Feed> Feed for (A, B, C, D, E) {
    fn feed(&self, digest: &mut Digest) {
        self.0.feed(digest);
        self.1.feed(digest);
        self.2.feed(digest);
        self.3.feed(digest);
        self.4.feed(digest);
    }
}

/// The `Debug` text without field and struct names, for the small config-side
/// value types (billboards, transforms) whose fields the model layer does not
/// own.
fn put_debug<T: Debug>(digest: &mut Digest, value: &T) {
    digest.put(strip_names(&format!("{value:?}")).as_str());
}

fn strip_names(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                i += 1;
                if chars[i - 1] == '"' {
                    break;
                }
            }
            continue;
        }
        let starts_word = (c.is_ascii_alphabetic() || c == '_')
            && (i == 0
                || !(chars[i - 1].is_ascii_alphanumeric()
                    || chars[i - 1] == '_'
                    || chars[i - 1] == '.'));
        if starts_word {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let field_name = chars.get(j) == Some(&':') && chars.get(j + 1) == Some(&' ');
            let struct_name = chars.get(j) == Some(&' ')
                && chars.get(j + 1) == Some(&'{')
                && c.is_ascii_uppercase();
            if field_name {
                i = j + 2;
                continue;
            }
            if struct_name {
                i = j + 1;
                continue;
            }
            out.push_str(&word);
            i = j;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// What each type feeds
// ---------------------------------------------------------------------------

fn feed_unlit(d: &mut Digest, m: &ModelUnlit) {
    d.put(&m.version);
    d.put(&m.vertex_count);
    d.put(&m.used_vertex_count);
    d.put(&m.vertex_x);
    d.put(&m.vertex_y);
    d.put(&m.vertex_z);
    d.put(&m.vertex_texture_vertex);
    d.put(&m.vertex_label);
    d.put(&m.vertex_source_models);
    d.put(&m.face_count);
    d.put(&m.textured_vertex_count);
    d.put(&m.texture_vertex_u);
    d.put(&m.texture_vertex_v);
    d.put(&m.face_vertex1);
    d.put(&m.face_vertex2);
    d.put(&m.face_vertex3);
    d.put(&m.face_texture_vertex_offset1);
    d.put(&m.face_texture_vertex_offset2);
    d.put(&m.face_texture_vertex_offset3);
    d.put(&m.face_type);
    d.put(&m.face_priority);
    d.put(&m.face_trans);
    d.put(&m.face_mapping);
    d.put(&m.face_colour);
    d.put(&m.face_material);
    d.put(&m.face_label);
    d.put(&m.default_priority);
    d.put(&m.face_source_models);
    d.put(&m.texture_triangle_count);
    d.put(&m.texture_triangle_type);
    d.put(&m.texture_triangle_vertex1);
    d.put(&m.texture_triangle_vertex2);
    d.put(&m.texture_triangle_vertex3);
    d.put(&m.texture_triangle_scale_x);
    d.put(&m.texture_triangle_scale_y);
    d.put(&m.texture_triangle_scale_z);
    d.put(&m.texture_triangle_speed);
    d.put(&m.texture_triangle_translation_u);
    d.put(&m.texture_triangle_translation_v);
    d.put(&m.texture_triangle_rotation);
    d.put(&m.texture_triangle_direction);
    put_debug(d, &m.emitters);
    put_debug(d, &m.effectors);
    put_debug(d, &m.billboard);
    d.put(&m.source_ids.as_ref().map(|ids| ids.to_vec()));
}

fn feed_raw(d: &mut Digest, data: &[u8]) {
    match model::decode(data) {
        Err(error) => d.put(format!("ERR {error:#}").as_str()),
        Ok(raw) => {
            d.put(&raw.version);
            d.put(&raw.verts);
            d.put(&raw.faces);
            d.put(&raw.colors);
            d.put(&raw.materials);
            d.put(&raw.priorities);
            d.put(&raw.alphas);
            d.put(&raw.face_types);
            put_debug(d, &raw.tex_tris);
            match model::decode_tex(data) {
                Err(error) => d.put(format!("ERR {error:#}").as_str()),
                Ok(tex) => {
                    d.put(&tex.vert_counts);
                    d.put(&tex.uvs);
                    d.put(&tex.face_mappings);
                    d.put(&tex.face_tex_offsets);
                    d.put(&tex.corner_uvs_batch(&raw));
                }
            }
        }
    }
}

/// The stores the lit-model build reads.
struct Stores {
    materials: MaterialStore,
    billboards: BillboardStore,
    emitters: EmitterStore,
}

fn lit(
    stores: &Stores,
    unlit: &ModelUnlit,
    flags: i32,
    ambient: i32,
    contrast: i32,
    detail: i32,
) -> anyhow::Result<GpuModel> {
    GpuModel::new(
        &ModelStores {
            materials: &stores.materials,
            billboards: &stores.billboards,
            emitters: &stores.emitters,
        },
        unlit,
        BuildParams {
            flags,
            ambient,
            contrast,
            detail,
        },
    )
}

/// Everything a consumer can read from a lit model.
fn feed_lit(d: &mut Digest, stores: &Stores, m: &mut GpuModel) {
    d.put(&m.flags);
    d.put(&m.detail);
    d.put(&m.ambient);
    d.put(&m.contrast);
    d.put(&m.face_count);
    d.put(&m.draw_face_count);
    d.put(&m.unique_count);
    d.put(&m.has_transparency);
    d.put(&m.has_animated_uvs);
    d.put(&m.has_particles);
    d.put(&m.source_face_count);
    d.put(&m.source_face_priority);
    d.put(&m.face_source);
    d.put(&m.face_part);
    d.put(&m.face_colour);
    d.put(&m.face_alpha);
    d.put(&m.face_material);
    d.put(&m.position_stream());
    d.put(&m.normal_stream());
    d.put(&m.uv_stream());
    d.put(&m.index_stream());
    d.put(&m.batches());
    match m.colour_stream(&stores.materials) {
        Ok(colours) => d.put(&colours),
        Err(error) => d.put(format!("ERR {error:#}").as_str()),
    }
    match m.albedo_stream(&stores.materials) {
        Ok(colours) => d.put(&colours),
        Err(error) => d.put(format!("ERR {error:#}").as_str()),
    }
    d.put(&m.draw_bounds());
    d.put(&m.cached_bounds());
    d.put(&(m.min_x(), m.max_x(), m.min_y(), m.max_y(), m.min_z()));
    d.put(&(
        m.max_z(),
        m.horizontal_radius(),
        m.radius(),
        m.height(),
        0_i32,
    ));
    put_debug(d, &m.billboard_instances());
    put_debug(d, &m.particle_emitters);
    put_debug(d, &m.particle_effectors);
    let (emitters, effectors) = m.particle_anchors(
        &crate::actor_matrix::Matrix::default(),
        crate::particle::Rotation::default(),
        7,
    );
    put_debug(d, &emitters);
    put_debug(d, &effectors);
}

fn feed_transforms(d: &mut Digest, ops: &[Transform]) {
    put_debug(d, &ops);
}

/// Lighting settings the client uses: loc models (flags 0x1F01F) and effect
/// models (ambient + 64, contrast + 850), textures on and off.
const LIGHTING: [(i32, i32, i32, i32); 2] = [
    (0x1_F01F, 64, 768, MODEL_DETAIL_FLAGS),
    (
        0x300,
        100,
        1000,
        MODEL_DETAIL_FLAGS | MODEL_DETAIL_NO_TEXTURES,
    ),
];

/// A hill of 5x5 tiles, so every hill-change kind has ground to sit on.
fn synthetic_ground() -> (FloorHeights, FloorHeights) {
    let heights = |k: i32| -> Vec<i32> {
        (0..36)
            .map(|i: i32| -(((i % 6) * 37 + (i / 6) * 53 + k * 400) % 300))
            .collect()
    };
    (
        FloorHeights::new(5, 5, 512, heights(0)),
        FloorHeights::new(5, 5, 512, heights(1)),
    )
}

fn row_unlit(pack: &Pack, id: u32) -> u64 {
    let mut d = Digest::new();
    match ModelUnlit::load(pack, id) {
        Err(error) => d.put(format!("ERR {error:#}").as_str()),
        Ok(m) => feed_unlit(&mut d, &m),
    }
    d.0
}

fn row_unlit_edit(pack: &Pack, id: u32) -> u64 {
    let mut d = Digest::new();
    match ModelUnlit::load(pack, id) {
        Err(_) => d.put("ERR"),
        Ok(base) => {
            let first_colour = base.face_colour.first().copied().unwrap_or(0);
            let first_material = base
                .face_material
                .as_ref()
                .and_then(|m| m.first().copied())
                .unwrap_or(-1);
            let mut m = base.clone();
            m.translate(17, -33, 5);
            m.rotate(513, 2049, 7);
            m.recolor(first_colour, 1234);
            m.rematerial(first_material, 21);
            feed_unlit(&mut d, &m);
            let mut m = base.clone();
            m.scale_by(1.25);
            feed_unlit(&mut d, &m);
            let mut m = base;
            m.scale_by_power_of_two(2);
            feed_unlit(&mut d, &m);
        }
    }
    d.0
}

fn row_unlit_merge(pack: &Pack, id: u32, next: u32) -> u64 {
    let mut d = Digest::new();
    match (ModelUnlit::load(pack, id), ModelUnlit::load(pack, next)) {
        (Ok(a), Ok(b)) => {
            feed_unlit(&mut d, &ModelUnlit::merge(&[&a, &b]));
            feed_unlit(
                &mut d,
                &ModelUnlit::merge_slots(&[Some(&a), None, Some(&b)]),
            );
        }
        _ => d.put("ERR"),
    }
    d.0
}

fn row_raw(pack: &Pack, id: u32) -> u64 {
    let mut d = Digest::new();
    match pack
        .read_group(MODEL_ARCHIVE, id)
        .ok()
        .and_then(|mut files| files.remove(&0))
    {
        None => d.put("MISSING"),
        Some(data) => feed_raw(&mut d, &data),
    }
    d.0
}

fn row_lit(pack: &Pack, stores: &Stores, id: u32) -> u64 {
    let mut d = Digest::new();
    match ModelUnlit::load(pack, id) {
        Err(_) => d.put("ERR"),
        Ok(unlit) => {
            for (flags, ambient, contrast, detail) in LIGHTING {
                match lit(stores, &unlit, flags, ambient, contrast, detail) {
                    Err(error) => d.put(format!("ERR {error:#}").as_str()),
                    Ok(mut m) => feed_lit(&mut d, stores, &mut m),
                }
            }
        }
    }
    d.0
}

fn row_posed(pack: &Pack, stores: &Stores, id: u32, next: u32) -> u64 {
    let mut d = Digest::new();
    let Ok(unlit) = ModelUnlit::load(pack, id) else {
        d.put("ERR");
        return d.0;
    };
    let (flags, ambient, contrast, detail) = LIGHTING[0];
    let Ok(base) = lit(stores, &unlit, flags, ambient, contrast, detail) else {
        d.put("ERR");
        return d.0;
    };
    let first_colour = base.face_colour.first().copied().unwrap_or(0);
    let first_material = base.face_material.first().copied().unwrap_or(-1);

    // The per-placement chain a loc goes through.
    let mut m = base.clone();
    m.mirror();
    m.rotate_y(4096);
    m.rotate_x(1024);
    m.rotate_z(512);
    m.translate(180, 0, -180);
    m.scale(160, 96, 128);
    m.tint(20, 3, 40, 64);
    m.recolor(first_colour, 2500);
    let _ = m.retexture(&stores.materials, first_material, 24);
    m.rotate_y_keep_normals(2050);
    feed_lit(&mut d, stores, &mut m);

    // Each hill-change kind onto the synthetic terrain.
    let (ground, above) = synthetic_ground();
    for kind in 1..=5 {
        let mut m = base.clone();
        m.hill_change(
            kind,
            30,
            TerrainHeights {
                floor: &ground,
                above: Some(&above),
            },
            [512, 40, 512],
        );
        feed_lit(&mut d, stores, &mut m);
    }

    // The normal merge with the neighbouring model.
    if let Ok(other) = ModelUnlit::load(pack, next) {
        if let Ok(mut other) = lit(stores, &other, flags, ambient, contrast, detail) {
            let mut m = base.clone();
            m.merge_normals(&mut other, 8, 0, -8);
            feed_lit(&mut d, stores, &mut m);
            feed_lit(&mut d, stores, &mut other);
        }
    }

    // The hard shadow, then a replacement transform.
    let sun = SunLighting::from_set_sun(0.8, 1.0, 0.5, 0.3, -1.0, 0.4);
    let mut m = base.clone();
    match m.hard_shadow(&sun) {
        None => d.put("NOSHADOW"),
        Some(shadow) => put_debug(&mut d, &shadow),
    }
    m.apply_srt([0.0, 0.3, 0.0, 0.95, 40.0, -20.0, 10.0, 1.2, 0.9, 1.1]);
    feed_lit(&mut d, stores, &mut m);
    d.0
}

fn feed_animation_set(d: &mut Digest, set: &FrameSetData) {
    put_debug(d, &set.bases);
    put_debug(d, &set.frames);
}

fn row_frameset(pack: &Pack, group: u32) -> u64 {
    let mut d = Digest::new();
    match load_frameset(pack, group) {
        Err(error) => d.put(format!("ERR {error:#}").as_str()),
        Ok(set) => feed_animation_set(&mut d, &set),
    }
    d.0
}

/// Models with labels to pose: the ones with the most vertex labels, plus
/// some with billboards, some with face labels and some with particles, so
/// every animation op type has a carrier.
fn carrier_pool(pack: &Pack, stores: &Stores, ids: &[u32]) -> Vec<GpuModel> {
    let mut scored: Vec<(usize, u32)> = Vec::new();
    let mut billboards = Vec::new();
    let mut face_labels = Vec::new();
    let mut particles = Vec::new();
    // Every seventh model is enough to find them.
    for &id in ids.iter().step_by(7) {
        let Ok(m) = ModelUnlit::load(pack, id) else {
            continue;
        };
        if let Some(labels) = &m.vertex_label {
            let max = labels.iter().copied().max().unwrap_or(-1);
            if max > 0 && m.face_count < 1500 {
                scored.push((max as usize, id));
            }
        }
        if m.billboard.is_some() && billboards.len() < 4 {
            billboards.push(id);
        }
        if m.face_label.is_some() && m.vertex_label.is_some() && face_labels.len() < 4 {
            face_labels.push(id);
        }
        if m.emitters.is_some() && particles.len() < 4 {
            particles.push(id);
        }
    }
    scored.sort_by(|a, b| b.cmp(a));
    let mut chosen: Vec<u32> = scored.iter().take(8).map(|&(_, id)| id).collect();
    chosen.extend(billboards);
    chosen.extend(face_labels);
    chosen.extend(particles);
    chosen.sort_unstable();
    chosen.dedup();
    let (flags, ambient, contrast, detail) = LIGHTING[0];
    chosen
        .into_iter()
        .filter_map(|id| {
            let unlit = ModelUnlit::load(pack, id).ok()?;
            lit(
                stores,
                &unlit,
                flags | 0x400 | 0x100 | 0x80,
                ambient,
                contrast,
                detail,
            )
            .ok()
        })
        .collect()
}

fn feed_posed(d: &mut Digest, stores: &Stores, m: &mut GpuModel) {
    d.put(&m.position_stream());
    d.put(&m.normal_stream());
    d.put(&m.face_alpha);
    d.put(&m.face_colour);
    match m.colour_stream(&stores.materials) {
        Ok(colours) => d.put(&colours),
        Err(_) => d.put("ERR"),
    }
    d.put(&(m.min_x(), m.max_x(), m.min_y(), m.max_y(), m.radius()));
    put_debug(d, &m.billboard_instances());
}

fn row_pose(stores: &Stores, pool: &[GpuModel], group: u32, set: &FrameSetData) -> u64 {
    let mut d = Digest::new();
    if pool.is_empty() {
        return d.0;
    }
    let frames: Vec<(&AnimBase, &AnimFrame)> = set
        .frames
        .keys()
        .filter_map(|&index| set.frame(index))
        .collect();
    // At most `POSED_FRAMES` frames of a set, spread over it.
    let stride = frames.len().div_ceil(POSED_FRAMES).max(1);
    for (at, &(base, frame)) in frames.iter().enumerate().step_by(stride) {
        let next = frames.get(at + 1).map(|&(_, f)| f);
        let carrier = &pool[(group as usize + at) % pool.len()];
        for angle in 0..4 {
            for (tick, next) in [(0, None), (1, next)] {
                let ops = classic_transforms_selected(
                    ClassicPose {
                        base,
                        frame,
                        next,
                        tick,
                        duration: 3,
                    },
                    PoseTarget {
                        angle,
                        normals: angle & 1 == 1,
                        ..PoseTarget::default()
                    },
                );
                if angle == 0 && tick == 0 {
                    feed_transforms(&mut d, &ops);
                }
                let mut m = carrier.clone();
                m.apply_animation(&ops);
                feed_posed(&mut d, stores, &mut m);
                if angle == 0 && tick == 0 {
                    let mut shadow = carrier.clone();
                    shadow.apply_shadow_animation(base, frame);
                    feed_posed(&mut d, stores, &mut shadow);
                }
            }
        }
    }
    d.0
}

fn feed_keyframes(d: &mut Digest, set: &KeyFrameSet) {
    put_debug(d, set);
    match crate::animation_skeletal::SkeletalPose::new(set.clone()) {
        Err(error) => d.put(format!("ERR {error:#}").as_str()),
        Ok(pose) => {
            let (start, end) = (i32::from(set.start), i32::from(set.end));
            for tick in [0, start, (start + end) / 2, end] {
                for angle in [0, 1] {
                    feed_transforms(d, &pose.transforms(tick, angle, angle == 1));
                }
            }
        }
    }
}

fn row_keyframes(pack: &Pack, id: u32) -> u64 {
    let mut d = Digest::new();
    match load_keyframeset(pack, id) {
        Err(error) => d.put(format!("ERR {error:#}").as_str()),
        Ok(set) => feed_keyframes(&mut d, &set),
    }
    d.0
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Coverage {
    Sample,
    Full,
}

impl Coverage {
    fn name(self) -> &'static str {
        match self {
            Self::Sample => "sample",
            Self::Full => "full",
        }
    }

    fn keeps(self, index: usize, stride: usize) -> bool {
        self == Self::Full || index.is_multiple_of(stride)
    }
}

/// One type's result: its `(id, row hash)` rows in id order.
struct Section {
    name: &'static str,
    rows: Vec<(u32, u64)>,
}

impl Section {
    fn digest(&self) -> u64 {
        let mut d = Digest::new();
        for &(id, hash) in &self.rows {
            d.put(&id);
            d.put(&hash);
        }
        d.0
    }
}

/// Frames of one set that are applied to a carrier model.
const POSED_FRAMES: usize = 8;

const PANIC_ROW: u64 = 0xdead_dead_dead_dead;

/// Run `row` over `ids` on every core; a panic is a row of its own (the code
/// under test computes with wrapping arithmetic and indexes freely).
fn run_rows<F>(name: &'static str, ids: &[u32], row: F) -> Section
where
    F: Fn(usize, u32) -> u64 + Sync,
{
    let started = std::time::Instant::now();
    let next = AtomicUsize::new(0);
    let rows: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::with_capacity(ids.len()));
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let at = next.fetch_add(1, Ordering::Relaxed);
                let Some(&id) = ids.get(at) else { break };
                let hash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| row(at, id)))
                    .unwrap_or(PANIC_ROW);
                rows.lock().unwrap().push((id, hash));
            });
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_unstable();
    eprintln!(
        "corpus {name}: {} rows in {:.1}s",
        rows.len(),
        started.elapsed().as_secs_f32()
    );
    Section { name, rows }
}

fn group_ids(pack: &Pack, archive: &str) -> Vec<u32> {
    pack.read_archive_index(archive)
        .unwrap_or_else(|error| panic!("{archive} index: {error}"))
        .group_id
        .clone()
}

fn snapshot(pack: &Pack, coverage: Coverage) -> Vec<Section> {
    let stores = Stores {
        materials: MaterialStore::load(pack).expect("materials"),
        billboards: BillboardStore::load(pack).expect("billboards"),
        emitters: EmitterStore::load(pack).expect("particle emitters"),
    };
    let all_models = group_ids(pack, MODEL_ARCHIVE);
    let models: Vec<u32> = all_models
        .iter()
        .enumerate()
        .filter(|&(i, _)| coverage.keeps(i, 200))
        .map(|(_, &id)| id)
        .collect();
    let successor = |id: u32| -> u32 {
        let at = all_models.binary_search(&id).unwrap_or(0);
        all_models[(at + 1) % all_models.len()]
    };
    let mut out = vec![
        run_rows("unlit", &models, |_, id| row_unlit(pack, id)),
        run_rows("unlit-edit", &models, |_, id| row_unlit_edit(pack, id)),
        run_rows("unlit-merge", &models, |_, id| {
            row_unlit_merge(pack, id, successor(id))
        }),
        run_rows("raw", &models, |_, id| row_raw(pack, id)),
        run_rows("lit", &models, |_, id| row_lit(pack, &stores, id)),
        run_rows("posed", &models, |_, id| {
            row_posed(pack, &stores, id, successor(id))
        }),
    ];

    let all_sets = group_ids(pack, "anims");
    let sets: Vec<u32> = all_sets
        .iter()
        .enumerate()
        .filter(|&(i, _)| coverage.keeps(i, 100))
        .map(|(_, &id)| id)
        .collect();
    out.push(run_rows("frameset", &sets, |_, id| row_frameset(pack, id)));
    let pool = carrier_pool(pack, &stores, &all_models);
    out.push(run_rows("pose", &sets, |_, id| {
        match load_frameset(pack, id) {
            Ok(set) => row_pose(&stores, &pool, id, &set),
            Err(_) => 0,
        }
    }));

    // `anims.keyframes`: one group per skeletal set.
    let all_keyframes = group_ids(pack, "anims.keyframes");
    let keyframes: Vec<u32> = all_keyframes
        .iter()
        .enumerate()
        .filter(|&(i, _)| coverage.keeps(i, 8))
        .map(|(_, &id)| id)
        .collect();
    out.push(run_rows("keyframes", &keyframes, |_, id| {
        row_keyframes(pack, id)
    }));
    out
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

fn fixture_path() -> PathBuf {
    rs910_core::test_support::client_dir().join("crates/rs910-model/fixtures/model-corpus.tsv")
}

/// `run<TAB>type<TAB>rows<TAB>panics<TAB>digest` lines of one run.
fn table(coverage: Coverage, sections: &[Section]) -> Vec<String> {
    sections
        .iter()
        .map(|s| {
            let panics = s.rows.iter().filter(|&&(_, h)| h == PANIC_ROW).count();
            format!(
                "{}\t{}\t{}\t{}\t{:016x}",
                coverage.name(),
                s.name,
                s.rows.len(),
                panics,
                s.digest()
            )
        })
        .collect()
}

fn dump(coverage: Coverage, sections: &[Section]) {
    let Some(path) = std::env::var_os("RS910_CORPUS_DUMP") else {
        return;
    };
    let mut text = String::new();
    for s in sections {
        for &(id, hash) in &s.rows {
            text.push_str(&format!(
                "{}\t{}\t{}\t{:016x}\n",
                coverage.name(),
                s.name,
                id,
                hash
            ));
        }
    }
    let path = PathBuf::from(path);
    let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
    existing.push_str(&text);
    std::fs::write(path, existing).unwrap();
}

fn read_fixture() -> BTreeMap<(String, String), String> {
    let text = std::fs::read_to_string(fixture_path()).unwrap_or_default();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let mut cols = l.splitn(3, '\t');
            let run = cols.next().unwrap().to_string();
            let name = cols.next().unwrap().to_string();
            ((run, name), l.to_string())
        })
        .collect()
}

fn check(coverage: Coverage) {
    let pack = crate::test_support::require_pack("client.models.js5");
    let sections = snapshot(&pack, coverage);
    dump(coverage, &sections);
    let now = table(coverage, &sections);
    if std::env::var_os("RS910_UPDATE_CORPUS").is_some() {
        let mut rows = read_fixture();
        for line in &now {
            let mut cols = line.splitn(3, '\t');
            let key = (
                cols.next().unwrap().to_string(),
                cols.next().unwrap().to_string(),
            );
            rows.insert(key, line.clone());
        }
        let mut text = String::from(
            "# Model corpus digest: see src/corpus.rs. Columns: run, type, rows, rows that panicked, digest. Hashes and counts only.\n",
        );
        for run in ["sample", "full"] {
            for ((r, _), line) in &rows {
                if r == run {
                    text.push_str(line);
                    text.push('\n');
                }
            }
        }
        std::fs::create_dir_all(fixture_path().parent().unwrap()).unwrap();
        std::fs::write(fixture_path(), text).unwrap();
        return;
    }
    let expected = read_fixture();
    let mut bad = Vec::new();
    for line in &now {
        let mut cols = line.splitn(3, '\t');
        let key = (
            cols.next().unwrap().to_string(),
            cols.next().unwrap().to_string(),
        );
        match expected.get(&key) {
            Some(want) if want == line => {}
            Some(want) => bad.push(format!("{}: now `{line}`, fixture `{want}`", key.1)),
            None => bad.push(format!("{}: not in the fixture", key.1)),
        }
    }
    assert!(
        bad.is_empty(),
        "the model layer produces different values ({} types):\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn sampled_corpus_matches_the_committed_digest() {
    check(Coverage::Sample);
}

#[test]
#[ignore = "nightly: the whole cache"]
fn full_corpus_matches_the_committed_digest() {
    check(Coverage::Full);
}

// ---------------------------------------------------------------------------
// Models of the cache that used to panic the build or the posing. Each
// fault class has one real model; the original client throws on the same
// input, and the port must drop or skip instead of crashing.
// ---------------------------------------------------------------------------

/// Stores and the lit model `id` at the loc lighting settings.
fn lit_model(pack: &Pack, stores: &Stores, id: u32) -> anyhow::Result<GpuModel> {
    let unlit = ModelUnlit::load(pack, id)?;
    let (flags, ambient, contrast, detail) = LIGHTING[0];
    lit(stores, &unlit, flags, ambient, contrast, detail)
}

fn load_stores(pack: &Pack) -> Stores {
    Stores {
        materials: MaterialStore::load(pack).expect("materials"),
        billboards: BillboardStore::load(pack).expect("billboards"),
        emitters: EmitterStore::load(pack).expect("particle emitters"),
    }
}

fn vertices(m: &GpuModel) -> Vec<[i32; 3]> {
    (0..m.vertex_count as usize)
        .map(|i| [m.vx[i], m.vy[i], m.vz[i]])
        .collect()
}

/// A face whose stored texture coordinates run past the model's table: the
/// build fails (the caller drops the model) instead of indexing out of range.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn stored_texture_coordinates_outside_the_table_fail_the_build() {
    let pack = crate::test_support::require_pack("client.models.js5");
    let stores = load_stores(&pack);
    // Model 24445 has 114 texture vertices and a face reading vertex 117.
    let Err(error) = lit_model(&pack, &stores, 24445) else {
        panic!("model 24445 must not build");
    };
    assert!(format!("{error:#}").contains("texture vertex 117 of 114"));
}

/// A model with no vertices has nothing to drape: every hill change leaves it
/// alone (its bounds are the empty sentinels, which walk off the height grid).
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn empty_model_ignores_every_hill_change() {
    let pack = crate::test_support::require_pack("client.models.js5");
    let stores = load_stores(&pack);
    let base = lit_model(&pack, &stores, 1077).expect("builds");
    assert_eq!(base.vertex_count, 0);
    let (ground, above) = synthetic_ground();
    for kind in 1..=5 {
        let mut m = base.clone();
        m.hill_change(
            kind,
            30,
            TerrainHeights {
                floor: &ground,
                above: Some(&above),
            },
            [512, 40, 512],
        );
        assert!(vertices(&m).is_empty());
    }
}

/// The stretch hill change divides by the model's height and the softened
/// one by its value; where the original divides by zero the model stays as
/// it was.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn hill_changes_that_divide_by_zero_leave_the_model_alone() {
    let pack = crate::test_support::require_pack("client.models.js5");
    let stores = load_stores(&pack);
    let (ground, above) = synthetic_ground();
    let terrain = TerrainHeights {
        floor: &ground,
        above: Some(&above),
    };
    // Model 596 has vertices and is exactly flat.
    let mut flat = lit_model(&pack, &stores, 596).expect("builds");
    assert!(flat.vertex_count > 0 && flat.max_y() == flat.min_y());
    let before = vertices(&flat);
    flat.hill_change(5, 30, terrain, [512, 40, 512]);
    assert_eq!(vertices(&flat), before);
    // Model 6321 has a height; a zero value has nothing to scale by.
    let mut tall = lit_model(&pack, &stores, 6321).expect("builds");
    assert!(tall.max_y() != tall.min_y());
    let before = vertices(&tall);
    tall.hill_change(2, 0, terrain, [512, 40, 512]);
    assert_eq!(vertices(&tall), before);
}

/// The softened hill change multiplies in 32 bits and wraps, as the original
/// does; the test profile checks overflow, so this fails if it ever traps.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn softened_hill_change_wraps_instead_of_trapping() {
    let pack = crate::test_support::require_pack("client.models.js5");
    let stores = load_stores(&pack);
    let (ground, above) = synthetic_ground();
    let base = lit_model(&pack, &stores, 6321).expect("builds");
    let mut m = base.clone();
    m.hill_change(
        2,
        30,
        TerrainHeights {
            floor: &ground,
            above: Some(&above),
        },
        [512, 40, 512],
    );
    assert_ne!(vertices(&m), vertices(&base));
}

/// A neighbour with more than 32767 unique vertices has slots that read
/// negative in one walk of the normal merge; the merge stops there instead of
/// indexing with them.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn normal_merge_with_a_huge_neighbour_stops_at_unreadable_slots() {
    let pack = crate::test_support::require_pack("client.models.js5");
    let stores = load_stores(&pack);
    let mut model = lit_model(&pack, &stores, 67632).expect("builds");
    let mut neighbour = lit_model(&pack, &stores, 67633).expect("builds");
    assert!(neighbour.unique_count > 32767);
    model.merge_normals(&mut neighbour, 8, 0, -8);
}
